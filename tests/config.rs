//! Config file + environment precedence, and the shipped examples. Only the first test touches
//! the environment; the others read no KRITIC_* variables.

use kritic::config::{Config, DEFAULT_DISABLED, KEYS};

#[test]
fn example_documents_every_key_and_file_env_precedence_holds() {
    // examples/kritic.toml documents every key under its section, with its env variable.
    let example =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/kritic.toml"))
            .unwrap();
    for (key, env) in KEYS {
        let (section, name) = key.split_once('.').unwrap();
        let at = example
            .find(&format!("[{section}]"))
            .unwrap_or_else(|| panic!("section [{section}] missing"));
        let body = &example[at..];
        let body = &body[..body[1..].find("\n[").map_or(body.len(), |i| i + 1)];
        assert!(
            body.contains(&format!("# {name} = ")),
            "{key} not documented in examples/kritic.toml"
        );
        assert!(body.contains(env), "{env} not documented for {key}");
    }

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("kritic.toml");
    std::fs::write(
        &file,
        "[kev]\nchunk_chars = 999\nmodel = \"kev-test\"\ncpu_only = true\nexternally_managed = true\nbogus = 1\n\n[vision]\nexternally_managed = true\n\n[cloud]\nkey_env = \"TYPESAFE_API_KEY\"\n\n[tools]\ndisabled = []\n",
    )
    .unwrap();
    // SAFETY: no other test in this binary reads the environment.
    unsafe {
        for (_, env) in KEYS {
            std::env::remove_var(env);
        }
        std::env::set_var("KRITIC_HOME", dir.path());
        std::env::remove_var("KRITIC_CONFIG");
        std::env::set_var("KRITIC_KEV_MODEL", "from-env");
    }
    let c = Config::load();
    assert_eq!(c.config_file.as_deref(), Some(file.as_path()));
    assert_eq!(c.chunk_chars, 999, "file value");
    assert!(c.kev_cpu_only, "CPU-only TOML boolean is accepted");
    assert!(c.kev_externally_managed && c.vision_externally_managed);
    assert_eq!(c.cloud_key_env, "TYPESAFE_API_KEY");
    assert_eq!(c.model, "from-env", "env beats file");
    assert_eq!(c.token_budget, 2900, "default");
    assert!(c.disabled_tools.is_empty(), "file list");
    assert_eq!(c.config_errors.len(), 1, "{:?}", c.config_errors);
    assert!(c.config_errors[0].contains("kev.bogus"));
    unsafe { std::env::set_var("KRITIC_KEV_CPU_ONLY", "false") };
    assert!(!Config::load().kev_cpu_only, "environment overrides file");
    unsafe { std::env::set_var("KRITIC_KEV_CPU_ONLY", "invalid") };
    let invalid = Config::load();
    assert!(!invalid.kev_cpu_only);
    assert!(
        invalid
            .config_errors
            .iter()
            .any(|e| e.contains("kev.cpu_only: expected true or false"))
    );
    unsafe { std::env::remove_var("KRITIC_KEV_CPU_ONLY") };
    for key in [
        "KRITIC_KEV_EXTERNALLY_MANAGED",
        "KRITIC_VISION_EXTERNALLY_MANAGED",
    ] {
        unsafe { std::env::set_var(key, "false") };
        let value = Config::load();
        assert!(!if key == "KRITIC_KEV_EXTERNALLY_MANAGED" {
            value.kev_externally_managed
        } else {
            value.vision_externally_managed
        });
        unsafe { std::env::set_var(key, "invalid") };
        assert!(
            Config::load()
                .config_errors
                .iter()
                .any(|error| error.contains("externally_managed: expected true or false"))
        );
        unsafe { std::env::remove_var(key) };
    }
    assert_eq!(
        c.runs_dir,
        dir.path().join("runs"),
        "paths default under KRITIC_HOME"
    );

    // No file: defaults, map_eval disabled; env list is comma-separated.
    unsafe {
        std::env::set_var("KRITIC_HOME", dir.path().join("empty"));
        std::env::set_var("KRITIC_DISABLED_TOOLS", "route, context_trim");
    }
    let c = Config::load();
    assert!(c.config_file.is_none() && c.config_errors.is_empty());
    assert!(!c.capability("history") && !c.tool_enabled("review_history"));
    assert!(!c.capability("debt") && !c.tool_enabled("technical_debt_critic"));
    unsafe { std::env::set_var("KRITIC_CAP_HISTORY", "true") };
    assert!(Config::load().tool_enabled("review_history"));
    unsafe { std::env::remove_var("KRITIC_CAP_HISTORY") };
    assert!(!c.kev_cpu_only, "GPU behavior is the default");
    assert!(!c.kev_externally_managed && !c.vision_externally_managed);
    assert_eq!(c.disabled_tools, ["route", "context_trim"]);
    unsafe { std::env::remove_var("KRITIC_DISABLED_TOOLS") };
    assert_eq!(Config::load().disabled_tools, DEFAULT_DISABLED);

    // Set-but-empty means unset; a relative home is made absolute.
    unsafe { std::env::set_var("KRITIC_HOME", "") };
    assert!(
        kritic::config::kritic_home().ends_with(".kritic")
            && kritic::config::kritic_home().is_absolute()
    );
    unsafe { std::env::set_var("KRITIC_HOME", "relative-home") };
    assert!(kritic::config::kritic_home().is_absolute());
    unsafe { std::env::set_var("KRITIC_HOME", dir.path().join("empty")) };

    assert_eq!(Config::load().visual_quality_target, "professional");
    unsafe { std::env::set_var("KRITIC_VISUAL_QUALITY_TARGET", "aaa") };
    assert_eq!(Config::load().visual_quality_target, "showcase");
    unsafe { std::env::set_var("KRITIC_VISUAL_QUALITY_TARGET", "unknown") };
    let invalid_target = Config::load();
    assert!(
        invalid_target
            .config_errors
            .iter()
            .any(|e| e.contains("vision.quality_target"))
    );
    unsafe { std::env::remove_var("KRITIC_VISUAL_QUALITY_TARGET") };

    for value in ["NaN", "-1", "11"] {
        unsafe { std::env::set_var("KRITIC_VISUAL_MIN", value) };
        let invalid = Config::load();
        assert_eq!(invalid.visual_min, 6.0);
        assert!(
            invalid
                .config_errors
                .iter()
                .any(|e| e.contains("vision.visual_min"))
        );
    }
    unsafe { std::env::remove_var("KRITIC_VISUAL_MIN") };

    // An explicit KRITIC_CONFIG that doesn't exist is an error, not silently ignored.
    unsafe { std::env::set_var("KRITIC_CONFIG", dir.path().join("missing.toml")) };
    assert_eq!(Config::load().config_errors.len(), 1);
    let unsupported = dir.path().join("unsupported.toml");
    std::fs::write(
        &unsupported,
        "[kev]\ndevice='cuda'\ngpu_uuid='test'\nmax_batch=4\nvulkan_visible_devices='0'\n",
    )
    .unwrap();
    unsafe { std::env::set_var("KRITIC_CONFIG", unsupported) };
    let unsupported = Config::load();
    for key in [
        "kev.device",
        "kev.gpu_uuid",
        "kev.max_batch",
        "kev.vulkan_visible_devices",
    ] {
        assert!(
            unsupported
                .config_errors
                .iter()
                .any(|error| error.contains(key)),
            "{key} must not be accepted as a no-op"
        );
    }
}

#[test]
fn example_ux_specs_validate() {
    for name in ["web.ux.toml", "unity.ux.toml"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/ux")
            .join(name);
        kritic::ux::load_spec(&path).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

#[test]
fn example_plugin_manifest_is_discovered() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugin-repo");
    let (plugins, errors) = kritic::plugins::discover(&base);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        plugins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["todo-budget"]
    );
}
