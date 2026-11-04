//! Pluggable policy evaluators: `state_t` (decoded) -> `action_t` (one value per head).
//! Defer (the game's own policy), scripted, an external local process, or Kev at low frequency.

use std::collections::BTreeMap;
use std::future::Future;
use std::process::Stdio;
use std::time::Instant;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::protocol::{Msg, read_msg, write_msg};
use super::schema::GameSchema;
use crate::kev::{Decider, Question};

/// What the policy sees besides the decoded state.
pub struct PolicyContext<'a> {
    pub schema: &'a GameSchema,
    /// Game-provided context rules, passed through opaquely.
    pub rules: &'a str,
    pub seed: u64,
    pub tick: u64,
    /// The raw `state_t` bytes as received.
    pub raw: &'a [u8],
}

#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Action(Vec<u32>),
    /// Let the game's own policy act this tick.
    Defer,
}

pub trait Policy: Send {
    fn name(&self) -> String;
    fn begin(&mut self, seed: u64) -> impl Future<Output = Result<(), String>> + Send;
    fn act(
        &mut self,
        state: &[f64],
        ctx: &PolicyContext<'_>,
    ) -> impl Future<Output = Result<Act, String>> + Send;
    fn end(&mut self) -> impl Future<Output = ()> + Send {
        async {}
    }
    /// Policy-specific counters for the run report.
    fn stats(&self) -> Value {
        Value::Null
    }
}

/// A condition on one state field and the action to take when it holds.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RuleSpec {
    /// Field reference: `name` or `name[i]`.
    pub field: String,
    #[serde(default)]
    pub below: Option<f64>,
    #[serde(default)]
    pub above: Option<f64>,
    pub action: Vec<u32>,
}

fn one_tick() -> u64 {
    1
}
fn d_every() -> u64 {
    30
}

/// Which evaluator to plug in.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicySpec {
    /// Send empty actions: the game's own policy acts; kritic only records.
    Defer,
    /// The same action every tick.
    Constant { action: Vec<u32> },
    /// Step through `actions`, holding each for `hold_ticks`.
    Cycle {
        actions: Vec<Vec<u32>>,
        #[serde(default = "one_tick")]
        hold_ticks: u64,
    },
    /// First matching rule wins, else `default`.
    Rules {
        rules: Vec<RuleSpec>,
        default: Vec<u32>,
    },
    /// Uniform random values from a generator seeded by the episode seed (reproducible).
    Random,
    /// An external local process speaking the frame protocol on stdin/stdout.
    Process {
        command: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
    },
    /// Kev (laya) picks each head's value every `every_ticks` ticks and the action is held in
    /// between. ~0.1 s per decision: not for per-tick control at native rates.
    Kev {
        #[serde(default = "d_every")]
        every_ticks: u64,
        #[serde(default)]
        instructions: Option<String>,
    },
}

impl PolicySpec {
    pub fn is_kev(&self) -> bool {
        matches!(self, PolicySpec::Kev { .. })
    }
}

/// Deterministic local policies: constant, cycle, rules, seeded random.
pub struct ScriptedPolicy {
    spec: PolicySpec,
    rules: Vec<(usize, RuleSpec)>,
    rng: u64,
}

impl ScriptedPolicy {
    pub fn new(spec: PolicySpec, schema: &GameSchema) -> Result<Self, String> {
        let check = |a: &[u32]| schema.encode_action(a).map(|_| ());
        let mut rules = Vec::new();
        match &spec {
            PolicySpec::Constant { action } => check(action)?,
            PolicySpec::Cycle {
                actions,
                hold_ticks,
            } => {
                if actions.is_empty() || *hold_ticks == 0 {
                    return Err("cycle needs actions and hold_ticks >= 1".into());
                }
                actions.iter().try_for_each(|a| check(a))?;
            }
            PolicySpec::Rules { rules: rs, default } => {
                check(default)?;
                for r in rs {
                    check(&r.action)?;
                    rules.push((schema.index_of(&r.field)?, r.clone()));
                }
            }
            PolicySpec::Random => {}
            _ => return Err("not a scripted policy".into()),
        }
        Ok(ScriptedPolicy {
            spec,
            rules,
            rng: 1,
        })
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn pick(&mut self, state: &[f64], ctx: &PolicyContext<'_>) -> Vec<u32> {
        match &self.spec {
            PolicySpec::Constant { action } => action.clone(),
            PolicySpec::Cycle {
                actions,
                hold_ticks,
            } => actions[((ctx.tick / hold_ticks) % actions.len() as u64) as usize].clone(),
            PolicySpec::Rules { default, .. } => self
                .rules
                .iter()
                .find(|(i, r)| {
                    let v = state[*i];
                    r.below.is_none_or(|b| v < b) && r.above.is_none_or(|a| v > a)
                })
                .map(|(_, r)| r.action.clone())
                .unwrap_or_else(|| default.clone()),
            PolicySpec::Random => {
                let heads: Vec<u32> = ctx.schema.action.heads.iter().map(|h| h.values).collect();
                heads
                    .into_iter()
                    .map(|n| (self.next_u64() % n as u64) as u32)
                    .collect()
            }
            _ => unreachable!("checked in new"),
        }
    }
}

impl Policy for ScriptedPolicy {
    fn name(&self) -> String {
        match &self.spec {
            PolicySpec::Constant { .. } => "constant",
            PolicySpec::Cycle { .. } => "cycle",
            PolicySpec::Rules { .. } => "rules",
            _ => "random",
        }
        .into()
    }

