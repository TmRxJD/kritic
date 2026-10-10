//! Repo plugin discovery, contract validation, decision-spec linting, and approval rules (fake Kev).

use std::collections::BTreeMap;

use kritic::config::Config;
use kritic::kev::{Answer, Decider, Decision, Question};
use kritic::plugins::{discover, run};
use serde_json::{Value, json};

struct FakeKev(f64);
impl Decider for FakeKev {
    async fn decide(
        &self,
        _s: &Value,
        qs: &BTreeMap<String, Question>,
    ) -> Result<Decision, String> {
        let answers = qs
            .keys()
            .map(|k| {
                (
                    k.clone(),
                    Answer {
                        value: self.0,
                        choice: None,
                        probabilities: None,
                        confidence: 0.9,
                    },
                )
            })
            .collect();
        Ok(Decision {
            answers,
            truncated: false,
            latency_ms: 1,
            input_tokens: 10,
        })
    }
}

const SCRIPT: &str = r#"
import json, sys
inp = json.load(sys.stdin)
a = inp["args"]
print(json.dumps({"event": "scored", "n": 1}), file=sys.stderr, flush=True)
if a.get("mode") == "garbage":
    print("not json"); sys.exit(0)
findings = [{"severity": "high", "message": "parity break at beat 12", "file": "Expert.dat", "line": 3}] if a.get("mode") == "bad" else []
state = {"parity": "TYPICAL: 0.004 breaks per note"}
if a.get("mode") == "wrong_state":
    state = {"parity": "0.004"}
print(json.dumps({"ok": True, "approval": a.get("mode") != "bad", "summary": "scored map", "metrics": {"nps": 5.2, "parity_breaks": len(findings)}, "findings": findings, "kev_state": state}))
"#;

/// A plugin spec as a repo would write it: every field defined, the question names its field and
/// defines both answers, and the gate carries dated calibration evidence.
const MANIFEST: &str = r#"name = "map-score"
description = "Score a Beat Saber map"
command = ["python", "score.py"]
timeout_s = 30

[input_schema]
type = "object"
required = ["mode"]

[kev]
name = "map_score_kev"
purpose = "Judge the parity band a map-score run reports."

[[kev.fields]]
name = "parity"
meaning = "The map's parity break rate, judged by code against ranked maps."
kind = { kind = "band", vocabulary = { name = "ranked_range", terms = [["TYPICAL", "within the range of ranked maps"], ["BEYOND", "outside every ranked map measured"]] } }

[[kev.questions]]
id = "parity_ok"
ask = { type = "noul", instructions = "Is `parity` TYPICAL?", yes = "it reads TYPICAL", no = "it reads BEYOND" }
gate = { gate = "rule", measure = "yes", fails_when = "below", threshold = 0.5, reason = "parity outside ranked maps", calibration = "2026-09-27: 20 ranked vs 20 broken maps in the fixture set, TYPICAL 0.96 vs BEYOND 0.03" }
"#;

/// The old free-form style: a vague question over a raw blob, with an uncalibrated rule.
const VAGUE: &str = r#"name = "vague"
description = "Old-style Kev block"
command = ["python", "score.py"]

[kev]
name = "vague"
purpose = "Would players like it."

[[kev.fields]]
name = "metrics"
meaning = "Whatever numbers the plugin produced."
kind = { kind = "text", max_chars = 2000 }

[[kev.questions]]
id = "fun"
ask = { type = "noul", instructions = "Would players enjoy this map or find it boring?", yes = "fun", no = "boring" }
gate = { gate = "rule", measure = "yes", fails_when = "below", threshold = 0.5, reason = "not fun", calibration = "seems right" }
"#;

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let pdir = d.path().join(".kritic/plugins");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(d.path().join("score.py"), SCRIPT).unwrap();
    std::fs::write(pdir.join("map-score.toml"), MANIFEST).unwrap();
    std::fs::write(pdir.join("vague.toml"), VAGUE).unwrap();
    std::fs::write(
        pdir.join("broken.toml"),
        "name = \"x y\"\ncommand = []\ndescription = \"d\"\n",
    )
    .unwrap();
    d
}

#[test]
fn discovery_reports_invalid_manifests_and_specs() {
    let d = repo();
    let (ok, errs) = discover(d.path());
    assert_eq!(ok.len(), 1, "{errs:?}");
    assert_eq!(ok[0].name, "map-score");
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs.iter().any(|e| e.contains("broken.toml")));
    let vague = errs
        .iter()
        .find(|e| e.contains("vague.toml"))
        .expect("vague manifest refused");
    for rule in [
        "names no state field",
        "one condition",
        "must be defined",
        "calibration evidence",
        "no question reads it",
    ] {
        assert!(vague.contains(rule), "missing '{rule}' in {vague}");
    }
}

#[tokio::test]
async fn clean_run_approves_and_calibrated_gates_apply() {
    let d = repo();
    let cfg = Config::from_env();
    let r = run(
        &FakeKev(0.9),
        &cfg,
        d.path(),
        "map-score",
        &json!({"mode": "ok"}),
    )
    .await;
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.approval, Some(true), "{:?}", r.reasons);
    assert_eq!(r.metrics["nps"], 5.2);
    assert!(r.full_result.is_some());
    let low = run(
        &FakeKev(0.2),
        &cfg,
        d.path(),
        "map-score",
        &json!({"mode": "ok"}),
    )
    .await;
    assert_eq!(low.approval, Some(false));
    assert!(
        low.reasons[0].contains("parity outside ranked maps"),
        "{:?}",
        low.reasons
    );
}

#[tokio::test]
async fn kev_state_that_breaks_its_spec_is_unknown_not_guessed() {
    let d = repo();
    let r = run(
        &FakeKev(0.9),
        &Config::from_env(),
        d.path(),
        "map-score",
        &json!({"mode": "wrong_state"}),
    )
    .await;
    assert_eq!(r.approval, None, "{:?}", r.reasons);
    assert!(
        r.reasons
            .iter()
            .any(|x| x.contains("does not start with a ranked_range term")),
        "{:?}",
        r.reasons
    );
}

#[tokio::test]
async fn high_findings_block_and_bad_output_is_an_error() {
    let d = repo();
    let cfg = Config::from_env();
    let r = run(
        &FakeKev(0.9),
        &cfg,
        d.path(),
        "map-score",
        &json!({"mode": "bad"}),
    )
    .await;
    assert_eq!(r.approval, Some(false));
    assert!(
        r.reasons
            .iter()
            .any(|x| x.contains("parity break at beat 12") && x.contains("Expert.dat:3"))
    );
    let g = run(
        &FakeKev(0.9),
        &cfg,
        d.path(),
        "map-score",
        &json!({"mode": "garbage"}),
    )
    .await;
    assert!(g.error.unwrap().contains("not one JSON object"));
    assert_eq!(g.approval, None);
    let m = run(&FakeKev(0.9), &cfg, d.path(), "map-score", &json!({})).await;
    assert!(m.error.unwrap().contains("missing required args: mode"));
    let n = run(&FakeKev(0.9), &cfg, d.path(), "nope", &json!({})).await;
    assert!(n.error.unwrap().contains("no plugin 'nope'"));
}
