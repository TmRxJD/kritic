//! Policy runner and trajectory diagnostics against a synthetic toy client. The toy has no
//! meaning: four numbers, two action parameters, made-up dynamics.
//!   cargo test --test policy

use std::collections::BTreeMap;
use std::time::Duration;

use kritic::kev::{Answer, Decider, Decision, Question};
use kritic::policy::diagnostics::{Trajectory, diagnose, oscillation_spans, stuck_spans};
use kritic::policy::policies::{AnyPolicy, DeferPolicy, KevPolicy, Policy, PolicySpec};
use kritic::policy::protocol::{Msg, read_msg, write_msg};
use kritic::policy::runner::{LoopMode, RunOptions, Sweep, read_recordings, run_sweep};
use kritic::policy::schema::GameSchema;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};

const TOY: &str = r#"{
  "name": "toy",
  "version": "1",
  "tick_hz": 60,
  "state": {"fields": [
    {"name": "x", "dtype": "f32", "count": 2, "min": -10, "max": 10},
    {"name": "p", "dtype": "f32", "min": 0, "max": 100},
    {"name": "k", "dtype": "u16", "min": 0, "max": 1000}
  ]},
  "action": {"heads": [
    {"name": "h0", "values": 3},
    {"name": "h1", "values": 2}
  ]},
  "thresholds": {
    "progress_field": "p",
    "success_at": 50,
    "watch_fields": ["x[0]", "x[1]", "p"],
    "stuck_ticks": 10,
    "stuck_epsilon": 0.001,
    "oscillation_window": 10,
    "oscillation_flips": 4,
    "oscillation_amplitude": 0.04,
    "unrecoverable": [{"field": "x[1]", "below": -5, "for_ticks": 5}],
    "cluster_radius": 0.1
  }
}"#;

fn toy() -> GameSchema {
    GameSchema::parse(TOY).expect("toy schema")
}

/// The toy client. h0 shifts x[0] by h0-1; h1=1 raises p by 2; h0=0 with h1=0 lowers x[1]
/// (floored at -10); k counts ticks. Terminal once p >= 60. An empty action (defer) acts as
/// the client's own policy, which is [1, 0].
async fn toy_client<S: AsyncRead + AsyncWrite + Unpin>(io: S, schema: GameSchema) {
    paced_client(io, schema, None).await
}

/// The toy client, optionally sleeping `pace` per tick like a client with a native tick rate.
async fn paced_client<S: AsyncRead + AsyncWrite + Unpin>(
    mut io: S,
    schema: GameSchema,
    pace: Option<Duration>,
) {
    let hello = json!({"schema": "toy", "version": "1", "state_bytes": schema.state_bytes()});
    write_msg(&mut io, &Msg::Hello(hello)).await.unwrap();
    let mut s = [0.0f64; 4];
    let mut tick = 0u64;
    loop {
        match read_msg(&mut io).await {
            Ok(Msg::Reset { seed, .. }) => {
                s = [(seed % 3) as f64 * 0.1, 0.0, 0.0, 0.0];
                tick = 0;
            }
            Ok(Msg::Action { action, .. }) => {
                let a = if action.is_empty() {
                    vec![1, 0]
                } else {
                    schema.decode_action(&action).unwrap()
                };
                s[0] += a[0] as f64 - 1.0;
                if a[1] == 1 {
                    s[2] += 2.0;
                }
                if a[0] == 0 && a[1] == 0 {
                    s[1] = (s[1] - 1.0).max(-10.0);
                }
                s[3] += 1.0;
                tick += 1;
            }
            _ => return,
        }
        if let Some(d) = pace {
            tokio::time::sleep(d).await;
        }
        let state = schema.encode_state(&s).unwrap();
        let m = Msg::State {
            tick,
            terminal: s[2] >= 60.0,
            state,
        };
        if write_msg(&mut io, &m).await.is_err() {
            return;
        }
    }
}

fn opts(seeds: &[u64], max_ticks: u32) -> RunOptions {
    RunOptions {
        seeds: seeds.to_vec(),
        max_ticks,
        rules: "toy rules".into(),
        mode: LoopMode::Lockstep,
        frame_timeout: Duration::from_secs(5),
        record_dir: None,
    }
}