    async fn begin(&mut self, seed: u64) -> Result<(), String> {
        self.rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        Ok(())
    }

    async fn act(&mut self, state: &[f64], ctx: &PolicyContext<'_>) -> Result<Act, String> {
        Ok(Act::Action(self.pick(state, ctx)))
    }
}

/// The game's own policy acts; the runner only records.
pub struct DeferPolicy;

impl Policy for DeferPolicy {
    fn name(&self) -> String {
        "defer".into()
    }
    async fn begin(&mut self, _seed: u64) -> Result<(), String> {
        Ok(())
    }
    async fn act(&mut self, _state: &[f64], _ctx: &PolicyContext<'_>) -> Result<Act, String> {
        Ok(Act::Defer)
    }
}

/// An external local policy process: gets RESET / STATE frames on stdin, answers ACTION frames
/// on stdout, gets BYE at the end.
pub struct ProcessPolicy {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    schema: GameSchema,
    label: String,
}

impl ProcessPolicy {
    pub fn spawn(
        command: &[String],
        cwd: Option<&str>,
        schema: &GameSchema,
    ) -> Result<Self, String> {
        let (exe, args) = command
            .split_first()
            .ok_or("process policy needs a command")?;
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
            .map_err(|e| format!("cannot start policy process `{exe}`: {e}"))?;
        Ok(ProcessPolicy {
            stdin: child.stdin.take().ok_or("no stdin")?,
            stdout: child.stdout.take().ok_or("no stdout")?,
            child,
            schema: schema.clone(),
            label: format!("process:{exe}"),
        })
    }
}

impl Policy for ProcessPolicy {
    fn name(&self) -> String {
        self.label.clone()
    }

    async fn begin(&mut self, seed: u64) -> Result<(), String> {
        write_msg(&mut self.stdin, &Msg::Reset { seed, max_ticks: 0 }).await
    }

    async fn act(&mut self, _state: &[f64], ctx: &PolicyContext<'_>) -> Result<Act, String> {
        let m = Msg::State {
            tick: ctx.tick,
            terminal: false,
            state: ctx.raw.to_vec(),
        };
        write_msg(&mut self.stdin, &m).await?;
        match read_msg(&mut self.stdout).await? {
            Msg::Action { action, .. } if action.is_empty() => Ok(Act::Defer),
            Msg::Action { action, .. } => Ok(Act::Action(self.schema.decode_action(&action)?)),
            Msg::Error(e) => Err(format!("policy process: {e}")),
            other => Err(format!("policy process sent {other:?}, expected ACTION")),
        }
    }

    async fn end(&mut self) {
        let _ = write_msg(&mut self.stdin, &Msg::Bye).await;
        let _ = self.child.kill().await;
    }
}

/// Kev answers one `choice` question per action head every `every` ticks; the chosen action is
/// held in between. Measured latency is reported, never hidden.
pub struct KevPolicy<'a, D: Decider> {
    kev: &'a D,
    every: u64,
    instructions: String,
    last: Option<Vec<u32>>,
    decisions: u64,
    total_ms: u64,
    max_ms: u64,
    mean_confidence: f64,
}

impl<'a, D: Decider> KevPolicy<'a, D> {
    pub fn new(kev: &'a D, every_ticks: u64, instructions: Option<String>) -> Self {
        KevPolicy {
            kev,
            every: every_ticks.max(1),
            instructions: instructions.unwrap_or_else(|| {
                "The state is a vector of named numeric fields from a program, with the program's \
                 own rules. Choose the value for the named action parameter."
                    .into()
            }),
            last: None,
            decisions: 0,
            total_ms: 0,
            max_ms: 0,
            mean_confidence: 0.0,
        }
    }

    fn questions(&self, schema: &GameSchema) -> BTreeMap<String, Question> {
        schema
            .action
            .heads
            .iter()
            .map(|h| {
                let criteria: serde_json::Map<String, Value> = (0..h.values)
                    .map(|i| {
                        let label = h
                            .labels
                            .get(i as usize)
                            .cloned()
                            .unwrap_or_else(|| format!("value {i}"));
                        (i.to_string(), Value::String(label))
                    })
                    .collect();
                let q = Question {
                    kind: "choice",
                    instructions: format!("{} Parameter: `{}`.", self.instructions, h.name),
                    criteria: Some(Value::Object(criteria)),
                };
                (h.name.clone(), q)
            })
            .collect()
    }
}

