use kritic::config::Config;
use kritic::kev::{Decider, Decision, Question};
use kritic::tools::{ValidateInput, artifact_validate};
use serde_json::{Value, json};
use std::collections::BTreeMap;

struct NoModel;
impl Decider for NoModel {
    async fn decide(&self, _: &Value, _: &BTreeMap<String, Question>) -> Result<Decision, String> {
        Err("model intentionally absent".into())
    }
}

#[tokio::test]
async fn skipped_artifacts_and_blank_commands_cannot_pass() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("image.png"), [0, 1, 2]).unwrap();
    let paths = vec!["image.png".into()];
    let r = artifact_validate(
        &NoModel,
        &Config::load(),
        ValidateInput {
            content: None,
            kind: None,
            paths: &paths,
            base_dir: dir.path().to_str(),
            commands: &[],
            task: "",
            use_model: false,
            revision: 0,
        },
    )
    .await;
    assert_eq!(r.approval, None);
    let commands = vec!["".into()];
    let r = artifact_validate(
        &NoModel,
        &Config::load(),
        ValidateInput {
            content: None,
            kind: None,
            paths: &[],
            base_dir: dir.path().to_str(),
            commands: &commands,
            task: "",
            use_model: false,
            revision: 0,
        },
    )
    .await;
    assert_eq!(r.approval, None);
}

#[test]
fn no_browser_pages_and_no_desktop_evidence_stay_unknown() {
    let report = kritic::ui::HarnessReport {
        ok: true,
        ..Default::default()
    };
    assert_eq!(
        kritic::ui::judge(
            report,
            &kritic::ui::UiOptions {
                axe_blocking: true,
                ignore: vec![]
            }
        )
        .2,
        None
    );
    assert_eq!(
        kritic::phase3::judge_desktop(&json!({"ok":true}), &[]).2,
        None
    );
    assert_eq!(kritic::unity::nunit_approval(0, 0, 0, 0), None);
    assert_eq!(kritic::unity::nunit_approval(3, 0, 0, 3), None);
    assert_eq!(kritic::unity::nunit_approval(3, 2, 0, 1), None);
    assert_eq!(kritic::unity::nunit_approval(3, 3, 0, 0), Some(true));
    assert_eq!(kritic::unity::nunit_approval(3, 2, 1, 0), Some(false));
}

#[test]
fn missing_plugin_verdict_and_uncalibrated_gates_cannot_pass() {
    let mut manifest: kritic::plugins::Manifest = serde_json::from_value(
        json!({"name":"fixture","description":"vacuity test","command":["echo"]}),
    )
    .unwrap();
    assert_eq!(
        kritic::plugins::judge(
            &manifest,
            &json!({"ok":true,"findings":[]}),
            &BTreeMap::new()
        )
        .0,
        None
    );
    manifest.kev = Some(kritic::spec::get("code_chunk").for_model("cloud:test:kev-4b"));
    assert_eq!(
        kritic::plugins::judge(
            &manifest,
            &json!({"ok":true,"approval":true,"findings":[]}),
            &BTreeMap::new()
        )
        .0,
        None
    );
    assert_eq!(
        kritic::plugins::judge(
            &manifest,
            &json!({"ok":true,"approval":false,"findings":[]}),
            &BTreeMap::new()
        )
        .0,
        Some(false)
    );
}

#[test]
fn shipped_skill_starts_with_testing_mandate() {
    let skill = kritic::cli::register::SKILL_MD;
    let header = skill.split("## Review workflow").next().unwrap();
    assert!(header.contains("TEST RELEASE"));
    assert!(header.contains("do not inherently trust"));
    assert!(header.contains("fix Kritic directly"));
    assert!(header.contains("open a focused PR"));
    assert!(header.contains("viability"));
}

#[tokio::test]
async fn quality_requires_evidence_and_display_limits_do_not_hide_violations() {
    use kritic::phase3::{QualityOptions, quality};
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::load();
    cfg.max_hotspots = 0;
    let opts = || QualityOptions {
        max_complexity: 1,
        max_function_lines: 2,
        run_linters: false,
        use_kev: false,
    };
    let empty = quality(&NoModel, &cfg, dir.path(), &[], opts()).await;
    assert_eq!(empty.approval, None);
    assert_eq!(empty.quality_score, None);
    let files = vec![(
        "long.py".into(),
        "def long(x):\n    if x:\n        return 1\n    return 0\n".into(),
    )];
    let actual = quality(&NoModel, &cfg, dir.path(), &files, opts()).await;
    assert!(actual.hotspots.is_empty());
    assert_eq!(actual.approval, Some(false));
}

#[test]
fn unavailable_resource_measurements_are_not_success() {
    use kritic::phase3::{Budgets, resource_budget_gaps};
    let stats = json!({"cpu_avg_pct": 0, "rss_peak_mb": 1, "duration_s": 2,
        "rss_slope_mb_per_min": 0, "handles_start": 1, "handles_end": 1, "samples": 4});
    let b = Budgets {
        max_gpu_util_avg_pct: Some(90.0),
        ..Default::default()
    };
    let gaps = resource_budget_gaps(&stats, &b);
    assert!(gaps.iter().any(|g| g.contains("gpu_util_avg_pct")));
    assert!(gaps.iter().any(|g| g.contains("15 seconds")));
}
