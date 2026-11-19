use kritic_continuity::{Request, evidence, handoff_critic, session_critic};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

struct Fixture {
    root: PathBuf,
    case: Value,
    handoff: Value,
}

fn write(root: &Path, path: &str, value: &[u8]) -> Value {
    std::fs::write(root.join(path), value).unwrap();
    json!({"path":path,"sha256":evidence::digest(value)})
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::var_os("KRITIC_CONTINUITY_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "continuity-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(root.join(".state")).unwrap();
        evidence::git(&root, &["init", "-q", "-b", "main"]).unwrap();
        evidence::git(&root, &["config", "user.name", "Continuity Fixture"]).unwrap();
        evidence::git(&root, &["config", "user.email", "fixture@example.invalid"]).unwrap();
        evidence::git(&root, &["config", "core.hooksPath", ".no-hooks"]).unwrap();
        write(&root, ".gitignore", b".state/\n");
        let current = write(&root, "fixture.dat", b"current schema v2\n");
        let old = write(&root, "newest-final.dat", b"obsolete schema v1\n");
        let binding_value =
            json!({"authority":{"path":"fixture.dat","sha256":current["sha256"],"revision":"v2"}});
        let binding = write(
            &root,
            "versions.json",
            serde_json::to_string(&binding_value).unwrap().as_bytes(),
        );
        evidence::git(&root, &["add", "--all"]).unwrap();
        evidence::git(&root, &["commit", "-qm", "baseline"]).unwrap();
        let revision = String::from_utf8(evidence::git(&root, &["rev-parse", "HEAD"]).unwrap())
            .unwrap()
            .trim()
            .to_owned();
        let messages = [
            json!({"id":"u1","session_id":"source","role":"user","text":"Implement the import task. Keep active fixture fixture.dat. Do not delete scratch work."}),
            json!({"id":"u2","session_id":"source","role":"user","text":"Stop using newest-final.dat. Use fixture.dat, which replaced it."}),
            json!({"id":"t1","session_id":"source","role":"tool","text":"Snapshot v2 recorded; import reproduction still fails."}),
        ];
        let capture_bytes = messages
            .iter()
            .map(|v| serde_json::to_string(v).unwrap() + "\n")
            .collect::<String>();
        let capture = write(&root, ".state/capture.jsonl", capture_bytes.as_bytes());
        let case = json!({"version":1,"host":"codex","session_id":"source","task_id":"imports","task_state":"active",
            "goal":"Implement the import task.","scope":["src/imports/"],"overarching_requests":["Do not delete scratch work."],
            "capture":{"file":capture,"format":"normalized_jsonl","complete":true,"gaps":[]},
            "workspace":{"revision":revision,"files":[],"deleted_paths":[]},
            "policy":{"enabled":true,"max_user_turns":2,"max_history_bytes":null,"max_context_fraction":0.6,"context_window_tokens":null,"fresh_context_max_bytes":65_536},
            "facts":[{"id":"correct-fixture","kind":"correction","event_id":"u2","quote":"Stop using newest-final.dat.","artifacts":[],"supersedes":[],"solution":{"event_id":"t1","quote":"Snapshot v2 recorded","artifacts":[current],"verification":"observed"}},
                {"id":"fix-imports","kind":"unfinished","event_id":"t1","quote":"import reproduction still fails","artifacts":[],"supersedes":[],"solution":null}],
            "fixtures":[{"id":"import-data","selected":current,"authority":current,"revision":"v2","authority_revision":"v2","binding":binding,"binding_pointer":"/authority","historical_reason":null,"authorization_event":null}]});
        let handoff = json!({"version":1,"session_case":{"path":".state/session.json","sha256":"placeholder"},"source_session_id":"source",
            "preserved_user_event_ids":["u1","u2"],"fact_ids":["correct-fixture","fix-imports"],"fixture_ids":["import-data"],"next_step":"Reproduce the import failure using fixture.dat", "uncertainties":["Root cause not yet isolated"],"continuation_questions":["Which fixture is authoritative, and which must not be reused?"]});
        let _ = old;
        Self {
            root,
            case,
            handoff,
        }
    }
    fn save(&mut self) {
        let reference = write(
            &self.root,
            ".state/session.json",
            serde_json::to_string(&self.case).unwrap().as_bytes(),
        );
        self.handoff["session_case"] = reference;
        write(
            &self.root,
            ".state/handoff.json",
            serde_json::to_string(&self.handoff).unwrap().as_bytes(),
        );
    }
    fn review(&mut self) -> Value {
        self.save();
        handoff_critic(&Request {
            base_dir: self.root.to_string_lossy().into(),
            case_path: Some(".state/handoff.json".into()),
        })
        .unwrap()
    }
    fn session(&mut self) -> Value {
        self.save();
        session_critic(&Request {
            base_dir: self.root.to_string_lossy().into(),
            case_path: Some(".state/session.json".into()),
        })
        .unwrap()
    }
}

