//! Offline tests with a fake Decider. Live tests against laya: tests/live.rs (KRITIC_LIVE=1).

use std::collections::BTreeMap;
use std::sync::Mutex;

use kritic::checks::{Status, deterministic_checks, run_command};
use kritic::chunk::{chunk_text, digest};
use kritic::config::Config;
use kritic::critic::{CriticResult, critic, critic_diff, parse_diff};
use kritic::files::resolve_paths;
use kritic::kev::{Answer, Decider, Decision, Question, group_questions, parse_answer};
use kritic::map::{self, Note};
use kritic::spec;
use kritic::tools::{ValidateInput, artifact_validate, context_trim, git_diff, route};
use serde_json::{Value, json};

/// Answers by looking for markers in the state. `BUG` -> defect/major, `EVAL` -> security,
/// `TODO` -> unfinished, `REVIEWER` -> reviewer text, `HALF` -> incomplete. Also records every
/// state it was asked about, so tests can check what Kev is (and is not) shown.
struct Fake {
    offline: bool,
    calls: Mutex<usize>,
    states: Mutex<Vec<Value>>,
}

impl Fake {
    fn new() -> Self {
        Fake {
            offline: false,
            calls: Mutex::new(0),
            states: Mutex::new(vec![]),
        }
    }
    fn offline() -> Self {
        Fake {
            offline: true,
            ..Fake::new()
        }
    }
}

fn noul(v: f64) -> Answer {
    Answer {
        value: v,
        choice: None,
        probabilities: None,
        confidence: v.max(1.0 - v),
    }
}
fn choice(c: &str, p: f64) -> Answer {
    Answer {
        value: p,
        choice: Some(c.into()),
        probabilities: Some(BTreeMap::from([(c.to_string(), p)])),
        confidence: p,
    }
}

impl Decider for Fake {
    async fn decide(
        &self,
        state: &Value,
        qs: &BTreeMap<String, Question>,
    ) -> Result<Decision, String> {
        *self.calls.lock().unwrap() += 1;
        self.states.lock().unwrap().push(state.clone());
        if self.offline {
            return Err("laya unreachable".into());
        }
        let s = state.to_string();
        let has = |m: &str| s.contains(m);
        let mut answers = BTreeMap::new();
        for id in qs.keys() {
            let a = match id.as_str() {
                "defect" => noul(if has("BUG") { 0.9 } else { 0.05 }),
                "severity" => {
                    let (label, p) = if has("BUG") {
                        ("major", 0.8)
                    } else if has("SEVLABEL") {
                        ("critical", 0.3)
                    } else {
                        ("none", 0.8)
                    };
                    let mut probs =
                        BTreeMap::from([("none".to_string(), 1.0 - p), (label.to_string(), p)]);
                    if label == "critical" {
                        probs.insert("minor".into(), 0.45);
                        probs.insert("none".into(), 0.25);
                    }
                    Answer {
                        value: p,
                        choice: Some(label.into()),
                        probabilities: Some(probs),
                        confidence: p,
                    }
                }
                "security" => noul(if has("EVAL") { 0.9 } else { 0.05 }),
                "unfinished" => noul(if has("TODO") { 0.9 } else { 0.05 }),
                "reviewer_text" => noul(if has("REVIEWER") { 0.9 } else { 0.02 }),
                "quality" => Answer {
                    value: 3.0,
                    choice: None,
                    probabilities: None,
                    confidence: 0.6,
                },
                "complete" => noul(if has("HALF") { 0.1 } else { 0.9 }),
                "tier" => choice(if has("rename") { "haiku" } else { "opus" }, 0.7),
                "relevant" => noul(if has("apple") { 0.9 } else { 0.05 }),
                "awkward" => noul(if has("99.00") { 0.9 } else { 0.1 }),
                "real" | "hard" | "fits" => noul(0.5),
                other => return Err(format!("unexpected question {other}")),
            };
            answers.insert(id.clone(), a);
        }
        Ok(Decision {
            answers,
            truncated: false,
            latency_ms: 1,
            input_tokens: 100,
        })
    }
}

fn cfg() -> Config {
    let mut c = Config::from_env();
    c.log_path = std::env::temp_dir().join("kritic-test-calls.jsonl");
    c
}

async fn run(files: &[(&str, &str)], task: &str) -> CriticResult {
    let f: Vec<(String, String)> = files
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    critic(&Fake::new(), &cfg(), task, &f, 0).await
}

// ------------------------------------------------------------------ chunking

#[test]
fn chunks_respect_size_and_cover_all_lines() {
    let text: String = (1..=400).map(|i| format!("let x{i} = {i};\n")).collect();
    let cs = chunk_text("a.rs", &text, 1300);
    assert!(cs.len() > 2);
    assert!(cs.iter().all(|c| c.text.len() <= 1300));
    assert_eq!(cs.first().unwrap().start_line, 1);
    assert_eq!(cs.last().unwrap().end_line, 400);
    for w in cs.windows(2) {
        assert_eq!(w[1].start_line, w[0].end_line + 1, "no gaps or overlaps");
    }
}

#[test]
fn chunks_hard_split_long_lines() {
    let text = "a".repeat(3000);
    let cs = chunk_text("min.js", &text, 1000);
    assert_eq!(cs.len(), 3);
    assert!(cs.iter().all(|c| c.start_line == 1));
}

#[test]
fn digest_lists_definitions() {
    let files = vec![(
        "src/a.rs".to_string(),
        "pub fn alpha() {}\nlet y = 2;\nstruct Beta;\n".to_string(),
    )];
    let d = digest(&files, 2000);
    assert!(d.contains("FILE a.rs") && d.contains("pub fn alpha") && d.contains("struct Beta"));
    assert!(!d.contains("let y"));
}

// ------------------------------------------------------------------ deterministic checks