async fn sweep_with<P: Policy>(policy: &mut P, o: &RunOptions) -> Sweep {
    let schema = toy();
    let (a, mut b) = tokio::io::duplex(1 << 16);
    let client = tokio::spawn(toy_client(a, schema.clone()));
    let s = run_sweep(&mut b, &schema, policy, o).await.expect("sweep");
    client.await.unwrap();
    s
}

async fn sweep_spec(spec: PolicySpec, o: &RunOptions) -> Sweep {
    let schema = toy();
    let mut p: AnyPolicy<'_, Fake> = AnyPolicy::build(&spec, &schema, None).unwrap();
    sweep_with(&mut p, o).await
}

fn trajs(s: &Sweep) -> Vec<Trajectory> {
    s.episodes.iter().map(|e| e.trajectory()).collect()
}

/// A stand-in Decider: always picks option "1", after an optional delay.
struct Fake {
    delay: Duration,
    calls: std::sync::atomic::AtomicU64,
}

impl Fake {
    fn new(ms: u64) -> Self {
        Fake {
            delay: Duration::from_millis(ms),
            calls: Default::default(),
        }
    }
}

impl Decider for Fake {
    async fn decide(
        &self,
        _s: &Value,
        qs: &BTreeMap<String, Question>,
    ) -> Result<Decision, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        let mut d = Decision::default();
        for id in qs.keys() {
            d.answers.insert(
                id.clone(),
                Answer {
                    value: 0.9,
                    choice: Some("1".into()),
                    probabilities: None,
                    confidence: 0.8,
                },
            );
        }
        Ok(d)
    }
}

// ---- schema parsing ----

#[test]
fn schema_parses_and_buffers_round_trip() {
    let s = toy();
    assert_eq!(s.dims(), 4);
    assert_eq!(s.state_bytes(), 2 * 4 + 4 + 2);
    assert_eq!(s.dim_labels(), ["x[0]", "x[1]", "p", "k"]);
    assert_eq!(s.index_of("x[1]").unwrap(), 1);
    assert_eq!(s.index_of("k").unwrap(), 3);
    assert!(s.index_of("x").is_err(), "array field needs an index");
    let v = [1.5, -2.25, 40.0, 7.0];
    assert_eq!(s.decode_state(&s.encode_state(&v).unwrap()).unwrap(), v);
    assert!(s.decode_state(&[0u8; 3]).is_err());
    assert_eq!(s.encode_action(&[2, 1]).unwrap(), vec![2, 1]);
    assert!(s.encode_action(&[3, 0]).is_err());
    assert!((s.tick_budget_us() - 16_666.67).abs() < 0.1);
}

#[test]
fn mixed_radix_and_big_endian_round_trip() {
    let mut v: Value = serde_json::from_str(TOY).unwrap();
    v["action"]["packing"] = json!("mixed_radix_u32");
    v["state"]["endian"] = json!("big");
    let s = GameSchema::parse(&v.to_string()).unwrap();
    assert_eq!(s.action_bytes(), 4);
    for a in [[0, 0], [2, 1], [1, 1], [2, 0]] {
        let raw = s.encode_action(&a).unwrap();
        assert_eq!(s.decode_action(&raw).unwrap(), a);
    }
    assert_eq!(s.encode_action(&[2, 1]).unwrap(), (2u32 + 3).to_le_bytes());
    let raw = s.encode_state(&[1.0, 2.0, 3.0, 4.0]).unwrap();
    assert_eq!(&raw[12..], &[0, 4], "u16 big-endian");
}

#[test]
fn invalid_schema_reports_every_problem() {
    let mut v: Value = serde_json::from_str(TOY).unwrap();
    v["thresholds"]["progress_field"] = json!("nope");
    v["state"]["fields"][1]["name"] = json!("x");
    v["action"]["heads"][1]["values"] = json!(1);
    v["tick_hz"] = json!(0);
    let errs = GameSchema::parse(&v.to_string()).unwrap_err();
    let all = errs.join("\n");
    for needle in [
        "unknown state field `nope`",
        "duplicate state field `x`",
        "at least 2 values",
        "tick_hz",
    ] {
        assert!(all.contains(needle), "missing `{needle}` in:\n{all}");
    }
    assert!(GameSchema::parse("{").is_err());
}

