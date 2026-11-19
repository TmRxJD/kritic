use super::*;

fn fixture() -> (Manifest, Request) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::var_os("KEV_TOOLS_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("kev-versions-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("newest-final.txt");
    let current = dir.join("plain.txt");
    std::fs::write(&current, b"current schema-enforced information").unwrap();
    // An old copy written later must never outrank the canonical revision.
    std::fs::write(&old, b"old information").unwrap();
    let m = serde_json::from_value(json!({"schema_version":1,"scope":"project/production/config",
        "required_data_schema":"settings/v2",
        "authority":{"path":current,"source":"reviewed canonical config","revision":"rev-new",
            "sha256":file_hash(&current).unwrap(),"observed_at":1000,"max_age_seconds":60},
        "candidates":[
            {"id":"old","scope":"project/production/config","path":old,"source":"archived config",
                "revision":"rev-old","sha256":file_hash(&old).unwrap(),"data_schema":"settings/v2"},
            {"id":"current","scope":"project/production/config","path":current,"source":"canonical config",
                "revision":"rev-new","sha256":file_hash(&current).unwrap(),"data_schema":"settings/v2","supersedes":["old"]}
        ]})).unwrap();
    let r = serde_json::from_value(
        json!({"action":"evaluate","manifest_path":dir.join("versions.json")}),
    )
    .unwrap();
    (m, r)
}

#[test]
fn current_is_selected_by_authority_and_content_not_name_or_mtime() {
    let (m, r) = fixture();
    let report = evaluate(&m, &r, 1001).unwrap();
    assert_eq!(report["status"], "ready");
    assert_eq!(report["selected"]["id"], "current");
    assert_eq!(report["candidates"][0]["classification"], "stale");
}

#[test]
fn historical_selection_is_explicit_and_hash_checked() {
    let (mut m, mut r) = fixture();
    r.mode = Mode::Historical;
    r.candidate_id = Some("old".into());
    assert!(evaluate(&m, &r, 1001).is_err());
    r.reason = Some("reproduce a regression at rev-old".into());
    assert_eq!(evaluate(&m, &r, 1001).unwrap()["selected"]["id"], "old");
    assert_eq!(
        evaluate(&m, &r, 1001).unwrap()["selected"]["historical"],
        true
    );
    std::fs::write(&m.candidates[0].path, "tampered").unwrap();
    assert_eq!(evaluate(&m, &r, 1001).unwrap()["status"], "blocked");
    m.candidates[0].sha256 = file_hash(&m.candidates[0].path).unwrap();
    m.candidates[0].data_schema = "other-contract".into();
    assert_eq!(evaluate(&m, &r, 1001).unwrap()["status"], "blocked");
}

#[test]
fn stale_future_or_changed_authority_cannot_approve_current() {
    let (m, r) = fixture();
    for time in [999, 1061] {
        let report = evaluate(&m, &r, time).unwrap();
        assert_eq!(report["status"], "blocked");
        assert!(report["selected"].is_null());
    }
    std::fs::write(&m.authority.path, b"changed upstream").unwrap();
    assert_eq!(evaluate(&m, &r, 1001).unwrap()["status"], "blocked");
}

#[test]
fn alternate_scopes_cycles_and_unknown_relationships_are_rejected() {
    let (mut m, r) = fixture();
    m.candidates[0].scope = "other-project".into();
    assert!(evaluate(&m, &r, 1001).is_err());
    m.candidates[0].scope = m.scope.clone();
    m.candidates[0].supersedes = vec!["current".into()];
    assert!(evaluate(&m, &r, 1001).is_err());
    m.candidates[0].supersedes = vec!["invented".into()];
    assert!(evaluate(&m, &r, 1001).is_err());
}

#[test]
fn ambiguous_heads_and_revision_collisions_block_selection() {
    let (mut m, r) = fixture();
    let mut copy: Candidate = serde_json::from_value(json!({"id":"alias","scope":m.scope,
        "path":m.candidates[1].path,"revision":m.authority.revision,"source":"alias",
        "sha256":m.authority.sha256,"data_schema":m.required_data_schema}))
    .unwrap();
    m.candidates.push(copy);
    assert_eq!(evaluate(&m, &r, 1001).unwrap()["status"], "blocked");
    copy = m.candidates.pop().unwrap();
    copy.revision = "newer-branch".into();
    copy.supersedes = vec!["current".into()];
    m.candidates.push(copy);
    assert_eq!(evaluate(&m, &r, 1001).unwrap()["status"], "blocked");
    m.candidates.pop();
    m.candidates[0].revision = m.authority.revision.clone();
    assert_eq!(evaluate(&m, &r, 1001).unwrap()["status"], "blocked");
}

#[test]
fn missing_files_and_incompatible_schemas_never_select() {
    let (mut m, r) = fixture();
    m.candidates[1].path = PathBuf::from("missing-relative-path");
    assert!(evaluate(&m, &r, 1001).is_err());
    m.candidates[1].path = m.authority.path.with_file_name("missing.txt");
    assert_eq!(evaluate(&m, &r, 1001).unwrap()["status"], "blocked");
    m.candidates[1].path = m.authority.path.clone();
    m.candidates[1].data_schema = "old-schema".into();
    assert_eq!(evaluate(&m, &r, 1001).unwrap()["status"], "blocked");
}

#[test]
fn public_entry_point_and_schema_reject_invalid_input() {
    assert!(call(&json!({"action":"evaluate"})).is_err());
    assert!(call(&json!({"action":"schema","ignore_errors":true})).is_err());
    assert!(call(&json!({"action":"schema","use_kev":true})).is_err());
    assert_eq!(
        serde_json::from_str::<Value>(&call(&json!({"action":"schema"})).unwrap()).unwrap()["properties"]
            ["schema_version"]["const"],
        1
    );
    let (m, r) = fixture();
    let path = r.manifest_path.unwrap();
    let raw =
        json!({"schema_version":1,"scope":"x","authority":{},"candidates":[],"invented":true});
    std::fs::write(&path, raw.to_string()).unwrap();
    assert!(call(&json!({"action":"evaluate","manifest_path":path})).is_err());
    assert!(file_hash(&m.authority.path).is_ok());
}