#[tokio::test]
async fn parsers() {
    let ok = |v: Vec<kritic::checks::Check>| v.iter().all(|c| c.status != Status::Fail);
    assert!(ok(deterministic_checks(r#"{"a":1}"#, "json").await));
    assert!(!ok(deterministic_checks(r#"{"a":1,}"#, "json").await));
    assert!(ok(deterministic_checks("a: [1, 2]", "yaml").await));
    assert!(!ok(deterministic_checks("a: [1", "yaml").await));
    assert!(!ok(deterministic_checks("a:\n\tb: 1", "yaml").await));
    assert!(ok(deterministic_checks("a = \"b\"", "toml").await));
    assert!(!ok(deterministic_checks("a = ", "toml").await));
    assert!(!ok(deterministic_checks(
        "<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\n",
        "code"
    )
    .await));
    assert!(!ok(deterministic_checks("bad \u{FFFD} byte", "text").await));
}

#[tokio::test]
async fn python_syntax_via_subprocess() {
    let good = deterministic_checks("def f():\n    return 1\n", "python").await;
    let bad = deterministic_checks("def f(:\n", "python").await;
    if good[0].status == Status::Skipped {
        eprintln!("python not on PATH; skipping");
        return;
    }
    assert_eq!(good[0].status, Status::Pass);
    assert_eq!(bad[0].status, Status::Fail);
}

#[tokio::test]
async fn commands_report_exit_and_output() {
    let dir = std::env::temp_dir();
    let ok = run_command("echo hello", &dir, 30).await;
    assert_eq!(ok.status, Status::Pass);
    assert!(ok.output_tail.contains("hello"));
    let bad = run_command("exit 3", &dir, 30).await;
    assert_eq!(bad.status, Status::Fail);
    assert_eq!(bad.exit_code, Some(3));
}

#[test]
fn path_filtering() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.py"), "x = 1\n").unwrap();
    std::fs::create_dir(d.path().join("node_modules")).unwrap();
    std::fs::write(d.path().join("node_modules/b.js"), "y").unwrap();
    std::fs::write(d.path().join("img.png"), [0u8, 1]).unwrap();
    let paths = vec![
        "a.py".into(),
        "node_modules/b.js".into(),
        "img.png".into(),
        "missing.ts".into(),
    ];
    let (ok, skipped) = resolve_paths(&paths, Some(d.path().to_str().unwrap()));
    assert_eq!(ok.len(), 1);
    let mut reasons: Vec<_> = skipped.iter().map(|s| s.reason.as_str()).collect();
    reasons.sort();
    assert_eq!(
        reasons,
        ["binary/lockfile", "in ignored directory", "not found"]
    );
}

// ------------------------------------------------------------------ kev plumbing

#[test]
fn question_groups_fit_budget() {
    let state = json!({"code": "x".repeat(1300)});
    let spec = spec::DecisionSpec {
        name: "t".into(),
        purpose: "budget test".into(),
        fields: vec![spec::code("code", 20_000, "Code for the budget test.")],
        questions: (0..6)
            .map(|i| {
                spec::noul(
                    &format!("q{i}"),
                    "Is `code` ok?",
                    "it is fine as written",
                    "it has a problem",
                    spec::advisory("test only, never gates"),
                )
            })
            .collect(),
        calibrated_for: vec!["kev-4b".into()],
    };
    let qs: BTreeMap<String, Question> = spec.wire();
    let groups = group_questions(&state, &qs, 2900);
    assert!(groups.len() >= 2, "6 x ~1000-token rows must be split");
    assert_eq!(groups.iter().map(|g| g.len()).sum::<usize>(), 6);
    // a single oversized row still gets its own request
    let huge = json!({"code": "x".repeat(20_000)});
    assert_eq!(group_questions(&huge, &qs, 2900).len(), 6);
}

#[test]
fn answers_are_validated_not_defaulted() {
    let t = |ask| {
        spec::DecisionSpec {
            name: "t".into(),
            purpose: "answer test".into(),
            fields: vec![spec::text("x", 10, "A field for the answer test.")],
            questions: vec![spec::QuestionSpec {
                id: "q".into(),
                ask,
                gate: spec::advisory("test only, never gates"),
            }],
            calibrated_for: vec!["kev-4b".into()],
        }
        .wire()
        .remove("q")
        .unwrap()
    };
    let n = t(spec::Ask::Noul {
        instructions: "Is `x` ok?".into(),
        yes: "it is fine".into(),
        no: "it is not fine".into(),
    });
    assert!(parse_answer("a", &n, &json!({"type":"noul","noul":0.4,"confidence":0.6})).is_ok());
    assert!(parse_answer("a", &n, &json!({"type":"noul","noul":1.4,"confidence":0.6})).is_err());
    assert!(
        parse_answer(
            "a",
            &n,
            &json!({"type":"choice","choice":"x","confidence":0.6})
        )
        .is_err()
    );
    assert!(parse_answer("a", &n, &json!({"type":"noul","confidence":0.6})).is_err());
    let c = t(spec::Ask::Choice {
        instructions: "Which is `x`?".into(),
        options: vec![
            ("yes".into(), "the first kind".into()),
            ("no".into(), "the second kind".into()),
        ],
    });
    assert!(
        parse_answer(
            "c",
            &c,
            &json!({"type":"choice","choice":"maybe","probabilities":{"maybe":1.0},"confidence":1})
        )
        .is_err()
    );
    let ok = parse_answer("c", &c, &json!({"type":"choice","choice":"no","probabilities":{"yes":0.3,"no":0.7},"confidence":0.7})).unwrap();
    assert_eq!(ok.value, 0.7);
    let s = t(spec::Ask::Score {
        instructions: "How good is `x`?".into(),
        rungs: vec![
            ("a".into(), "the lowest rung".into()),
            ("b".into(), "the middle rung".into()),
            ("c".into(), "the highest rung".into()),
        ],
    });
    assert!(
        parse_answer(
            "s",
            &s,
            &json!({"type":"score","score":1.5,"confidence":0.5})
        )
        .is_ok()
    );
    assert!(
        parse_answer(
            "s",
            &s,
            &json!({"type":"score","score":2.5,"confidence":0.5})
        )
        .is_err()
    );
}

// ------------------------------------------------------------------ critic

#[tokio::test]
async fn clean_code_approves() {
    let r = run(
        &[("a.py", "def add(a, b):\n    return a + b\n")],
        "write add",
    )
    .await;
    assert!(r.kev_ok);
    assert_eq!(r.approval, Some(true), "{:?}", r.reasons);
    assert_eq!(r.completeness_basis, "code");
    assert!(r.flagged_chunks.is_empty());
}

#[tokio::test]
async fn each_rule_rejects_and_names_itself() {
    for (marker, needle) in [
        ("EVAL", "security vulnerability"),
        ("TODO", "unfinished work"),
        ("REVIEWER", "addressed to the reviewer"),
        ("HALF", "does not do everything"),
    ] {
        let r = run(&[("a.py", &format!("# {marker}\nx = 1\n"))], "t").await;
        assert_eq!(r.approval, Some(false), "{marker}");
        assert!(
            r.reasons.iter().any(|x| x.contains(needle)),
            "{marker}: {:?}",
            r.reasons
        );
    }
}

#[tokio::test]
async fn likely_defect_is_reported_but_does_not_gate() {
    // code_chunk.defect is advisory: measured AUC 0.51 on clean vs mutated code (2026-09-27).
    let r = run(&[("a.py", "# BUG\nx = 1\n")], "t").await;
    assert_eq!(r.approval, Some(true), "{:?}", r.reasons);
    assert!(r.p_correct_min.unwrap() < 0.2);
    assert_eq!(r.worst_severity.as_deref(), Some("major"));
    assert!(r.flagged_chunks.is_empty());
    assert_eq!(r.advisory_chunks.len(), 1);
    assert!(r.advisory_chunks[0].location.starts_with("a.py:"));
    assert!(
        r.advisory_notes
            .iter()
            .any(|s| s.contains("approval is computed separately"))
    );
    assert!(
        !r.gates.iter().any(|g| g.ends_with(".defect")),
        "{:?}",
        r.gates
    );
    assert!(r.gates.contains(&"code_chunk.security".to_string()));
}

#[tokio::test]
async fn problem_in_one_chunk_is_located() {
    let clean: String = (0..200).map(|i| format!("v{i} = {i}\n")).collect();
    let r = run(&[("good.py", &clean), ("bad.py", "# EVAL\nx = 1\n")], "t").await;
    assert_eq!(r.approval, Some(false));
    assert_eq!(r.completeness_basis, "digest");
    assert!(
        r.advisory_notes
            .iter()
            .any(|s| s.contains("not task execution"))
    );
    assert_eq!(r.flagged_chunks.len(), 1);
    assert!(r.flagged_chunks[0].location.starts_with("bad.py:"));
}

#[tokio::test]
async fn unit_states_never_carry_the_request() {
    // Measured: request wording moved severity by +0.3 on identical code. Only the
    // completeness state may contain the task.
    let f = Fake::new();
    let task = "UNIQUE-TASK-TEXT fix every bug";
    let _ = critic(&f, &cfg(), task, &[("a.py".into(), "x = 1\n".into())], 0).await;
    let _ = critic_diff(&f, &cfg(), task, DIFF, 0).await;
    let states = f.states.lock().unwrap();
    let with_task: Vec<&Value> = states
        .iter()
        .filter(|s| s.to_string().contains("UNIQUE-TASK-TEXT"))
        .collect();
    assert_eq!(with_task.len(), 2, "one completeness state per call");
    assert!(
        with_task.iter().all(|s| s.get("task").is_some()
            && s.get("code").is_none()
            && s.get("change").is_none())
    );
}

#[tokio::test]
async fn offline_never_fabricates() {
    let f = Fake::offline();
    let r = critic(&f, &cfg(), "t", &[("a.py".into(), "x = 1\n".into())], 0).await;
    assert!(!r.kev_ok);
    assert!(r.error.is_some());
    assert_eq!(r.approval, None);
    assert!(r.correctness.is_none() && r.quality.is_none() && r.p_correct_min.is_none());
}

#[tokio::test]
async fn empty_artifact_is_an_error_without_model_calls() {
    let f = Fake::new();
    let r = critic(&f, &cfg(), "t", &[("a.py".into(), "   \n".into())], 0).await;
    assert!(r.error.is_some() && r.approval.is_none());
    assert_eq!(*f.calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn oversized_artifact_is_not_approved_on_partial_coverage() {
    let big: String = (0..20_000).map(|i| format!("v{i} = {i}\n")).collect();
    let r = run(&[("big.py", &big)], "t").await;
    assert!(r.chunks_unscored > 0);
    assert_eq!(r.approval, None);
}

#[tokio::test]
async fn revision_cap() {
    let c = cfg();
    let f = [("a.py".to_string(), "x = 1\n".to_string())];
    assert!(critic(&Fake::new(), &c, "t", &f, 1).await.revise_allowed);
    assert!(!critic(&Fake::new(), &c, "t", &f, 2).await.revise_allowed);
}

// ------------------------------------------------------------------ validate

#[tokio::test]
async fn validate_command_failure_rejects() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.json"), r#"{"ok": true}"#).unwrap();
    let paths = vec![d.path().join("a.json").display().to_string()];
    let cmds = vec!["exit 1".to_string()];
    let r = artifact_validate(
        &Fake::new(),
        &cfg(),
        ValidateInput {
            content: None,
            kind: None,
            paths: &paths,
            base_dir: d.path().to_str(),
            commands: &cmds,
            task: "t",
            use_model: false,
            revision: 0,
        },
    )
    .await;
    assert!(!r.deterministic_ok);
    assert_eq!(r.approval, Some(false));
}

#[tokio::test]
async fn validate_parse_failure_and_missing_file_reject() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("bad.json"), "{nope").unwrap();
    let paths = vec!["bad.json".to_string(), "gone.py".to_string()];
    let r = artifact_validate(
        &Fake::new(),
        &cfg(),
        ValidateInput {
            content: None,
            kind: None,
            paths: &paths,
            base_dir: d.path().to_str(),
            commands: &[],
            task: "t",
            use_model: true,
            revision: 0,
        },
    )
    .await;
    assert_eq!(r.approval, Some(false));
    assert_eq!(r.missing_files.len(), 1);
    assert!(r.reasons.iter().any(|x| x.contains("json_parse")));
}

#[tokio::test]
async fn validate_offline_critic_gives_unknown() {
    let f = Fake::offline();
    let r = artifact_validate(
        &f,
        &cfg(),
        ValidateInput {
            content: Some("x = 1\n"),
            kind: Some("python"),
            paths: &[],
            base_dir: None,
            commands: &[],
            task: "t",
            use_model: true,
            revision: 0,
        },
    )
    .await;
    assert!(r.deterministic_ok);
    assert_eq!(r.approval, None);
}

// ------------------------------------------------------------------ maps

fn n(beat: f64, x: i64, y: i64, color: i64, dir: i64) -> Note {
    Note {
        beat,
        x,
        y,
        color,
        dir,
    }
}

#[test]
fn map_stats_detect_problems() {
    let notes = vec![
        n(1.0, 0, 0, 0, 1),
        n(0.5, 5, 0, 1, 1), // out of grid + unsorted
        n(0.5, 5, 0, 1, 1), // duplicate
        n(2.0, 1, 1, 0, 1), // left hand down again -> parity break
        n(3.0, 2, 1, 1, 9), // invalid direction
    ];
    let s = map::stats("v3", &notes, 0, 0, 0);
    assert_eq!(s.out_of_grid, 2);
    assert_eq!(s.unsorted_pairs, 1);
    assert_eq!(s.duplicate_positions, 1);
    assert_eq!(s.invalid_directions, 1);
    assert!(s.parity_breaks >= 1);
    assert!(!map::heuristic_issues(&s).is_empty());
}

#[test]
fn alternating_swings_have_no_parity_breaks() {
    let notes: Vec<Note> = (0..32)
        .map(|i| n(i as f64, 1, 0, 0, if i % 2 == 0 { 1 } else { 0 }))
        .collect();
    assert_eq!(map::stats("v3", &notes, 0, 0, 0).parity_breaks, 0);
}

#[test]
fn parses_v2_and_v3() {
    let v3 = json!({"version":"3.3.0","colorNotes":[{"b":1,"x":0,"y":0,"c":0,"d":1}],"bombNotes":[{"b":2,"x":1,"y":1}]});
    let (f, notes, bombs, _, _) = map::parse_notes(&v3).unwrap();
    assert_eq!((f.as_str(), notes.len(), bombs), ("v3", 1, 1));
    let v2 = json!({"_notes":[{"_time":1,"_lineIndex":0,"_lineLayer":0,"_type":0,"_cutDirection":1},{"_time":2,"_lineIndex":1,"_lineLayer":0,"_type":3,"_cutDirection":0}]});
    let (f, notes, bombs, _, _) = map::parse_notes(&v2).unwrap();
    assert_eq!((f.as_str(), notes.len(), bombs), ("v2", 1, 1));
    assert!(map::parse_notes(&json!({"rooms": []})).is_none());
}

#[test]
fn same_direction_after_a_long_gap_is_a_reset_not_a_parity_break() {
    // down at 0, down again at 4 beats: a reset (ranked maps do this); down at 4.5: a break.
    let notes = vec![n(0.0, 1, 0, 0, 1), n(4.0, 1, 0, 0, 1), n(4.5, 1, 0, 0, 1)];
    let s = map::stats("v3", &notes, 0, 0, 0);
    assert_eq!(s.parity_breaks, 1);
    assert_eq!(s.parity_break_rate, Some(0.333));
}

#[test]
fn parity_threshold_allows_ranked_rates() {
    // 6% parity breaks is within what ranked maps measured (max 6.3%); 10% is not.
    let mut s = map::MapStats {
        note_count: 100,
        parity_breaks: 6,
        ..Default::default()
    };
    assert!(map::heuristic_issues(&s).is_empty());
    s.parity_breaks = 10;
    assert!(
        map::heuristic_issues(&s)
            .iter()
            .any(|i| i.contains("parity breaks"))
    );
}

#[test]
fn parses_v4_with_omitted_zero_fields() {
    // v4: colorNotes index colorNotesData; zero-valued fields are omitted by editors.
    let v4 = json!({"version":"4.0.0",
        "colorNotes":[{"b":1,"i":0},{"b":2,"i":1},{"i":0},{"b":4},{"b":5,"i":9}],
        "colorNotesData":[{"x":1,"d":1},{"x":2,"y":1,"c":1}],
        "bombNotes":[{"b":3,"i":0}], "obstacles":[]});
    let (f, notes, bombs, _, invalid) = map::parse_notes(&v4).unwrap();
    // {"b":4} has its index omitted (0); index 9 does not exist.
    assert_eq!((f.as_str(), notes.len(), bombs, invalid), ("v4", 4, 1, 1));
    assert_eq!((notes[3].beat, notes[3].x, notes[3].dir), (4.0, 1, 1));
    assert_eq!(
        (notes[0].x, notes[0].y, notes[0].color, notes[0].dir),
        (1, 0, 0, 1)
    );
    assert_eq!(
        (notes[1].x, notes[1].y, notes[1].color, notes[1].dir),
        (2, 1, 1, 0)
    );
    assert_eq!(notes[2].beat, 0.0);
}

#[test]
fn unity_app_event_stream_that_never_connected_is_a_breakage() {
    let u = json!({"app_events": {"url": "ws://127.0.0.1:4733", "connected": false, "error": "refused"}, "log": {"counts": {}}});
    let b = kritic::unity::judge_unity(&u, None, None);
    assert!(
        b.iter()
            .any(|x| x.contains("never connected") && x.contains("refused")),
        "{b:?}"
    );
    let ok = json!({"app_events": {"url": "ws://127.0.0.1:4733", "connected": true}, "log": {"counts": {}}});
    assert!(kritic::unity::judge_unity(&ok, None, None).is_empty());
}

fn clean_map() -> String {
    let notes: Vec<Value> = (0..64)
        .map(|i| json!({"b": i as f64 * 0.5, "x": 1 + (i % 2), "y": 0, "c": i % 2, "d": if (i / 2) % 2 == 0 { 1 } else { 0 }}))
        .collect();
    json!({"version":"3.3.0","colorNotes": notes}).to_string()
}

#[tokio::test]
async fn map_eval_clean_approves() {
    let r = map::evaluate(&Fake::new(), &cfg(), &clean_map(), Some(120.0), true).await;
    assert_eq!(r.map_kind, "beat_saber");
    assert!(r.heuristic_issues.is_empty(), "{:?}", r.heuristic_issues);
    assert_eq!(r.approval, Some(true), "{:?}", r.reasons);
    assert!(r.slices_scored >= 2);
}

#[tokio::test]
async fn map_eval_slice_flags_are_advisory_and_bad_json_fails() {
    let mut v: Value = serde_json::from_str(&clean_map()).unwrap();
    v["colorNotes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"b": 99.0, "x": 1, "y": 0, "c": 0, "d": 8}));
    // map_slice.awkward is advisory: measured AUC 0.685 on ranked vs broken stretches.
    let r = map::evaluate(&Fake::new(), &cfg(), &v.to_string(), Some(120.0), true).await;
    assert!(
        r.flagged_slices
            .iter()
            .any(|s| s.start_beat <= 99.0 && 99.0 < s.end_beat)
    );
    assert_eq!(r.approval, Some(true), "{:?}", r.reasons);
    assert!(
        r.kev_advisory.iter().any(|a| a.contains("awkward")),
        "{:?}",
        r.kev_advisory
    );
    let bad = map::evaluate(&Fake::new(), &cfg(), "{nope", None, false).await;
    assert_eq!(bad.approval, Some(false));
}

#[tokio::test]
async fn map_eval_is_deterministic_by_default_and_offline_keeps_the_verdict() {
    let f = Fake::new();
    let r = map::evaluate(&f, &cfg(), &clean_map(), None, false).await;
    assert_eq!(r.approval, Some(true));
    assert_eq!(
        *f.calls.lock().unwrap(),
        0,
        "no Kev calls unless kev_slices"
    );
    let off = map::evaluate(&Fake::offline(), &cfg(), &clean_map(), None, true).await;
    assert!(!off.kev_ok && off.error.is_some());
    assert_eq!(
        off.approval,
        Some(true),
        "Kev is advisory here, so its absence does not change the verdict"
    );
}

// ------------------------------------------------------------------ route / trim

#[tokio::test]
async fn route_suggests_with_fixed_basis() {
    let r = route(&Fake::new(), "rename a variable", "", "").await;
    assert_eq!(r.suggestion.as_deref(), Some("haiku"));
    assert!(r.basis.unwrap().contains("mechanical"));
    assert!(route(&Fake::new(), "  ", "", "").await.error.is_some());
}

#[tokio::test]
async fn trim_ranks_and_budgets() {
    let chunks = vec![
        "apple pie".to_string(),
        "car engine".to_string(),
        "apple tree".to_string(),
    ];
    let r = context_trim(&Fake::new(), &cfg(), "fruit", &chunks, 1000, 0.2, None).await;
    assert_eq!(r.kept, vec![0, 2]);
    assert_eq!(r.dropped, vec![1]);
    let r = context_trim(&Fake::new(), &cfg(), "fruit", &chunks, 1000, 0.2, Some(1)).await;
    assert_eq!(r.kept.len(), 1);
    let too_many = vec!["x".to_string(); 65];
    assert!(
        context_trim(&Fake::new(), &cfg(), "q", &too_many, 1000, 0.2, None)
            .await
            .error
            .is_some()
    );
}

// ------------------------------------------------------------------ diff mode

const DIFF: &str = "diff --git a/src/a.py b/src/a.py
index 111..222 100644
--- a/src/a.py
+++ b/src/a.py
@@ -1,3 +1,3 @@ def f():
 def f():
-    return 1
+    return 2
@@ -10,2 +10,3 @@ def g():
 def g():
+    # EVAL
     pass
diff --git a/src/b.py b/src/b.py
new file mode 100644
--- /dev/null
+++ b/src/b.py
@@ -0,0 +1,2 @@
+def h():
+    return 3
";

#[test]
fn parses_hunks_per_file_with_new_line_numbers() {
    let h = parse_diff(DIFF, 1300);
    let got: Vec<(String, usize)> = h.iter().map(|h| (h.file.clone(), h.new_start)).collect();
    assert_eq!(
        got,
        vec![
            ("src/a.py".into(), 1),
            ("src/a.py".into(), 10),
            ("src/b.py".into(), 1)
        ]
    );
    assert!(h.iter().all(|h| h.text.starts_with("@@")));
}

#[test]
fn long_hunks_are_sliced_with_header() {
    let body: String = (0..300).map(|i| format!("+line {i}\n")).collect();
    let d = format!("diff --git a/x b/x\n@@ -0,0 +1,300 @@\n{body}");
    let h = parse_diff(&d, 500);
    assert!(h.len() > 3);
    assert!(
        h.iter()
            .all(|h| h.text.len() <= 560 && h.text.starts_with("@@ -0,0 +1,300 @@"))
    );
}

#[tokio::test]
async fn diff_mode_flags_the_bad_hunk() {
    let r = critic_diff(&Fake::new(), &cfg(), "t", DIFF, 0).await;
    assert_eq!(r.mode, "diff");
    assert_eq!(r.approval, Some(false));
    assert_eq!(r.flagged_chunks.len(), 1);
    assert_eq!(r.flagged_chunks[0].location, "src/a.py:+10");
    assert!(
        r.reasons
            .iter()
            .any(|x| x.contains("security vulnerability")),
        "{:?}",
        r.reasons
    );
    let clean = DIFF.replace("# EVAL", "# ok");
    assert_eq!(
        critic_diff(&Fake::new(), &cfg(), "t", &clean, 0)
            .await
            .approval,
        Some(true)
    );
    assert!(
        critic_diff(&Fake::new(), &cfg(), "t", "", 0)
            .await
            .error
            .is_some()
    );
}

fn sh(dir: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "{:?}",
        String::from_utf8_lossy(&ok.stderr)
    );
}

#[tokio::test]
async fn git_diff_includes_modified_and_untracked_but_filters_junk() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    sh(p, &["init", "-q"]);
    sh(p, &["config", "user.email", "t@t"]);
    sh(p, &["config", "user.name", "t"]);
    std::fs::write(p.join("a.py"), "x = 1\n").unwrap();
    sh(p, &["add", "."]);
    sh(p, &["commit", "-qm", "init"]);
    std::fs::write(p.join("a.py"), "x = 2\n").unwrap();
    std::fs::write(p.join("new.py"), "y = 1\n").unwrap();
    std::fs::create_dir(p.join("node_modules")).unwrap();
    std::fs::write(p.join("node_modules/z.js"), "junk").unwrap();
    let diff = git_diff(p.to_str(), &[]).await.unwrap();
    assert!(diff.contains("+x = 2") && diff.contains("-x = 1"));
    assert!(diff.contains("b/new.py") && diff.contains("+y = 1"));
    assert!(!diff.contains("junk"));
    let only_a = git_diff(p.to_str(), &["a.py".to_string()]).await.unwrap();
    assert!(only_a.contains("a.py") && !only_a.contains("new.py"));
}

#[tokio::test]
async fn low_mass_severity_label_does_not_reject() {
    // argmax "critical" at 0.30 with most mass on none/minor must not trip the severity rule
    let r = run(&[("a.py", "# SEVLABEL\nx = 1\n")], "t").await;
    assert_eq!(r.worst_severity.as_deref(), Some("critical"));
    assert!(r.p_severe_max.unwrap() <= 0.5);
    assert_eq!(r.approval, Some(true), "{:?}", r.reasons);
}

#[test]
fn app_events_must_be_loopback() {
    use kritic::tools::require_loopback_ws;
    for ok in [
        "ws://127.0.0.1:4733",
        "ws://localhost:4733/",
        "ws://[::1]:4733",
        "wss://LOCALHOST:1",
    ] {
        assert!(require_loopback_ws(ok).is_ok(), "{ok}");
    }
    for bad in [
        "ws://example.com:4733",
        "ws://127.0.0.1.evil.com:1",
        "ws://user@127.0.0.1:1",
        "http://127.0.0.1:4733",
        "ws://10.0.0.5:4733",
    ] {
        assert!(require_loopback_ws(bad).is_err(), "{bad}");
    }
}

// ------------------------------------------------------------------ hygiene

#[test]
fn desktop_fails_fast_on_busy_port_stall_and_leaks() {
    let report = serde_json::json!({
        "ok": true, "crashes": [], "hangs": [], "interactions": [], "persistence": [],
        "orphans": ["9700 node.exe"],
        "hygiene": {"processes": [{"pid": 26644, "name": "Example App.exe", "age_s": 43, "ports": [4733]}],
                    "ports": [{"port": 4733, "pid": 27360, "name": "exited owner (inherited handle)"}]},
        "unity": {"preflight": {"port_busy": "port 4733 held by PID 14776 (python.exe)"},
                  "ready": {"stalled": "no app event and no log output for 15s"}, "steps": []}
    });
    let (_, b, approval) = kritic::phase3::judge_desktop(&report, &[]);
    let all = b.join("\n");
    assert_eq!(approval, Some(false), "{all}");
    for needle in [
        "port 4733 held by PID 14776",
        "stalled while starting",
        "9700 node.exe",
        "still alive after it ended",
        "port 4733 still listening",
    ] {
        assert!(all.contains(needle), "missing {needle:?} in:\n{all}");
    }
}

#[test]
fn hygiene_audit_reports_orphans_inherited_sockets_and_leftovers() {
    let audit = serde_json::json!({
        "orphans": [{"pid": 9700, "name": "node.exe", "age_s": 600, "ports": [4733]}],
        "dead_owner_sockets": [{"port": 4733, "owner_pid": 27360, "inheritors": [{"pid": 9700, "name": "node.exe"}]}],
        "leftovers": ["C:/apps/example/harness/results.txt"]
    });
    let r = kritic::hygiene::judge_audit(&audit);
    assert_eq!(r.len(), 3, "{r:?}");
    assert!(
        r[1].contains("inherited the listening handle (9700 node.exe)"),
        "{r:?}"
    );
    assert!(kritic::hygiene::judge_audit(&serde_json::json!({})).is_empty());
}

// ------------------------------------------------------------------ stalls

#[tokio::test]
async fn critic_past_its_deadline_reports_a_stall_instead_of_running_on() {
    let mut c = cfg();
    c.critic_deadline_s = 0;
    let r = critic(
        &Fake::new(),
        &c,
        "t",
        &[("a.rs".into(), "fn a() {}\n".into())],
        0,
    )
    .await;
    let e = r.error.expect("a stall is an error");
    assert!(e.contains("critic stalled") && e.contains("0 of 1"), "{e}");
}

#[test]
fn plain_unified_diff_after_git_diff_keeps_file_identity() {
    let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n--- a/C:/guard/with spaces.py\told timestamp\n+++ b/C:/guard/with spaces.py\tnew timestamp\n@@ -3 +3 @@\n-old_guard\n+new_guard\n";
    let hunks = parse_diff(diff, 1300);
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks[0].file, "a.rs");
    assert_eq!(hunks[1].file, "C:/guard/with spaces.py");
    assert_eq!(hunks[1].new_start, 3);
}

#[test]
fn header_like_source_and_no_newline_markers_are_preserved() {
    let diff = "--- a/operators.rs\n+++ b/operators.rs\n@@ -1 +1 @@\n---removed literal\n+++added literal\n\\ No newline at end of file\n";
    let hunks = parse_diff(diff, 1300);
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].file, "operators.rs");
    assert!(hunks[0].text.contains("---removed literal"));
    assert!(hunks[0].text.contains("+++added literal"));
    assert!(hunks[0].text.contains("No newline"));
}