impl<D: Decider> Policy for KevPolicy<'_, D> {
    fn name(&self) -> String {
        format!("kev(every {} ticks)", self.every)
    }

    async fn begin(&mut self, _seed: u64) -> Result<(), String> {
        self.last = None;
        Ok(())
    }

    async fn act(&mut self, state: &[f64], ctx: &PolicyContext<'_>) -> Result<Act, String> {
        if let Some(a) = &self.last
            && !ctx.tick.is_multiple_of(self.every)
        {
            return Ok(Act::Action(a.clone()));
        }
        let named: serde_json::Map<String, Value> = ctx
            .schema
            .dim_labels()
            .into_iter()
            .zip(state.iter().map(|v| json!(v)))
            .collect();
        let st = json!({"rules": ctx.rules, "tick": ctx.tick, "state": named});
        let qs = self.questions(ctx.schema);
        let t0 = Instant::now();
        let d = self.kev.decide(&st, &qs).await?;
        let ms = t0.elapsed().as_millis() as u64;
        let mut a = Vec::with_capacity(qs.len());
        let mut conf = 0.0;
        for h in &ctx.schema.action.heads {
            let ans = d
                .answers
                .get(&h.name)
                .ok_or_else(|| format!("kev did not answer `{}`", h.name))?;
            let v: u32 = ans
                .choice
                .as_deref()
                .and_then(|c| c.parse().ok())
                .filter(|v| *v < h.values)
                .ok_or_else(|| format!("kev gave no valid choice for `{}`", h.name))?;
            conf += ans.confidence;
            a.push(v);
        }
        self.decisions += 1;
        self.total_ms += ms;
        self.max_ms = self.max_ms.max(ms);
        let c = conf / ctx.schema.action.heads.len().max(1) as f64;
        self.mean_confidence += (c - self.mean_confidence) / self.decisions as f64;
        self.last = Some(a.clone());
        Ok(Act::Action(a))
    }

    fn stats(&self) -> Value {
        json!({
            "decisions": self.decisions,
            "every_ticks": self.every,
            "mean_decision_ms": self.total_ms.checked_div(self.decisions).unwrap_or(0),
            "max_decision_ms": self.max_ms,
            "mean_confidence": (self.mean_confidence * 1000.0).round() / 1000.0,
        })
    }
}

/// Every evaluator behind one type, for callers that pick at run time.
pub enum AnyPolicy<'a, D: Decider> {
    Defer(DeferPolicy),
    Scripted(ScriptedPolicy),
    Process(Box<ProcessPolicy>),
    Kev(KevPolicy<'a, D>),
}

impl<'a, D: Decider> AnyPolicy<'a, D> {
    pub fn build(
        spec: &PolicySpec,
        schema: &GameSchema,
        kev: Option<&'a D>,
    ) -> Result<Self, String> {
        Ok(match spec {
            PolicySpec::Defer => AnyPolicy::Defer(DeferPolicy),
            PolicySpec::Process { command, cwd } => AnyPolicy::Process(Box::new(
                ProcessPolicy::spawn(command, cwd.as_deref(), schema)?,
            )),
            PolicySpec::Kev {
                every_ticks,
                instructions,
            } => AnyPolicy::Kev(KevPolicy::new(
                kev.ok_or("kev policy needs a Kev client")?,
                *every_ticks,
                instructions.clone(),
            )),
            s => AnyPolicy::Scripted(ScriptedPolicy::new(s.clone(), schema)?),
        })
    }
}

impl<D: Decider> Policy for AnyPolicy<'_, D> {
    fn name(&self) -> String {
        match self {
            AnyPolicy::Defer(p) => p.name(),
            AnyPolicy::Scripted(p) => p.name(),
            AnyPolicy::Process(p) => p.name(),
            AnyPolicy::Kev(p) => p.name(),
        }
    }
    async fn begin(&mut self, seed: u64) -> Result<(), String> {
        match self {
            AnyPolicy::Defer(p) => p.begin(seed).await,
            AnyPolicy::Scripted(p) => p.begin(seed).await,
            AnyPolicy::Process(p) => p.begin(seed).await,
            AnyPolicy::Kev(p) => p.begin(seed).await,
        }
    }
    async fn act(&mut self, state: &[f64], ctx: &PolicyContext<'_>) -> Result<Act, String> {
        match self {
            AnyPolicy::Defer(p) => p.act(state, ctx).await,
            AnyPolicy::Scripted(p) => p.act(state, ctx).await,
            AnyPolicy::Process(p) => p.act(state, ctx).await,
            AnyPolicy::Kev(p) => p.act(state, ctx).await,
        }
    }
    async fn end(&mut self) {
        if let AnyPolicy::Process(p) = self {
            p.end().await
        }
    }
    fn stats(&self) -> Value {
        match self {
            AnyPolicy::Kev(p) => p.stats(),
            _ => Value::Null,
        }
    }
}
