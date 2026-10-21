//! CLI helpers: config-file edits that must keep the user's own content, hardware detection,
//! and a full register -> uninstall round trip in a sandboxed home.

use kritic::cli::detect::{Gpu, laya_asset, laya_backend, parse_nvidia_smi};
use kritic::cli::register::{
    remove_block, upsert_block, yaml_get_scalar, yaml_remove_child, yaml_upsert_child,
    yaml_upsert_scalar,
};

#[test]
fn claude_md_block_is_idempotent_and_removable() {
    let original = "# My rules\n\nBe terse.\n";
    let once = upsert_block(original, "RULE v1");
    assert!(once.starts_with(original) && once.contains("RULE v1"));
    let twice = upsert_block(&once, "RULE v2");
    assert!(
        twice.contains("RULE v2") && !twice.contains("RULE v1"),
        "replaced, not appended"
    );
    assert_eq!(twice.matches("kritic:begin").count(), 1);
    assert_eq!(remove_block(&twice), original);
    assert_eq!(upsert_block("", "R").lines().count(), 3);
}

#[test]
fn hermes_yaml_edits_keep_comments_and_other_servers() {
    let cfg = "# my hermes config\nmodel:\n  default: claude-sonnet-5  # comment\n\nmcp_servers:\n    github:\n        command: \"npx\"\n        args: [\"-y\", \"gh\"]\n\ncontext:\n    engine: compressor\n";
    let body = vec![
        "command: \"C:/k/kritic.exe\"".to_string(),
        "args: []".to_string(),
    ];
    let a = yaml_upsert_child(cfg, "mcp_servers", "kritic", &body).unwrap();
    assert!(a.contains("# my hermes config") && a.contains("# comment") && a.contains("github:"));
    assert!(
        a.contains("    kritic:\n        command: \"C:/k/kritic.exe\""),
        "matches the file's 4-space indent:\n{a}"
    );
    let v: serde_yaml::Value = serde_yaml::from_str(&a).unwrap();
    assert_eq!(v["mcp_servers"]["kritic"]["command"], "C:/k/kritic.exe");
    assert_eq!(v["mcp_servers"]["github"]["command"], "npx");
    // Replacing is idempotent.
    let b = yaml_upsert_child(&a, "mcp_servers", "kritic", &body).unwrap();
    assert_eq!(a, b);
    // Scalar in an existing section.
    let c = yaml_upsert_scalar(&b, "context", "engine", "kritic").unwrap();
    assert_eq!(
        yaml_get_scalar(&c, "context", "engine").as_deref(),
        Some("kritic")
    );
    assert!(!c.contains("compressor"));
    // Removal leaves everything else.
    let d = yaml_remove_child(&c, "mcp_servers", "kritic");
    let v: serde_yaml::Value = serde_yaml::from_str(&d).unwrap();
    assert!(v["mcp_servers"].get("kritic").is_none() && v["mcp_servers"].get("github").is_some());
    // Missing sections are created; an empty file works.
    let e = yaml_upsert_child("", "mcp_servers", "kritic", &body).unwrap();
    assert_eq!(
        serde_yaml::from_str::<serde_yaml::Value>(&e).unwrap()["mcp_servers"]["kritic"]["args"],
        serde_yaml::Value::Sequence(vec![])
    );
    let f = yaml_upsert_scalar("model:\n  default: x\n", "context", "engine", "kritic").unwrap();
    assert_eq!(
        yaml_get_scalar(&f, "context", "engine").as_deref(),
        Some("kritic")
    );
    // An inline mapping is refused, not rewritten.
    assert!(yaml_upsert_child("mcp_servers: {}\n", "mcp_servers", "kritic", &body).is_err());
}

#[test]
fn gpu_detection_picks_a_runnable_laya_build() {
    let g = parse_nvidia_smi("NVIDIA GeForce RTX 5070, 12227, 12.0\n").unwrap();
    assert_eq!(
        g,
        Gpu::Nvidia {
            name: "NVIDIA GeForce RTX 5070".into(),
            vram_mb: 12227,
            compute: (12, 0)
        }
    );
    assert!(g.fits_local_kev());
    assert_eq!(
        laya_backend(&g),
        "cuda-sm89",
        "sm89 PTX JIT-compiles forward to sm120"
    );
    let ampere = parse_nvidia_smi("RTX 3060, 12288, 8.6").unwrap();
    assert_eq!(laya_backend(&ampere), "cuda-sm86");
    let a100 = parse_nvidia_smi("A100, 40960, 8.0").unwrap();
    assert_eq!(laya_backend(&a100), "cuda-sm80");
    let turing = parse_nvidia_smi("GTX 1660, 6144, 7.5").unwrap();
    assert_eq!(
        laya_backend(&turing),
        "vulkan",
        "no CUDA build runs below sm80"
    );
    assert!(
        !parse_nvidia_smi("GT 1030, 2048, 6.1")
            .unwrap()
            .fits_local_kev()
    );
    assert!(parse_nvidia_smi("garbage").is_none());
    assert_eq!(laya_backend(&Gpu::Unknown), "vulkan");
    let asset = laya_asset("cuda-sm89");
    assert!(asset.starts_with("laya-") && (asset.ends_with(".zip") || asset.ends_with(".tar.gz")));
}