#[test]
fn plain_additions_and_deletions_keep_paths_and_zero_counts() {
    let diff = "--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1,2 @@\n+one\n+two\n--- a/old.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n";
    let hunks = parse_diff(diff, 1300);
    assert_eq!(hunks.len(), 2);
    assert_eq!((&*hunks[0].file, hunks[0].new_start), ("new.rs", 1));
    assert_eq!((&*hunks[1].file, hunks[1].new_start), ("old.rs", 0));
}

#[test]
fn split_hunk_locations_advance_for_additions_and_context_only() {
    let diff = "--- a/a.rs\n+++ b/a.rs\n@@ -10,3 +20,3 @@\n context\n-old\n+new\n tail\n";
    let hunks = parse_diff(diff, 1);
    assert_eq!(
        hunks.iter().map(|h| h.new_start).collect::<Vec<_>>(),
        vec![21, 21]
    );
    assert_eq!(hunks.iter().filter(|h| h.text.contains("-old")).count(), 1);
    assert_eq!(hunks.iter().filter(|h| h.text.contains("+new")).count(), 1);
}

/// A laya that answers its health check but never answers a request: the request must fail
/// within the limit, not wait for ever (a critic call once hung for over an hour this way).
#[tokio::test]
async fn kev_request_to_a_silent_laya_fails_within_its_limit() {
    use kritic::kev::KevClient;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut c = cfg();
    // Health must satisfy the configured device policy so this test reaches
    // the request timeout rather than failing early on GPU validation.
    let health =
        json!({"status":"ok", "device":c.laya_device, "gpu_uuid":c.kev_gpu_uuid}).to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let body = health.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                if String::from_utf8_lossy(&buf[..n]).starts_with("GET /health") {
                    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await;
                } else {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await; // never answers
                }
            });
        }
    });
    c.laya_url = format!("http://127.0.0.1:{port}");
    c.vision_gpu_kev_url = None; // never route this test to a real second Kev
    c.laya_exe = "kritic-test-no-such-laya.exe".into(); // the stall handler's stop must not touch a real laya
    c.kev_request_s = 1;
    let kev = KevClient::new(std::sync::Arc::new(c));
    let qs = kritic::critic::completeness_spec("code").wire();
    let t0 = std::time::Instant::now();
    let r = kev.decide(&json!({"code": "x"}), &qs).await;
    let e = r.expect_err("a silent laya is an error");
    assert!(e.contains("stalled") || e.contains("timed out"), "{e}");
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(10),
        "took {:?}",
        t0.elapsed()
    );
}