#[test]
fn complete_handoff_preserves_corrections_and_actual_remaining_work() {
    let mut f = Fixture::new();
    let report = f.review();
    assert_eq!(report["status"], "covered");
    assert_eq!(report["automatic_transfer_allowed"], false);
    let context = report["curated_context"].as_str().unwrap();
    assert!(context.contains("Stop using newest-final.dat."));
    assert!(context.contains("import reproduction still fails"));
    assert!(context.contains("Snapshot v2 recorded"));
    assert!(context.contains("Do not delete scratch work."));
}

#[test]
fn omitted_user_correction_and_declared_fact_block_resume() {
    let mut f = Fixture::new();
    f.handoff["preserved_user_event_ids"] = json!(["u1"]);
    assert_eq!(f.review()["status"], "blocked");
    f.handoff["preserved_user_event_ids"] = json!(["u1", "u2"]);
    f.handoff["fact_ids"] = json!(["fix-imports"]);
    assert_eq!(f.review()["status"], "blocked");
}

#[test]
fn newer_filename_cannot_select_old_fixture() {
    let mut f = Fixture::new();
    f.case["fixtures"][0]["selected"] =
        json!({"path":"newest-final.dat","sha256":evidence::digest(b"obsolete schema v1\n")});
    f.case["fixtures"][0]["revision"] = json!("v1");
    assert_eq!(f.review()["status"], "blocked");
}

#[test]
fn canonical_binding_change_and_mutated_data_are_not_blessed() {
    let mut f = Fixture::new();
    std::fs::write(
        f.root.join("versions.json"),
        b"{\"authority\":{\"path\":\"another.dat\"}}",
    )
    .unwrap();
    assert_eq!(f.review()["status"], "blocked");
    let mut f = Fixture::new();
    std::fs::write(f.root.join("fixture.dat"), b"mutated current payload").unwrap();
    assert_eq!(f.review()["status"], "blocked");
}

#[test]
fn missing_capture_coverage_is_unknown_not_approval() {
    let mut f = Fixture::new();
    f.case["capture"]["complete"] = json!(false);
    assert_eq!(f.review()["status"], "unknown");
}

#[test]
fn dirty_work_is_required_and_content_changes_invalidate_snapshot() {
    let mut f = Fixture::new();
    let pin = write(&f.root, "unfinished.rs", b"requested partial work\n");
    assert_eq!(f.review()["status"], "blocked");
    f.case["workspace"]["files"] = json!([pin]);
    assert_eq!(f.review()["status"], "covered");
    std::fs::write(f.root.join("unfinished.rs"), b"changed after snapshot\n").unwrap();
    assert_eq!(f.review()["status"], "blocked");
}

#[test]
fn fresh_context_overflow_blocks_instead_of_losing_protected_context() {
    let mut f = Fixture::new();
    let raw = std::fs::read_to_string(f.root.join(".state/capture.jsonl")).unwrap();
    let mut rows: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    rows[0]["text"] = json!(format!(
        "{} {}",
        rows[0]["text"].as_str().unwrap(),
        "persistent instruction ".repeat(300)
    ));
    let raw = rows
        .iter()
        .map(|v| serde_json::to_string(v).unwrap() + "\n")
        .collect::<String>();
    f.case["capture"]["file"] = write(&f.root, ".state/capture.jsonl", raw.as_bytes());
    f.case["policy"]["fresh_context_max_bytes"] = json!(4096);
    let report = f.review();
    assert_eq!(report["status"], "blocked");
    assert!(
        report["curated_context"]
            .as_str()
            .unwrap()
            .contains("persistent instruction")
    );
}

#[test]
fn unknown_context_telemetry_is_null_and_history_threshold_is_independent() {
    let mut f = Fixture::new();
    let report = f.session();
    assert_eq!(report["rollover_due"], true);
    assert_eq!(report["metrics"]["observed_context_fraction"], Value::Null);
    assert_eq!(report["current_turn_may_finish"], true);
    f.case["task_state"] = json!("complete");
    assert_eq!(f.session()["rollover_due"], false);
}

#[test]
fn invented_solution_and_cross_role_user_correction_are_blocked() {
    let mut f = Fixture::new();
    f.case["facts"][0]["solution"]["verification"] = json!("user_confirmed");
    assert_eq!(f.review()["status"], "blocked");
    f.case["facts"][0]["quote"] = json!("the user certainly confirmed this");
    assert_eq!(f.review()["status"], "blocked");
}

#[test]
fn deleted_work_is_preserved_as_a_tombstone_not_recreated() {
    let mut f = Fixture::new();
    std::fs::remove_file(f.root.join("newest-final.dat")).unwrap();
    assert_eq!(f.review()["status"], "blocked");
    f.case["workspace"]["deleted_paths"] = json!(["newest-final.dat"]);
    assert_eq!(f.review()["status"], "covered");
    assert!(!f.root.join("newest-final.dat").exists());
}
