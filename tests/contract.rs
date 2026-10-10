use kritic::config::Config;
use kritic::contract::{Outcome, critique, hash, parse};

fn cfg(dir: &std::path::Path) -> Config {
    let mut c = Config::load();
    c.runs_dir = dir.join("runs");
    c.command_timeout_s = 3;
    c
}

fn write_contract(dir: &std::path::Path, text: &str) {
    std::fs::create_dir_all(dir.join(".kritic")).unwrap();
    std::fs::write(dir.join(".kritic/contract.toml"), text).unwrap();
}

#[tokio::test]
async fn distinguishes_verification_enforcement_and_uncovered_work() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "Never publish without approval.",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("ci.yml"),
        "run: cargo test --test acceptance",
    )
    .unwrap();
    write_contract(
        dir.path(),
        r#"
version=1
title="Acceptance"
[[requirements]]
id="tests"
description="Validation passes and is wired into CI"
require_enforcement=true
[[requirements.checks]]
kind="command"
command="echo acceptance"
[[requirements.enforcement]]
path="ci.yml"
pattern="cargo test --test acceptance"
[[requirements]]
id="approval"
description="Human approved publishing"
"#,
    );
    let r = critique(&cfg(dir.path()), dir.path(), None, "prepare release", false).await;
    assert_eq!(r.approval, None);
    assert_eq!(r.requirements[0].enforcement, "configured");
    assert_eq!(r.requirements[0].outcome, Outcome::Unverified);
    let r = critique(&cfg(dir.path()), dir.path(), None, "prepare release", true).await;
    assert_eq!(r.requirements[0].outcome, Outcome::Verified);
    assert_eq!(r.requirements[1].outcome, Outcome::Unverified);
    assert_eq!(r.approval, None);
    assert_eq!(r.coverage, 0.5);
    assert_eq!(r.sources[0].status, "prose_unverified");
    std::fs::write(dir.path().join("ci.yml"), "no validator").unwrap();
    let r = critique(&cfg(dir.path()), dir.path(), None, "", true).await;
    assert_eq!(r.approval, Some(false));
    assert_eq!(r.requirements[0].enforcement, "missing");
}

#[tokio::test]
async fn rejects_stale_report_input_and_failing_command() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("input.txt"), "current").unwrap();
    std::fs::write(dir.path().join("report.json"), "{\"approval\":true}").unwrap();
    write_contract(
        dir.path(),
        &format!(
            r#"
version=1
title="Report evidence"
[[requirements]]
id="report"
description="Check passed on the current input"
[[requirements.checks]]
kind="report"
path="report.json"
pointer="/approval"
expected=true
inputs={{"input.txt"="{}"}}
"#,
            hash(b"current")
        ),
    );
    assert_eq!(
        critique(&cfg(dir.path()), dir.path(), None, "", false)
            .await
            .approval,
        Some(true)
    );
    std::fs::write(dir.path().join("input.txt"), "changed").unwrap();
    let r = critique(&cfg(dir.path()), dir.path(), None, "", false).await;
    assert_eq!(r.approval, None);
    assert!(r.requirements[0].checks[0].detail.contains("stale"));
    write_contract(
        dir.path(),
        r#"version=1
title="Command"
[[requirements]]
id="fail"
description="Command succeeds"
[[requirements.checks]]
kind="command"
command="exit 3"
"#,
    );
    assert_eq!(
        critique(&cfg(dir.path()), dir.path(), None, "", true)
            .await
            .approval,
        Some(false)
    );
}

#[tokio::test]
async fn empty_contract_missing_evidence_and_escape_never_pass() {
    assert!(parse("version=1\ntitle='empty'\nrequirements=[]").is_err());
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    write_contract(
        dir.path(),
        &format!(
            "version=1\ntitle='Escape'\n[[requirements]]\nid='x'\ndescription='read evidence'\n[[requirements.checks]]\nkind='file_exists'\npath='{}'\n",
            outside.path().to_string_lossy().replace('\\', "/")
        ),
    );
    let r = critique(&cfg(dir.path()), dir.path(), None, "", false).await;
    assert_eq!(r.approval, Some(false));
    assert!(r.requirements[0].checks[0].detail.contains("escapes"));
    assert_eq!(
        critique(
            &cfg(dir.path()),
            dir.path(),
            Some("missing.toml"),
            "",
            false
        )
        .await
        .approval,
        None
    );
}

#[tokio::test]
async fn later_commands_cannot_certify_stale_assertions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("state.txt"), "good").unwrap();
    write_contract(
        dir.path(),
        r#"version=1
title="Mutation during verification"
[[requirements]]
id="stable"
description="State remains good after validation"
[[requirements.checks]]
kind="content"
path="state.txt"
pattern="^good$"
[[requirements.checks]]
kind="command"
command="python -c \"from pathlib import Path; Path('state.txt').write_text('bad')\""
"#,
    );
    let r = critique(&cfg(dir.path()), dir.path(), None, "", true).await;
    assert_ne!(r.approval, Some(true));
    if r.requirements[0].checks[1].command.as_ref().unwrap().status == kritic::checks::Status::Pass
    {
        assert_eq!(r.requirements[0].checks[0].outcome, Outcome::Unverified);
        assert!(r.requirements[0].checks[0].detail.contains("changed"));
    } else {
        panic!(
            "mutation reproducer must execute successfully: {:?}",
            r.requirements[0].checks[1]
        );
    }
}

#[tokio::test]
async fn broad_patterns_and_all_optional_requirements_do_not_approve() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("state.txt"), "anything").unwrap();
    write_contract(
        dir.path(),
        r#"version=1
title="Vacuity"
[[requirements]]
id="x"
description="behavior works"
[[requirements.checks]]
kind="content"
path="state.txt"
pattern=".*"
"#,
    );
    assert_eq!(
        critique(&cfg(dir.path()), dir.path(), None, "", false)
            .await
            .approval,
        None
    );
    write_contract(
        dir.path(),
        r#"version=1
title="Optional only"
[[requirements]]
id="x"
description="file exists"
required=false
[[requirements.checks]]
kind="file_exists"
path="state.txt"
"#,
    );
    assert_eq!(
        critique(&cfg(dir.path()), dir.path(), None, "", false)
            .await
            .approval,
        None
    );
}