#[test]
fn check_schema_describes_the_format_without_input() {
    let c = kritic::policy::check_schema(None, None);
    assert!(c.ok);
    let d = c.definition_schema.unwrap().to_string();
    assert!(d.contains("progress_field") && d.contains("mixed_radix_u32"));
    let c = kritic::policy::check_schema(None, Some(TOY));
    assert!(c.ok && c.state_bytes == 14 && c.action_bytes == 2);
}

// ---- protocol + deterministic loop ----

#[tokio::test]
async fn frames_round_trip() {
    let msgs = [
        Msg::Hello(json!({"schema": "toy"})),
        Msg::Reset {
            seed: 9,
            max_ticks: 77,
        },
        Msg::State {
            tick: 3,
            terminal: true,
            state: vec![1, 2, 3],
        },
        Msg::Action {
            tick: 3,
            action: vec![],
        },
        Msg::Action {
            tick: 4,
            action: vec![2, 1],
        },
        Msg::Bye,
        Msg::Error("x".into()),
    ];
    let (mut a, mut b) = tokio::io::duplex(4096);
    for m in &msgs {
        write_msg(&mut a, m).await.unwrap();
    }
    for m in &msgs {
        assert_eq!(&read_msg(&mut b).await.unwrap(), m);
    }
}

#[tokio::test]
async fn lockstep_loop_is_deterministic() {
    let spec = PolicySpec::Random;
    let a = sweep_spec(spec.clone(), &opts(&[0, 1, 2], 50)).await;
    let b = sweep_spec(spec, &opts(&[0, 1, 2], 50)).await;
    let da: Vec<_> = a.episodes.iter().map(|e| e.digest.clone()).collect();
    let db: Vec<_> = b.episodes.iter().map(|e| e.digest.clone()).collect();
    assert_eq!(da, db, "same seeds, same digests");
    assert_ne!(da[0], da[1], "different seeds differ");
    assert!(a.timing.reproducible);
    assert_eq!(
        a.timing.ticks,
        a.episodes
            .iter()
            .map(|e| e.frames.len() as u64)
            .sum::<u64>()
    );
}

#[tokio::test]
async fn episodes_end_on_terminal_or_tick_limit() {
    let s = sweep_spec(
        PolicySpec::Constant { action: vec![1, 1] },
        &opts(&[0], 100),
    )
    .await;
    let e = &s.episodes[0];
    assert!(e.terminal);
    assert_eq!(e.frames.len(), 31, "p reaches 60 after 30 actions");
    assert_eq!(e.frames.last().unwrap().action, None);
    assert_eq!(e.frames[0].action, Some(vec![1, 1]));
    let s = sweep_spec(PolicySpec::Constant { action: vec![1, 0] }, &opts(&[0], 40)).await;
    assert!(!s.episodes[0].terminal);
    assert_eq!(s.episodes[0].frames.len(), 40);
}

#[tokio::test]
async fn defer_lets_the_client_policy_act() {
    let s = sweep_with(&mut DeferPolicy, &opts(&[0], 12)).await;
    let e = &s.episodes[0];
    assert!(e.frames.iter().all(|f| f.action.is_none()));
    assert_eq!(
        e.frames.last().unwrap().state[3],
        11.0,
        "client stepped every tick"
    );
}

#[tokio::test]
async fn kev_policy_decides_at_low_frequency_and_holds() {
    let fake = Fake::new(0);
    let mut p = KevPolicy::new(&fake, 5, None);
    let s = sweep_with(&mut p, &opts(&[0], 21)).await;
    assert_eq!(
        fake.calls.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "ticks 0,5,10,15"
    );
    assert!(
        s.episodes[0].frames[..20]
            .iter()
            .all(|f| f.action == Some(vec![1, 1]))
    );
    assert_eq!(s.policy_stats["decisions"], 4);
}

