use kritic::{config::Config, native::web};
use serde_json::json;

#[tokio::test]
#[ignore = "explicit headless browser regression; set KRITIC_EFFECT_BROWSER to an installed Chromium executable"]
async fn focus_scroll_and_dead_control_effects() {
    let browser = std::env::var_os("KRITIC_EFFECT_BROWSER")
        .expect("KRITIC_EFFECT_BROWSER is required for this explicit browser regression");
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("effects.html");
    std::fs::write(&page, r##"<!doctype html><title>Native effects</title>
<button id="focus" onclick="document.getElementById('content').focus({preventScroll:true})">Move focus</button>
<button onclick="document.getElementById('panel').scrollTop=120">Scroll panel</button>
<button>Does nothing</button><a href="#content">Fragment link</a>
<main id="content" tabindex="-1">Main content</main>
<div id="panel" style="height:60px;overflow:auto"><div style="height:500px">Scrollable content</div></div>
<div style="height:1200px"></div><button>Offscreen does nothing</button>"##).unwrap();
    let mut cfg = Config::load();
    cfg.browser = Some(browser.into());
    cfg.runs_dir = dir.path().join("runs");
    let url = reqwest::Url::from_file_path(page).unwrap();
    let result = web::ui_test(
        &cfg,
        &json!({
            "url": url.as_str(), "out_dir": dir.path().join("report"), "max_pages": 1,
            "max_actions": 10, "axe": false, "wait_ms": 20, "timeout_ms": 10000
        }),
    )
    .await;
    assert_eq!(result["ok"], true, "{result}");
    let interactions = result["interactions"].as_array().unwrap();
    for (name, expected) in [
        ("Move focus", "ok"),
        ("Scroll panel", "ok"),
        ("Does nothing", "no_effect"),
        ("Offscreen does nothing", "no_effect"),
        ("Fragment link", "navigated"),
    ] {
        let interaction = interactions
            .iter()
            .find(|i| i["element"]["name"] == name)
            .unwrap_or_else(|| panic!("missing {name}: {result}"));
        assert_eq!(interaction["result"], expected, "{name}: {result}");
    }
}