// ------------------------------------------------------------------ ux_critic (offline)

mod ux_offline {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use kritic::ux::{self, Adapter, Control, Navigator, Obs, Options, Run};
    use serde_json::{Value, json};

    /// (label, x, target screen; "" = disabled)
    type Button = (&'static str, f64, &'static str);
    type Screen = (&'static str, &'static str, Vec<Button>);

    /// A screen graph: screen -> (visible text, buttons).
    struct FakeApp {
        screens: HashMap<&'static str, (&'static str, Vec<Button>)>,
        at: &'static str,
    }

    impl FakeApp {
        fn new(screens: Vec<Screen>) -> Self {
            FakeApp {
                screens: screens.into_iter().map(|(n, t, c)| (n, (t, c))).collect(),
                at: "home",
            }
        }
        fn obs(&self) -> Obs {
            let (text, cs) = &self.screens[self.at];
            let controls = cs
                .iter()
                .map(|(l, x, to)| Control {
                    id: format!("button:{l}#1"),
                    label: l.to_string(),
                    text: l.to_string(),
                    role: "button".into(),
                    rect: [*x, 10.0, 80.0, 30.0],
                    enabled: !to.is_empty(),
                    tooltip: if to.is_empty() {
                        String::new()
                    } else {
                        "t".into()
                    },
                    ..Default::default()
                })
                .collect();
            Obs {
                fingerprint: self.at.into(),
                text: text.to_string(),
                viewport: [1200.0, 900.0],
                controls,
                ..Default::default()
            }
        }
    }

    impl Adapter for FakeApp {
        async fn reset(&mut self) -> Result<(), String> {
            self.at = "home";
            Ok(())
        }
        async fn observe(&mut self, _opts: Value) -> Result<Obs, String> {
            Ok(self.obs())
        }
        async fn act(&mut self, id: &str) -> Result<(), String> {
            let (_, cs) = &self.screens[self.at];
            let (_, _, to) = cs
                .iter()
                .find(|(l, _, _)| format!("button:{l}#1") == id)
                .ok_or(format!("no {id}"))?;
            self.at = to;
            Ok(())
        }
    }

    /// Presses the label containing a word from the goal, else the first control.
    struct Keyword;
    impl Navigator for Keyword {
        async fn choose(
            &self,
            goal: &str,
            _p: &[String],
            _i: Option<&str>,
            labels: &[String],
            _seed: u64,
        ) -> Result<usize, String> {
            Ok(labels
                .iter()
                .position(|l| goal.split(' ').any(|w| w.len() > 3 && l.contains(w)))
                .unwrap_or(0))
        }
    }

    const SPEC: &str = r#"
