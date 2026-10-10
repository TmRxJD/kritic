//! ux_critic continuity: a screen has to be the same screen every time it is reached, nothing
//! may be drawn over anything else, and text the spec calls volatile must not make a new screen.

use std::time::{Duration, Instant};

use kritic::ux::{self, Adapter, Control, Obs, Options, Run};
use serde_json::Value;

const SPEC: &str = r#"
volatile = ["^clock "]
[app]
kind = "web"
purpose = "test app"
url = "http://localhost/"
[[tasks]]
name = "open"
goal = "Open the page"
done = { text = "Page" }
"#;

/// Two screens, home and page. `leak` makes a banner appear on every screen from the second reset on
/// (state carried over between visits); `clock` puts a ticking clock text on every observation.
struct App {
    at: &'static str,
    resets: usize,
    leak: bool,
    clock: bool,
    ticks: usize,
}

impl App {
    fn new(leak: bool, clock: bool) -> Self {
        App {
            at: "home",
            resets: 0,
            leak,
            clock,
            ticks: 0,
        }
    }
}

fn button(label: &str, rect: [f64; 4]) -> Control {
    Control {
        id: format!("button:{label}#1"),
        label: label.into(),
        text: label.into(),
        role: "button".into(),
        tooltip: "t".into(),
        enabled: true,
        rect,
        ..Default::default()
    }
}

impl Adapter for App {
    async fn reset(&mut self) -> Result<(), String> {
        self.at = "home";
        self.resets += 1;
        Ok(())
    }

    async fn observe(&mut self, _opts: Value) -> Result<Obs, String> {
        self.ticks += 1;
        let mut controls = if self.at == "home" {
            vec![button("Open", [10.0, 10.0, 80.0, 30.0])]
        } else {
            vec![button("Back", [10.0, 10.0, 80.0, 30.0])]
        };
        if self.leak && self.resets >= 2 {
            let mut banner = button("Banner", [300.0, 10.0, 80.0, 30.0]);
            banner.skip = Some("ignored".into());
            controls.push(banner);
        }
        let mut texts = vec![if self.at == "home" {
            "Home".to_string()
        } else {
            "Page".to_string()
        }];
        if self.clock {
            texts.push(format!("clock {}", self.ticks));
        }
        // the fingerprint is the screen name: the structural comparison must not depend on it
        Ok(Obs {
            fingerprint: self.at.into(),
            text: texts.join(" "),
            texts,
            viewport: [1200.0, 900.0],
            controls,
            ..Default::default()
        })
    }

    async fn act(&mut self, id: &str) -> Result<(), String> {
        match (self.at, id) {
            ("home", "button:Open#1") => self.at = "page",
            ("page", "button:Back#1") => self.at = "home",
            _ => return Err(format!("no {id}")),
        }
        Ok(())
    }
}

fn opts() -> Options {
    Options {
        max_depth: 4,
        max_actions: 50,
        runs: 1,
        deadline: Instant::now() + Duration::from_secs(30),
        out_dir: std::env::temp_dir(),
    }
}

#[tokio::test]
async fn a_screen_that_changes_between_visits_is_a_finding_and_fails_the_run() {
    let spec = ux::parse_spec(SPEC, "t.toml").unwrap();
    let mut app = App::new(true, false);
    let mut run = Run::new(&spec, opts());
    run.explore(&mut app).await;
    let r = run.finish(false);
    assert!(!r.continuity.is_empty(), "{:?}", r.exploration.problems);
    let first = &r.continuity[0];
    assert_eq!(first.kind, "replay_differs");
    assert!(
        first.added.iter().any(|a| a.contains("Banner")),
        "{:?}",
        first.added
    );
    assert!(
        r.hard_fails
            .iter()
            .any(|h| h.contains("not the same from reset to reset")),
        "hard {:?} continuity {} problems {:?} screens {}",
        r.hard_fails,
        r.continuity.len(),
        r.exploration.problems,
        r.screens.len()
    );
    assert_eq!(r.approval, Some(false));
}

#[tokio::test]
async fn a_stable_app_has_no_continuity_findings() {
    let spec = ux::parse_spec(SPEC, "t.toml").unwrap();
    let mut app = App::new(false, false);
    let mut run = Run::new(&spec, opts());
    run.explore(&mut app).await;
    let r = run.finish(false);
    assert!(r.continuity.is_empty(), "{:?}", r.continuity);
    assert!(
        !r.hard_fails.iter().any(|h| h.contains("reset to reset")),
        "{:?}",
        r.hard_fails
    );
}

#[tokio::test]
async fn volatile_text_is_not_a_difference() {
    let spec = ux::parse_spec(SPEC, "t.toml").unwrap();
    let mut app = App::new(false, true);
    let mut run = Run::new(&spec, opts());
    run.explore(&mut app).await;
    let r = run.finish(false);
    assert!(
        r.continuity.is_empty(),
        "a ticking clock the spec names volatile was reported: {:?}",
        r.continuity
    );
}

#[tokio::test]
async fn volatile_patterns_are_validated() {
    let bad = SPEC.replace("^clock ", "([");
    let errors = ux::parse_spec(&bad, "t.toml").unwrap_err();
    assert!(
        errors.iter().any(|e| e.contains("volatile[0]")),
        "{errors:?}"
    );
}

fn at(id: &str, rect: [f64; 4]) -> Control {
    let mut c = button(id, rect);
    c.id = id.into();
    c
}

#[test]
fn overlapping_and_off_screen_controls_are_found() {
    let controls = vec![
        at("a", [10.0, 10.0, 100.0, 30.0]),
        at("b", [60.0, 12.0, 100.0, 30.0]), // half over a
        at("frame", [300.0, 10.0, 200.0, 200.0]),
        at("inside", [320.0, 30.0, 40.0, 30.0]), // a part of the frame, not a collision
        at("clipped", [1150.0, 10.0, 100.0, 30.0]), // reaches past a 1200 px window
        at("fine", [600.0, 10.0, 50.0, 30.0]),
    ];
    let hits: Vec<(String, String)> = ux::audit(&controls, &[], [1200.0, 900.0])
        .into_iter()
        .map(|(r, _, k, _)| (r.to_string(), k))
        .collect();
    let has = |r: &str, k: &str| hits.iter().any(|(rule, key)| rule == r && key.contains(k));
    assert!(has("overlap", "a x b"), "{hits:?}");
    assert!(
        !has("overlap", "inside") && !has("overlap", "frame"),
        "{hits:?}"
    );
    assert!(
        has("offscreen", "clipped") && !has("offscreen", "fine"),
        "{hits:?}"
    );
}