#[tokio::test]
async fn hold_mode_answers_at_once_and_reports_action_age() {
    let fake = Fake::new(30);
    let mut p = KevPolicy::new(&fake, 1, None);
    let mut o = opts(&[0], 100);
    o.mode = LoopMode::Hold;
    let schema = toy();
    let (a, mut b) = tokio::io::duplex(1 << 16);
    let client = tokio::spawn(paced_client(
        a,
        schema.clone(),
        Some(Duration::from_millis(2)),
    ));
    let s = run_sweep(&mut b, &schema, &mut p, &o).await.expect("sweep");
    client.await.unwrap();
    let f = &s.episodes[0].frames;
    assert_eq!(f[0].action, None, "nothing decided yet: defer");
    assert!(s.timing.held_ticks > 0);
    assert!(s.timing.mean_action_age_ticks > 0.0);
    assert!(!s.timing.reproducible);
    assert!(
        (fake.calls.load(std::sync::atomic::Ordering::SeqCst) as usize) < f.len(),
        "evaluations ran slower than the client ticks"
    );
}

#[tokio::test]
async fn telemetry_is_recorded_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(&[4, 5], 20);
    o.record_dir = Some(dir.path().join("rec"));
    let s = sweep_spec(PolicySpec::Constant { action: vec![2, 1] }, &o).await;
    let back = read_recordings(&dir.path().join("rec")).unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].seed, 4);
    // JSON keeps each value to within one ulp (serde_json parses without float_roundtrip).
    let want = s.episodes[0].trajectory().states;
    assert_eq!(back[0].states.len(), want.len());
    for (x, y) in back[0].states.iter().flatten().zip(want.iter().flatten()) {
        assert!((x - y).abs() <= 1e-12 * y.abs().max(1.0), "{x} vs {y}");
    }
}

#[tokio::test]
async fn schema_mismatch_is_refused() {
    let mut v: Value = serde_json::from_str(TOY).unwrap();
    v["name"] = json!("other");
    let schema = GameSchema::parse(&v.to_string()).unwrap();
    let (a, mut b) = tokio::io::duplex(1 << 16);
    tokio::spawn(toy_client(a, toy()));
    let err = run_sweep(&mut b, &schema, &mut DeferPolicy, &opts(&[0], 5))
        .await
        .err()
        .unwrap();
    assert!(err.contains("schema `toy`"), "{err}");
}

#[tokio::test]
async fn only_loopback_addresses_are_accepted() {
    assert!(kritic::policy::loopback_addr("127.0.0.1:9").await.is_ok());
    assert!(kritic::policy::loopback_addr("8.8.8.8:80").await.is_err());
}

// ---- diagnostics ----

fn line(n: usize, f: impl Fn(usize) -> [f64; 4]) -> Trajectory {
    Trajectory {
        seed: 0,
        states: (0..n).map(|t| f(t).to_vec()).collect(),
    }
}

#[test]
fn stuck_spans_need_a_flat_stretch() {
    let s = toy();
    let rs = kritic::policy::diagnostics::ranges(&s, &[]);
    let t = line(40, |t| {
        if (10..30).contains(&t) {
            [1.0, 0.0, 5.0, t as f64]
        } else {
            [t as f64 * 0.3 - 6.0, 0.0, 5.0, t as f64]
        }
    });
    let dims = [0, 1, 2];
    let spans = stuck_spans(&t, &dims, &rs, 10, 0.001);
    assert_eq!(spans.len(), 1);
    assert_eq!((spans[0].start, spans[0].end), (10, 30));
    assert!(
        stuck_spans(&t, &[3], &rs, 10, 0.001).is_empty(),
        "a moving counter is never stuck"
    );
}