/// Register and uninstall in a sandbox: HOME, the Claude and Hermes folders and PATH all point
/// into a temp dir, so nothing on the machine is touched. The only test that sets the environment.
#[test]
fn register_then_uninstall_round_trip() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join(".hermes")).unwrap();
    std::fs::write(
        home.join(".claude.json"),
        "{\"theme\": \"dark\", \"mcpServers\": {\"other\": {\"command\": \"x\"}}}",
    )
    .unwrap();
    std::fs::write(home.join(".claude").join("CLAUDE.md"), "# Mine\n").unwrap();
    std::fs::write(
        home.join(".hermes").join("config.yaml"),
        "# hermes\nmodel:\n  default: m\n",
    )
    .unwrap();
    // SAFETY: the only test in this binary that touches the environment.
    unsafe {
        std::env::set_var("USERPROFILE", home);
        std::env::set_var("HOME", home);
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::set_var("HERMES_HOME", home.join(".hermes"));
        std::env::set_var("PATH", home.join("empty-path"));
        std::env::set_var("KRITIC_HOME", home.join(".kritic"));
    }
    let exe = home.join("bin").join("kritic.exe");
    let khome = home.join(".kritic");
    kritic::cli::register::register_claude(
        &khome,
        &exe,
        &kritic::cli::register::ClaudeOptions {
            rule: true,
            concise: false,
        },
    )
    .unwrap();
    kritic::cli::register::register_hermes(
        &khome,
        &exe,
        &kritic::cli::register::HermesOptions {
            context_engine: true,
            concise: false,
        },
    )
    .unwrap();

    let cj: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(cj["theme"], "dark");
    assert_eq!(cj["mcpServers"]["other"]["command"], "x");
    assert!(
        cj["mcpServers"]["kritic"]["command"]
            .as_str()
            .unwrap()
            .ends_with("bin/kritic.exe")
    );
    let keys: Vec<&String> = cj.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["theme", "mcpServers"], "key order preserved");
    assert!(home.join(".claude/skills/kritic/SKILL.md").is_file());
    assert_eq!(
        std::fs::read_to_string(home.join(".claude/skills/kritic/references/setup.md")).unwrap(),
        kritic::cli::register::SETUP_REFERENCE
    );
    assert!(home.join(".claude/agents/ux-critic.md").is_file());
    assert!(
        std::fs::read_to_string(home.join(".claude/CLAUDE.md"))
            .unwrap()
            .starts_with("# Mine\n")
    );
    let hy = std::fs::read_to_string(home.join(".hermes/config.yaml")).unwrap();
    assert!(hy.starts_with("# hermes\n"));
    assert_eq!(
        yaml_get_scalar(&hy, "context", "engine").as_deref(),
        Some("kritic")
    );
    assert!(home.join(".hermes/plugins/kritic/__init__.py").is_file());
    assert!(
        khome.join("backups").read_dir().unwrap().count() >= 2,
        "originals backed up"
    );

    let done = kritic::cli::register::unregister_claude(&khome);
    assert!(done.len() >= 3, "{done:?}");
    let done = kritic::cli::register::unregister_hermes(&khome);
    assert!(done.len() >= 2, "{done:?}");
    let cj: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
    assert!(
        cj["mcpServers"].get("kritic").is_none() && cj["mcpServers"]["other"]["command"] == "x"
    );
    assert_eq!(
        std::fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap(),
        "# Mine\n"
    );
    assert!(
        !home.join(".claude/skills/kritic").exists()
            && !home.join(".hermes/plugins/kritic").exists()
    );
    let hy = std::fs::read_to_string(home.join(".hermes/config.yaml")).unwrap();
    assert_eq!(
        yaml_get_scalar(&hy, "context", "engine").as_deref(),
        Some("compressor")
    );
    assert!(
        serde_yaml::from_str::<serde_yaml::Value>(&hy).unwrap()["mcp_servers"]
            .get("kritic")
            .is_none()
    );
}
