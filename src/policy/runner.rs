//! The frame loop. Per tick:
//!   state_t  = capture_generic_game_state_buffer()            (STATE frame from the client)
//!   action_t = evaluate_policy(state_t, game_provided_rules)   (a pluggable `Policy`)
//!   send_generic_action_buffer_to_game(action_t)               (ACTION frame)
//!   log_generic_frame_telemetry(state_t)                       (in memory; written per episode)
//!
//! `lockstep`: the client waits for each ACTION, so game time is decoupled from policy latency
//! and a run is reproducible. `hold`: every STATE is answered at once with the last finished
//! action while the policy evaluates a snapshot in the background, so the game keeps its native
//! rate; runs then depend on wall-clock timing and are not reproducible.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};

use super::diagnostics::Trajectory;
use super::policies::{Act, Policy, PolicyContext};
use super::protocol::{Msg, read_msg, write_msg};
use super::schema::GameSchema;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    #[default]
    Lockstep,
    Hold,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub seeds: Vec<u64>,
    /// Ticks per episode, including the final state.
    pub max_ticks: u32,
    pub rules: String,
    pub mode: LoopMode,
    /// Longest wait for one frame from the client.
    pub frame_timeout: Duration,
    /// Write `episode-<seed>.jsonl` here when set.
    pub record_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameRec {
    pub tick: u64,
    pub state: Vec<f64>,
    /// None: deferred to the game's own policy, or the episode's final state.
    pub action: Option<Vec<u32>>,
    /// Time spent in the policy for this tick (lockstep) or by the evaluation whose result was
    /// first applied at this tick (hold).
    pub policy_us: u64,
}

#[derive(Debug, Clone)]
pub struct Episode {
    pub seed: u64,
    pub frames: Vec<FrameRec>,
    pub terminal: bool,
    pub digest: String,
}