[app]
kind = "web"
purpose = "test app"
url = "http://localhost/"
[[modes]]
name = "Library"
done = { text = "Library" }
[[modes]]
name = "Composer"
done = { text = "Composer" }
[[tasks]]
name = "compose"
goal = "Open the Composer"
done = { text = "Composer" }
primary = true
[[tasks]]
name = "export"
goal = "Export the map"
done = { text = "Exported" }
budget = 1
"#;

    fn buried() -> FakeApp {
        FakeApp::new(vec![
            (
                "home",
                "Library",
                vec![("Editor", 10.0, "editor"), ("Export", 1100.0, "export1")],
            ),
            (
                "editor",
                "Editor",
                vec![("Library", 10.0, "home"), ("More", 100.0, "menu")],
            ),
            (
                "menu",
                "Editor menu",
                vec![("Library", 10.0, "home"), ("Composer", 100.0, "composer")],
            ),
            ("composer", "Composer", vec![("Library", 10.0, "home")]),
            (
                "export1",
                "Export dialog",
                vec![("Export now", 10.0, "exported")],
            ),
            ("exported", "Exported", vec![("Library", 10.0, "home")]),
        ])
    }

    fn opts() -> Options {
        Options {
            max_depth: 5,
            max_actions: 200,
            runs: 3,
            deadline: Instant::now() + Duration::from_secs(60),
            out_dir: std::env::temp_dir(),
        }
    }

    #[tokio::test]
    async fn bfs_measures_optimal_steps_and_fails_buried_mode_and_over_budget() {
        let spec = ux::parse_spec(SPEC, "t.toml").unwrap();
        let mut app = buried();
        let mut run = Run::new(&spec, opts());
        run.explore(&mut app).await;
        let r = run.finish(false);
        assert_eq!(r.approval, Some(false));
        assert_eq!(r.modes[0].best.optimal_steps, Some(0));
        assert_eq!(r.modes[1].best.optimal_steps, Some(3));
        assert_eq!(r.modes[1].best.path, vec!["Editor", "More", "Composer"]);
        assert!(
            r.hard_fails
                .iter()
                .any(|h| h.contains("mode \"Composer\" has no top-level entry")),
            "{:?}",
            r.hard_fails
        );
        assert_eq!(r.tasks[1].best.optimal_steps, Some(2));
        assert!(
            r.hard_fails
                .iter()
                .any(|h| h.contains("task \"export\": takes 2 actions, budget 1")),
            "{:?}",
            r.hard_fails
        );
        assert_eq!(r.exploration.screens, 6);
        assert!(r.exploration.truncated.is_none());
    }

    #[tokio::test]
    async fn top_level_entries_pass_and_novice_is_recorded() {
        let spec = ux::parse_spec(SPEC, "t.toml").unwrap();
        let mut app = FakeApp::new(vec![
            (
                "home",
                "Library",
                vec![
                    ("Library", 10.0, "home"),
                    ("Composer", 100.0, "composer"),
                    ("Export", 200.0, "exported"),
                ],
            ),
            ("composer", "Composer", vec![("Library", 10.0, "home")]),
            (
                "exported",
                "Library Exported",
                vec![("Library", 10.0, "home")],
            ),
        ]);
        let mut run = Run::new(&spec, opts());
        run.explore(&mut app).await;
        assert!(run.wants_novice());
        run.novice(&mut app, &Keyword, "keyword", true).await;
        let r = run.finish(true);
        assert_eq!(r.approval, Some(true), "{:?}", r.hard_fails);
        let n = r.tasks[0].novice.as_ref().unwrap();
        assert_eq!((n.runs.len(), n.successes), (3, 3));
        assert_eq!(n.runs[0].tried, vec!["Composer"]);
    }

    #[tokio::test]
    async fn novice_failing_a_primary_task_two_of_three_fails_it() {
        let spec = ux::parse_spec(SPEC, "t.toml").unwrap();
        // the entry exists, but its label never tells the novice it is the Composer
        let mut app = FakeApp::new(vec![
            (
                "home",
                "Library",
                vec![
                    ("Library", 10.0, "home"),
                    ("Workspace B", 100.0, "composer"),
                    ("Export", 200.0, "exported"),
                ],
            ),
            ("composer", "Composer", vec![("Library", 10.0, "home")]),
            (
                "exported",
                "Library Exported",
                vec![("Library", 10.0, "home")],
            ),
        ]);
        let mut run = Run::new(&spec, opts());
        run.explore(&mut app).await;
        run.novice(&mut app, &Keyword, "keyword", true).await;
        let r = run.finish(true);
        assert_eq!(r.tasks[0].best.optimal_steps, Some(1));
        assert_eq!(r.tasks[0].novice.as_ref().unwrap().successes, 0);
        assert!(
            r.hard_fails
                .iter()
                .any(|h| h.contains("primary task \"compose\": the novice failed 3 of 3")),
            "{:?}",
            r.hard_fails
        );
        // not primary: the novice result is reported, never a failure
        assert!(r.tasks[1].fails.is_empty());

        // an advisory novice (the Kev fallback) is reported but never fails a task
        let mut run = Run::new(&spec, opts());
        run.explore(&mut app).await;
        run.novice(&mut app, &Keyword, "kev", false).await;
        let r = run.finish(true);
        assert_eq!(r.tasks[0].novice.as_ref().unwrap().successes, 0);
        assert!(
            r.hard_fails.iter().all(|h| !h.contains("novice")),
            "{:?}",
            r.hard_fails
        );
        assert_eq!(r.novice, "kev, advisory");
    }

    #[tokio::test]
    async fn unreachable_and_trivially_done_tasks_fail_and_the_action_cap_is_reported() {
        let spec = ux::parse_spec(
            &SPEC.replace("text = \"Exported\"", "text = \"Library\""),
            "t.toml",
        )
        .unwrap();
        let mut app = buried();
        let mut run = Run::new(
            &spec,
            Options {
                max_actions: 2,
                ..opts()
            },
        );
        run.explore(&mut app).await;
        let r = run.finish(false);
        assert!(
            r.hard_fails
                .iter()
                .any(|h| h.contains("already holds on the home screen")),
            "{:?}",
            r.hard_fails
        );
        assert!(
            r.hard_fails
                .iter()
                .any(|h| h.contains("task \"compose\": unreachable") && h.contains("action cap 2")),
            "{:?}",
            r.hard_fails
        );
        assert_eq!(r.exploration.actions, 2);
    }

    #[test]
    fn rules_flag_labels_tooltips_targets_disabled_and_contrast() {
        let c = |id: &str, label: &str, text: &str, tip: &str, w: f64, enabled: bool| Control {
            id: id.into(),
            label: label.into(),
            text: text.into(),
            tooltip: tip.into(),
            role: "button".into(),
            rect: [0.0, 0.0, w, 30.0],
            enabled,
            ..Default::default()
        };
        let controls = vec![
            c("ok", "Save", "Save", "", 60.0, true),
            c("icon", "", "", "", 36.0, true),
            c("named-icon", "Play", "", "", 36.0, true),
            c("tipped-icon", "Play", "", "Play (Space)", 36.0, true),
            c("tiny", "x", "x", "", 16.0, true),
            c("dis", "Apply", "Apply", "", 60.0, false),
            c("dis-why", "Apply", "Apply", "Set points first", 60.0, false),
        ];
        let axe: Vec<kritic::ui::AxeViolation> = serde_json::from_value(json!([
            {"id": "color-contrast", "impact": "serious", "help": "contrast", "nodes": 1, "targets": ["#faint"]},
            {"id": "region", "impact": "moderate", "help": "x", "nodes": 1, "targets": ["main"]}
        ]))
        .unwrap();
        let hits: Vec<(String, String)> = ux::audit(&controls, &axe, [1600.0, 900.0])
            .into_iter()
            .map(|(r, _, k, _)| (r.to_string(), k))
            .collect();
        let has = |r: &str, k: &str| hits.contains(&(r.to_string(), k.to_string()));
        assert!(has("unlabelled", "icon") && !has("unlabelled", "ok"));
        assert!(
            has("icon_no_tooltip", "icon")
                && has("icon_no_tooltip", "named-icon")
                && !has("icon_no_tooltip", "tipped-icon")
        );
        assert!(has("small_target", "tiny") && !has("small_target", "ok"));
        assert!(has("disabled_no_reason", "dis") && !has("disabled_no_reason", "dis-why"));
        assert!(has("contrast", "#faint") && !hits.iter().any(|(r, _)| r == "region"));
    }

    #[test]
    fn scattered_steps_measure_against_the_screen_diagonal() {
        let at = |x: f64, y: f64| Control {
            rect: [x, y, 40.0, 20.0],
            ..Default::default()
        };
        let grouped = ux::mean_step_distance(
            &[at(100.0, 100.0), at(180.0, 100.0), at(260.0, 100.0)],
            [1280.0, 800.0],
        )
        .unwrap();
        let corners = ux::mean_step_distance(
            &[at(10.0, 10.0), at(1200.0, 760.0), at(1200.0, 10.0)],
            [1280.0, 800.0],
        )
        .unwrap();
        assert!(
            grouped < ux::SCATTERED && corners > ux::SCATTERED,
            "{grouped} {corners}"
        );
        assert_eq!(
            ux::mean_step_distance(&[at(1.0, 1.0)], [1280.0, 800.0]),
            None
        );
    }

    #[test]
    fn spec_errors_are_listed_at_once() {
        let bad = r#"
ignore = ["("]
[app]
kind = "phone"
purpose = ""
[[modes]]
name = "A"
done = {}
[[tasks]]
name = "t"
goal = ""
done = { url = "x", event = "(" }
budget = 5
primary = true
"#;
        let e = ux::parse_spec(bad, "bad.toml").unwrap_err().join("\n");
        for want in [
            "app.kind",
            "app.purpose",
            "ignore[0]",
            "modes[0] (A).done: give at least one",
            "tasks[0] (t).goal",
            "a primary task takes at most 3",
            "done.url: only for kind",
            "done.event: needs app.app_events",
            "done.event: invalid regex",
        ] {
            assert!(e.contains(want), "missing {want:?} in:\n{e}");
        }
        let unknown = "[app]\nkind='web'\npurpose='p'\nurl='u'\nbogus=1\n[[tasks]]\nname='t'\ngoal='g'\ndone={text='x'}";
        assert!(
            ux::parse_spec(unknown, "x").is_err(),
            "unknown keys are rejected"
        );
        let unity = "[app]\nkind='unity'\npurpose='p'\nexe='a.exe'\n[[tasks]]\nname='t'\ngoal='g'\ndone={text='x'}";
        assert!(ux::parse_spec(unity, "x").unwrap_err()[0].contains("app_events"));
    }

    #[test]
    fn sampling_is_seeded_and_follows_weights() {
        let w = [0.0, 0.9, 0.1];
        assert_eq!(ux::sample(&w, 7), ux::sample(&w, 7));
        let ones = (1..=1000u64).filter(|s| ux::sample(&w, *s) == 1).count();
        assert!((800..=980).contains(&ones), "{ones}");
        assert!((1..=1000u64).all(|s| ux::sample(&w, s) != 0));
    }

    #[test]
    fn novice_kev_spec_is_builtin_and_advisory() {
        let s = kritic::spec::get("ux_novice_step");
        assert!(s.lint().is_empty(), "{:?}", s.lint());
        assert!(!s.gates("next_step"));
    }
}
