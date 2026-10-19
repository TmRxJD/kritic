//! Game-agnostic real-time policy evaluation and trajectory diagnostics.
//! The game supplies a schema (state layout, action space, thresholds) and a client that speaks
//! the frame protocol; kritic supplies orchestration, timing, recording and diagnostics.
//! Everything runs locally. See docs/policy-runner.md.

pub mod diagnostics;
pub mod policies;
pub mod protocol;
pub mod runner;
pub mod schema;

use std::process::Stdio;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kev::Decider;
use diagnostics::{Diagnostics, diagnose};
use policies::{AnyPolicy, PolicySpec};
use runner::{LoopMode, RunOptions, Timing, run_sweep};
use schema::GameSchema;

/// Where the game client is.
#[derive(Debug, Clone)]
pub enum Transport {
    /// Connect to a client listening on a loopback TCP address.
    Connect(String),
    /// Spawn the client and speak the protocol over its stdin/stdout.
    Command {
        argv: Vec<String>,
        cwd: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EpisodeSummary {
    pub seed: u64,
    pub ticks: usize,
    pub terminal: bool,
    /// Hash of states and actions; equal across lockstep runs of a deterministic client+policy.
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RunReport {
    /// false when the veto rejected a family or the run failed; null when nothing ran.
    pub approval: Option<bool>,
    pub schema: String,
    pub policy: String,
    pub policy_stats: Value,
    pub timing: Timing,
    pub episodes: Vec<EpisodeSummary>,
    pub diagnostics: Option<Diagnostics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_dir: Option<String>,
    /// Feasibility limits and other facts the caller should know.
    pub notes: Vec<String>,
}

/// Load a schema from a file path or inline JSON text.
pub fn load_schema(path: Option<&str>, content: Option<&str>) -> Result<GameSchema, String> {
    let text = match (path, content) {
        (Some(p), _) => crate::files::read_lossy(std::path::Path::new(p))
            .map_err(|e| format!("cannot read {p}: {e}"))?,
        (None, Some(c)) => c.to_string(),
        (None, None) => return Err("give schema_path or schema".into()),
    };
    GameSchema::parse(&text).map_err(|e| format!("invalid schema: {}", e.join("; ")))
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct SchemaCheck {
    pub ok: bool,
    pub errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub tick_hz: f64,
    pub tick_budget_ms: f64,
    pub state_dims: usize,
    pub state_bytes: usize,
    /// Flattened dimension labels, in buffer order.
    pub fields: Vec<String>,
    /// (head, number of values)
    pub action_heads: Vec<(String, u32)>,
    pub action_bytes: usize,
    /// JSON Schema of the definition format; returned when no schema was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition_schema: Option<Value>,
}

/// Parse and validate a schema; with no input, describe the format instead.
pub fn check_schema(path: Option<&str>, content: Option<&str>) -> SchemaCheck {
    let text = match (path, content) {
        (Some(p), _) => match crate::files::read_lossy(std::path::Path::new(p)) {
            Ok(t) => t,
            Err(e) => {
                return SchemaCheck {
                    errors: vec![format!("cannot read {p}: {e}")],
                    ..Default::default()
                };
            }
        },
        (None, Some(c)) => c.to_string(),
        (None, None) => {
            return SchemaCheck {
                ok: true,
                definition_schema: Some(schema::meta_schema()),
                ..Default::default()
            };
        }
    };
    match GameSchema::parse(&text) {
        Err(errors) => SchemaCheck {
            errors,
            ..Default::default()
        },
        Ok(s) => SchemaCheck {
            ok: true,
            errors: Vec::new(),
            name: Some(s.name.clone()),
            tick_hz: s.tick_hz,
            tick_budget_ms: s.tick_budget_us() / 1000.0,
            state_dims: s.dims(),
            state_bytes: s.state_bytes(),
            fields: s.dim_labels(),
            action_heads: s
                .action
                .heads
                .iter()
                .map(|h| (h.name.clone(), h.values))
                .collect(),
            action_bytes: s.action_bytes(),
            definition_schema: None,
        },
    }
}

/// Resolve `addr` and refuse anything that is not loopback: the runner never leaves the host.
pub async fn loopback_addr(addr: &str) -> Result<std::net::SocketAddr, String> {
    let all: Vec<_> = tokio::net::lookup_host(addr)
        .await
        .map_err(|e| format!("cannot resolve `{addr}`: {e}"))?
        .collect();
    let first = *all
        .first()
        .ok_or_else(|| format!("`{addr}` resolves to nothing"))?;
    if all.iter().any(|a| !a.ip().is_loopback()) {
        return Err(format!(
            "`{addr}` is not a loopback address; the runner only talks to local clients"
        ));
    }
    Ok(first)
}

/// Run a sweep against a client, then diagnose the recorded trajectories.
pub async fn run<D: Decider>(
    schema: &GameSchema,
    spec: &PolicySpec,
    kev: Option<&D>,
    transport: &Transport,
    opts: &RunOptions,
) -> Result<RunReport, String> {
    let mut policy = AnyPolicy::build(spec, schema, kev)?;
    let sweep = match transport {
        Transport::Connect(addr) => {
            let sa = loopback_addr(addr).await?;
            let mut s =
                tokio::time::timeout(opts.frame_timeout, tokio::net::TcpStream::connect(sa))
                    .await
                    .map_err(|_| format!("connect to {sa} timed out"))?
                    .map_err(|e| format!("cannot connect to {sa}: {e}"))?;
            let _ = s.set_nodelay(true);
            run_sweep(&mut s, schema, &mut policy, opts).await?
        }
        Transport::Command { argv, cwd } => {
            let (exe, args) = argv.split_first().ok_or("command is empty")?;
            let mut cmd = tokio::process::Command::new(exe);
            cmd.args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            if let Some(d) = cwd {
                cmd.current_dir(d);
            }
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let mut child = cmd
                .spawn()
                .map_err(|e| format!("cannot start client `{exe}`: {e}"))?;
            let out = child.stdout.take().ok_or("no stdout")?;
            let inp = child.stdin.take().ok_or("no stdin")?;
            let mut io = tokio::io::join(out, inp);
            let r = run_sweep(&mut io, schema, &mut policy, opts).await;
            drop(io);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
            r?
        }
    };
    Ok(report(schema, spec, opts, sweep))
}

/// Turn a finished sweep into the report, diagnostics and feasibility notes.
pub fn report(
    schema: &GameSchema,
    spec: &PolicySpec,
    opts: &RunOptions,
    sweep: runner::Sweep,
) -> RunReport {
    let trajs: Vec<_> = sweep.episodes.iter().map(|e| e.trajectory()).collect();
    let mut notes = Vec::new();
    let t = &sweep.timing;
    let ms = |us: u64| us as f64 / 1000.0;
    if !t.realtime_feasible && t.evaluations > 0 {
        let msg = match opts.mode {
            LoopMode::Lockstep => format!(
                "policy p95 {:.1} ms exceeds the {:.1} ms tick budget at {} Hz: the lockstep loop ran at {} Hz of game time per second; use mode=hold to keep the native rate with held actions, or a faster policy",
                ms(t.policy_us_p95),
                t.budget_us / 1000.0,
                schema.tick_hz,
                t.achieved_hz
            ),
            LoopMode::Hold => format!(
                "policy p95 {:.1} ms exceeds the {:.1} ms tick budget: actions were applied {:.1} ticks after the state they were decided on, on average",
                ms(t.policy_us_p95),
                t.budget_us / 1000.0,
                t.mean_action_age_ticks
            ),
        };
        notes.push(msg);
    }
    if spec.is_kev() {
        notes.push(
            "Kev is a typed-decision classifier, not a trained control policy: its choices here are untrained guesses over the given labels, useful for smoke runs and low-rate probing, not as a competent agent. Measured on an RTX 2070: ~0.1 s per warm decision (about 8 Hz)."
                .into(),
        );
    }
    if opts.mode == LoopMode::Hold {
        notes.push("hold mode depends on wall-clock timing: digests are not reproducible".into());
    }
    let diagnostics = match diagnose(schema, &trajs) {
        Ok(d) => Some(d),
        Err(e) => {
            notes.push(format!("diagnostics failed: {e}"));
            None
        }
    };
    RunReport {
        approval: diagnostics.as_ref().and_then(|d| d.approval),
        schema: schema.name.clone(),
        policy: sweep.policy,
        policy_stats: sweep.policy_stats,
        timing: sweep.timing,
        episodes: sweep
            .episodes
            .iter()
            .map(|e| EpisodeSummary {
                seed: e.seed,
                ticks: e.frames.len(),
                terminal: e.terminal,
                digest: e.digest.clone(),
            })
            .collect(),
        diagnostics,
        record_dir: opts.record_dir.as_ref().map(|p| p.display().to_string()),
        notes,
    }
}