impl Episode {
    pub fn trajectory(&self) -> Trajectory {
        Trajectory {
            seed: self.seed,
            states: self.frames.iter().map(|f| f.state.clone()).collect(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct Timing {
    pub mode: Option<LoopMode>,
    pub tick_hz: f64,
    /// Per-tick policy budget at the native rate.
    pub budget_us: f64,
    pub ticks: u64,
    /// Policy evaluations measured.
    pub evaluations: u64,
    pub policy_us_p50: u64,
    pub policy_us_p95: u64,
    pub policy_us_max: u64,
    /// Evaluations that took longer than one tick.
    pub deadline_misses: u64,
    /// Ticks per second the whole loop achieved (client + transport + policy).
    pub achieved_hz: f64,
    /// p95 evaluation fits the per-tick budget.
    pub realtime_feasible: bool,
    /// hold: ticks answered with an action decided on an older state.
    pub held_ticks: u64,
    /// hold: mean age, in ticks, of the state the applied action was decided on.
    pub mean_action_age_ticks: f64,
    /// lockstep with a deterministic client and policy: equal seeds give equal digests.
    pub reproducible: bool,
}

pub struct Sweep {
    pub hello: Value,
    pub policy: String,
    pub policy_stats: Value,
    pub episodes: Vec<Episode>,
    pub timing: Timing,
}

type Eval<'p, P> =
    Pin<Box<dyn Future<Output = (Result<Act, String>, &'p mut P, u64, u64)> + Send + 'p>>;

async fn next_state<S: AsyncRead + Unpin>(
    io: &mut S,
    t: Duration,
) -> Result<(u64, bool, Vec<u8>), String> {
    match tokio::time::timeout(t, read_msg(io)).await {
        Err(_) => Err(format!(
            "no frame from the client within {} ms",
            t.as_millis()
        )),
        Ok(Err(e)) => Err(e),
        Ok(Ok(Msg::State {
            tick,
            terminal,
            state,
        })) => Ok((tick, terminal, state)),
        Ok(Ok(Msg::Error(e))) => Err(format!("client error: {e}")),
        Ok(Ok(m)) => Err(format!("expected STATE, got {m:?}")),
    }
}

fn action_frame(
    schema: &GameSchema,
    tick: u64,
    act: &Act,
) -> Result<(Msg, Option<Vec<u32>>), String> {
    Ok(match act {
        Act::Action(a) => (
            Msg::Action {
                tick,
                action: schema.encode_action(a)?,
            },
            Some(a.clone()),
        ),
        Act::Defer => (
            Msg::Action {
                tick,
                action: Vec::new(),
            },
            None,
        ),
    })
}

fn digest(frames: &[FrameRec]) -> String {
    let mut h = Sha256::new();
    for f in frames {
        h.update(f.tick.to_le_bytes());
        for v in &f.state {
            h.update(v.to_bits().to_le_bytes());
        }
        match &f.action {
            Some(a) => a.iter().for_each(|x| h.update(x.to_le_bytes())),
            None => h.update([0xFF]),
        }
    }
    h.finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Run every seed against a connected client. On any error the client gets an ERROR frame.
pub async fn run_sweep<S, P>(
    io: &mut S,
    schema: &GameSchema,
    policy: &mut P,
    opts: &RunOptions,
) -> Result<Sweep, String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    P: Policy,
{
    let r = sweep(io, schema, policy, opts).await;
    if let Err(e) = &r {
        let _ = write_msg(io, &Msg::Error(e.clone())).await;
    }
    policy.end().await;
    r
}

async fn sweep<S, P>(
    io: &mut S,
    schema: &GameSchema,
    policy: &mut P,
    opts: &RunOptions,
) -> Result<Sweep, String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    P: Policy,
{
    if opts.max_ticks == 0 || opts.seeds.is_empty() {
        return Err("need max_ticks >= 1 and at least one seed".into());
    }
    let hello = match tokio::time::timeout(opts.frame_timeout, read_msg(io)).await {
        Ok(Ok(Msg::Hello(v))) => v,
        Ok(Ok(m)) => return Err(format!("expected HELLO, got {m:?}")),
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err("no HELLO from the client".into()),
    };
    if let Some(n) = hello.get("schema").and_then(Value::as_str)
        && n != schema.name
    {
        return Err(format!(
            "client speaks schema `{n}`, expected `{}`",
            schema.name
        ));
    }
    if let Some(b) = hello.get("state_bytes").and_then(Value::as_u64)
        && b as usize != schema.state_bytes()
    {
        return Err(format!(
            "client state is {b} bytes, schema says {}",
            schema.state_bytes()
        ));
    }
    let mut eval_us: Vec<u64> = Vec::new();
    let mut held_ticks = 0u64;
    let mut ages: Vec<u64> = Vec::new();
    let mut episodes = Vec::new();
    let t_run = Instant::now();
    for &seed in &opts.seeds {
        policy.begin(seed).await?;
        write_msg(
            io,
            &Msg::Reset {
                seed,
                max_ticks: opts.max_ticks,
            },
        )
        .await?;
        let (frames, terminal) = match opts.mode {
            LoopMode::Lockstep => {
                lockstep(io, schema, &mut *policy, opts, seed, &mut eval_us).await?
            }
            LoopMode::Hold => {
                hold(
                    io,
                    schema,
                    &mut *policy,
                    opts,
                    seed,
                    &mut eval_us,
                    &mut held_ticks,
                    &mut ages,
                )
                .await?
            }
        };
        let ep = Episode {
            seed,
            digest: digest(&frames),
            frames,
            terminal,
        };
        if let Some(dir) = &opts.record_dir {
            write_recording(dir, schema, &policy.name(), opts.mode, &ep)?;
        }
        episodes.push(ep);
    }
    write_msg(io, &Msg::Bye).await?;
    let ticks: u64 = episodes.iter().map(|e| e.frames.len() as u64).sum();
    let budget = schema.tick_budget_us();
    let mut sorted = eval_us.clone();
    sorted.sort_unstable();
    let pct = |q: f64| -> u64 {
        if sorted.is_empty() {
            0
        } else {
            sorted[((sorted.len() - 1) as f64 * q).round() as usize]
        }
    };
    let secs = t_run.elapsed().as_secs_f64().max(1e-9);
    let timing = Timing {
        mode: Some(opts.mode),
        tick_hz: schema.tick_hz,
        budget_us: budget,
        ticks,
        evaluations: sorted.len() as u64,
        policy_us_p50: pct(0.5),
        policy_us_p95: pct(0.95),
        policy_us_max: sorted.last().copied().unwrap_or(0),
        deadline_misses: sorted.iter().filter(|u| **u as f64 > budget).count() as u64,
        achieved_hz: (ticks as f64 / secs * 10.0).round() / 10.0,
        realtime_feasible: (pct(0.95) as f64) <= budget,
        held_ticks,
        mean_action_age_ticks: if ages.is_empty() {
            0.0
        } else {
            ages.iter().sum::<u64>() as f64 / ages.len() as f64
        },
        reproducible: opts.mode == LoopMode::Lockstep,
    };
    Ok(Sweep {
        hello,
        policy: policy.name(),
        policy_stats: policy.stats(),
        episodes,
        timing,
    })
}

async fn lockstep<S, P>(
    io: &mut S,
    schema: &GameSchema,
    policy: &mut P,
    opts: &RunOptions,
    seed: u64,
    eval_us: &mut Vec<u64>,
) -> Result<(Vec<FrameRec>, bool), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    P: Policy,
{
    let mut frames = Vec::new();
    loop {
        let (tick, terminal, raw) = next_state(io, opts.frame_timeout).await?;
        let state = schema.decode_state(&raw)?;
        if terminal || frames.len() + 1 >= opts.max_ticks as usize {
            frames.push(FrameRec {
                tick,
                state,
                action: None,
                policy_us: 0,
            });
            return Ok((frames, terminal));
        }
        let ctx = PolicyContext {
            schema,
            rules: &opts.rules,
            seed,
            tick,
            raw: &raw,
        };
        let t0 = Instant::now();
        let act = policy.act(&state, &ctx).await?;
        let us = t0.elapsed().as_micros() as u64;
        eval_us.push(us);
        let (msg, action) = action_frame(schema, tick, &act)?;
        write_msg(io, &msg).await?;
        frames.push(FrameRec {
            tick,
            state,
            action,
            policy_us: us,
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn hold<'p, S, P>(
    io: &mut S,
    schema: &'p GameSchema,
    policy: &'p mut P,
    opts: &'p RunOptions,
    seed: u64,
    eval_us: &mut Vec<u64>,
    held_ticks: &mut u64,
    ages: &mut Vec<u64>,
) -> Result<(Vec<FrameRec>, bool), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    P: Policy,
{
    let mut frames = Vec::new();
    let mut idle: Option<&'p mut P> = Some(policy);
    let mut pending: Option<Eval<'p, P>> = None;
    // Last finished action, the tick of the state it was decided on, and its evaluation time.
    let mut held: Option<(Act, u64, u64)> = None;
    let mut fresh = false;
    let result = loop {
        // The read future lives across select iterations, so a finished evaluation never
        // cancels a half-read frame.
        let state = {
            let mut read = Box::pin(next_state(io, opts.frame_timeout));
            loop {
                let ev = tokio::select! {
                    r = async {
                        match pending.as_mut() {
                            Some(f) => f.await,
                            None => std::future::pending().await,
                        }
                    } => Err(r),
                    m = &mut read => Ok(m),
                };
                match ev {
                    Err((r, p, decided_at, us)) => {
                        pending = None;
                        idle = Some(p);
                        eval_us.push(us);
                        held = Some((r?, decided_at, us));
                        fresh = true;
                    }
                    Ok(m) => break m,
                }
            }
        };
        let (tick, terminal, raw) = match state {
            Ok(s) => s,
            Err(e) => break Err(e),
        };
        let st = match schema.decode_state(&raw) {
            Ok(s) => s,
            Err(e) => break Err(e),
        };
        if terminal || frames.len() + 1 >= opts.max_ticks as usize {
            frames.push(FrameRec {
                tick,
                state: st,
                action: None,
                policy_us: 0,
            });
            break Ok(terminal);
        }
        if let Some(p) = idle.take() {
            let (s2, rules) = (st.clone(), opts.rules.as_str());
            pending = Some(Box::pin(async move {
                let ctx = PolicyContext {
                    schema,
                    rules,
                    seed,
                    tick,
                    raw: &raw,
                };
                let t0 = Instant::now();
                let r = p.act(&s2, &ctx).await;
                (r, p, tick, t0.elapsed().as_micros() as u64)
            }));
        }
        let (act, us) = match &held {
            Some((a, at, us)) => {
                ages.push(tick.saturating_sub(*at));
                if !fresh {
                    *held_ticks += 1;
                }
                (a.clone(), if fresh { *us } else { 0 })
            }
            None => (Act::Defer, 0),
        };
        fresh = false;
        let (msg, action) = action_frame(schema, tick, &act)?;
        write_msg(io, &msg).await?;
        frames.push(FrameRec {
            tick,
            state: st,
            action,
            policy_us: us,
        });
    };
    // Let an outstanding evaluation finish so the policy is free for the next episode.
    if let Some(f) = pending.take() {
        let (r, _, _, us) = f.await;
        eval_us.push(us);
        r?;
    }
    result.map(|t| (frames, t))
}

fn write_recording(
    dir: &Path,
    schema: &GameSchema,
    policy: &str,
    mode: LoopMode,
    ep: &Episode,
) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("episode-{}.jsonl", ep.seed));
    let f = std::fs::File::create(&path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let mut w = std::io::BufWriter::new(f);
    let head = json!({"seed": ep.seed, "schema": schema.name, "version": schema.version,
        "policy": policy, "mode": mode, "terminal": ep.terminal, "digest": ep.digest});
    let mut out = || -> std::io::Result<()> {
        writeln!(w, "{head}")?;
        for fr in &ep.frames {
            writeln!(
                w,
                "{}",
                json!({"t": fr.tick, "s": fr.state, "a": fr.action, "us": fr.policy_us})
            )?;
        }
        w.flush()
    };
    out().map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Read recorded episodes: one `.jsonl` file, or every `.jsonl` file in a directory. A line with
/// `seed` starts an episode; lines with `s` are its states.
pub fn read_recordings(path: &Path) -> Result<Vec<Trajectory>, String> {
    let mut files: Vec<PathBuf> = if path.is_dir() {
        std::fs::read_dir(path)
            .map_err(|e| format!("cannot list {}: {e}", path.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .collect()
    } else {
        vec![path.to_path_buf()]
    };
    files.sort();
    let mut out: Vec<Trajectory> = Vec::new();
    for f in files {
        let text =
            std::fs::read_to_string(&f).map_err(|e| format!("cannot read {}: {e}", f.display()))?;
        for (n, line) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let v: Value = serde_json::from_str(line)
                .map_err(|e| format!("{}:{}: {e}", f.display(), n + 1))?;
            if let Some(seed) = v.get("seed").and_then(Value::as_u64) {
                out.push(Trajectory {
                    seed,
                    states: Vec::new(),
                });
            } else if let Some(s) = v.get("s") {
                let s: Vec<f64> = serde_json::from_value(s.clone())
                    .map_err(|e| format!("{}:{}: {e}", f.display(), n + 1))?;
                out.last_mut()
                    .ok_or_else(|| {
                        format!("{}:{}: state before any episode header", f.display(), n + 1)
                    })?
                    .states
                    .push(s);
            }
        }
    }
    Ok(out)
}