#[test]
fn oscillation_needs_repeated_large_reversals() {
    let s = toy();
    let rs = kritic::policy::diagnostics::ranges(&s, &[]);
    let labels = s.dim_labels();
    let osc = line(30, |t| [if t % 2 == 0 { 5.0 } else { -5.0 }, 0.0, 0.0, 0.0]);
    let spans = oscillation_spans(&osc, &[0], &labels, &rs, 10, 4, 0.04);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].field, "x[0]");
    assert!(spans[0].reversals >= 20);
    let tiny = line(30, |t| [if t % 2 == 0 { 0.1 } else { -0.1 }, 0.0, 0.0, 0.0]);
    assert!(
        oscillation_spans(&tiny, &[0], &labels, &rs, 10, 4, 0.04).is_empty(),
        "below amplitude"
    );
    let ramp = line(30, |t| [t as f64 * 0.5 - 7.0, 0.0, 0.0, 0.0]);
    assert!(oscillation_spans(&ramp, &[0], &labels, &rs, 10, 4, 0.04).is_empty());
}

#[tokio::test]
async fn families_are_clustered_and_dead_ones_vetoed() {
    let o = opts(&[0, 1, 2], 60);
    let good = sweep_spec(PolicySpec::Constant { action: vec![1, 1] }, &o).await;
    let idle = sweep_spec(PolicySpec::Constant { action: vec![1, 0] }, &o).await;
    let flip = sweep_spec(
        PolicySpec::Cycle {
            actions: vec![vec![0, 0], vec![2, 0]],
            hold_ticks: 1,
        },
        &o,
    )
    .await;
    let all: Vec<Trajectory> = [trajs(&good), trajs(&idle), trajs(&flip)].concat();
    let d = diagnose(&toy(), &all).unwrap();

    assert_eq!(
        d.families.len(),
        3,
        "{:#?}",
        d.families.iter().map(|f| &f.seeds).collect::<Vec<_>>()
    );
    let fam_of = |i: usize| d.episodes[i].family;
    assert!(fam_of(0) == fam_of(1) && fam_of(1) == fam_of(2));
    assert!(fam_of(3) == fam_of(4) && fam_of(4) == fam_of(5));
    assert!(fam_of(6) == fam_of(7) && fam_of(7) == fam_of(8));
    assert_eq!(d.families[fam_of(0)].success_rate, 1.0);

    assert_eq!(d.approval, Some(false));
    assert!(d.veto.rejected);
    assert_eq!(d.veto.families.len(), 2);
    let idle_v = d
        .veto
        .families
        .iter()
        .find(|v| v.family == fam_of(3))
        .unwrap();
    assert_eq!(idle_v.reason, "no_progress");
    assert!(
        idle_v
            .issues
            .iter()
            .any(|i| i.starts_with("stuck in 3 of 3"))
    );
    assert_eq!(idle_v.coordinates.len(), idle_v.coordinate_fields.len());
    assert_eq!(idle_v.coordinate_fields[0], "x[0].mean");
    let flip_v = d
        .veto
        .families
        .iter()
        .find(|v| v.family == fam_of(6))
        .unwrap();
    assert!(
        flip_v
            .issues
            .iter()
            .any(|i| i.starts_with("oscillating in 3 of 3"))
    );
    assert!(
        flip_v
            .issues
            .iter()
            .any(|i| i.starts_with("unrecoverable metric in 3 of 3"))
    );
    assert!(
        d.episodes[6]
            .unrecoverable
            .iter()
            .any(|u| u.field == "x[1]")
    );

    let ok = diagnose(&toy(), &trajs(&good)).unwrap();
    assert_eq!(ok.approval, Some(true));
    assert!(!ok.veto.rejected);
}

#[test]
fn no_success_is_distinguished_from_no_progress() {
    let t = line(40, |t| [0.0, 0.0, (t as f64).min(20.0), t as f64]);
    let d = diagnose(&toy(), &[t]).unwrap();
    assert_eq!(d.veto.families[0].reason, "no_success");
    assert!(diagnose(&toy(), &[]).unwrap().approval.is_none());
}

#[test]
fn clustering_is_order_stable_and_radius_bound() {
    use kritic::policy::diagnostics::cluster;
    let pts = vec![
        vec![0.0, 0.0],
        vec![0.05, 0.0],
        vec![1.0, 1.0],
        vec![0.02, 0.01],
        vec![0.98, 1.0],
    ];
    assert_eq!(cluster(&pts, 0.1), vec![0, 0, 1, 0, 1]);
    assert_eq!(cluster(&pts, 2.0), vec![0, 0, 0, 0, 0]);
}
