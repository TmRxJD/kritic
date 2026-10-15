use kritic::organization::{Layer, LineBaseline, NamingRule, OrganizationPolicy, audit, validate};

fn files(path: &str, text: &str) -> Vec<(String, String)> {
    vec![(path.into(), text.into())]
}

#[test]
fn sizes_and_baselines_enforce_no_growth() {
    let mut policy = OrganizationPolicy {
        max_file_lines: 2,
        hard_file_lines: 3,
        ..Default::default()
    };
    let report = audit(&files("src/a.rs", "a\n\nc\nd\n"), &policy);
    assert!(!report.approval);
    assert_eq!(report.files[0].physical_lines, 4);
    assert_eq!(report.files[0].nonblank_lines, 3);
    policy.baselines.insert(
        "src/a.rs".into(),
        LineBaseline {
            max_lines: 4,
            justification: "Legacy parser; cohesive extraction scheduled".into(),
        },
    );
    assert!(audit(&files("src/a.rs", "a\nb\nc\nd"), &policy).approval);
    assert!(!audit(&files("src/a.rs", "a\nb\nc\nd\ne"), &policy).approval);
    let stale = audit(&files("src/a.rs", "a\nb"), &policy);
    assert!(stale.violations.iter().any(|f| f.code == "stale_baseline"));
    let subset = audit(&files("src/b.rs", "b"), &policy);
    assert!(subset.approval);
    assert!(
        subset
            .recommendations
            .iter()
            .any(|f| f.code == "unverified_baseline_file")
    );
}

#[test]
fn malformed_configuration_and_inventory_fail_closed() {
    assert!(serde_json::from_str::<OrganizationPolicy>(r#"{"unknown":true}"#).is_err());
    let mut policy = OrganizationPolicy::default();
    policy.naming_rules.push(NamingRule {
        prefix: "src".into(),
        extensions: vec![],
        pattern: "[".into(),
        purpose: "source".into(),
    });
    assert!(validate(&policy).is_err());
    assert!(!audit(&[], &policy).policy_valid);
    assert!(!audit(&files("../escape.rs", ""), &Default::default()).approval);
    let duplicate = vec![("a.rs".into(), "".into()), ("a.rs".into(), "".into())];
    assert!(!audit(&duplicate, &Default::default()).approval);
}

#[test]
fn repository_naming_and_roots_are_explicit() {
    let policy = OrganizationPolicy {
        directory_naming_rules: vec![NamingRule {
            prefix: "src".into(),
            extensions: vec![],
            pattern: "^[a-z][a-z-]*$".into(),
            purpose: "Source directories use kebab-case".into(),
        }],
        naming_rules: vec![NamingRule {
            prefix: "src/components".into(),
            extensions: vec!["vue".into()],
            pattern: r"^[A-Z][A-Za-z0-9]*\.vue$".into(),
            purpose: "Reusable UI components use PascalCase".into(),
        }],
        required_prefixes: vec!["src".into()],
        forbidden_prefixes: vec!["src/misc".into()],
        max_depth: 2,
        ..Default::default()
    };
    let input = vec![
        ("src/components/bad-name.vue".into(), "".into()),
        ("other/a.ts".into(), "".into()),
        ("src/misc/a.ts".into(), "".into()),
        ("src/a/b/c/d.ts".into(), "".into()),
        ("src/BadDirectory/a.ts".into(), "".into()),
    ];
    let report = audit(&input, &policy);
    for code in [
        "file_naming",
        "misplaced_file",
        "forbidden_location",
        "path_depth",
        "folder_naming",
    ] {
        assert!(report.violations.iter().any(|f| f.code == code), "{code}");
    }
}

#[test]
fn import_boundaries_and_unresolved_coverage() {
    let policy = OrganizationPolicy {
        layers: vec![
            Layer {
                name: "schema".into(),
                prefix: "src/schemas".into(),
                purpose: "Data contracts without UI dependencies".into(),
                allow_imports_from: vec![],
                forbidden_imports: vec![],
            },
            Layer {
                name: "ui".into(),
                prefix: "src/components".into(),
                purpose: "Presentation consumes contracts".into(),
                allow_imports_from: vec!["schema".into()],
                forbidden_imports: vec![],
            },
        ],
        ..Default::default()
    };
    let input = vec![
        (
            "src/schemas/a.ts".into(),
            "import { B } from '../components/B.js';\nimport './missing';".into(),
        ),
        (
            "src/components/B.ts".into(),
            "import '../schemas/a';".into(),
        ),
    ];
    let report = audit(&input, &policy);
    assert_eq!(report.imports.relative_imports, 3);
    assert_eq!(report.imports.resolved_imports, 2);
    assert_eq!(report.imports.unresolved_imports, 1);
    assert_eq!(
        report
            .violations
            .iter()
            .filter(|f| f.code == "layer_dependency")
            .count(),
        1
    );
}

#[test]
fn comments_are_review_evidence_not_quality_approval() {
    let policy = OrganizationPolicy {
        long_function_lines: 2,
        ..Default::default()
    };
    let report = audit(
        &files(
            "a.rs",
            "fn compute() {\n  one();\n  two();\n}\n// good\nfn compute_again() {\n  one();\n  two();\n}",
        ),
        &policy,
    );
    assert!(
        report
            .recommendations
            .iter()
            .any(|f| f.code == "missing_function_explanation")
    );
    assert!(
        report
            .recommendations
            .iter()
            .any(|f| f.code == "review_function_explanation")
    );
    assert!(
        report.approval,
        "Documentation sufficiency remains human/model review, not a false deterministic failure"
    );
}

#[test]
fn platform_isolation_checks_exact_package_boundaries_and_retains_dynamic_gaps() {
    let policy = OrganizationPolicy {
        layers: vec![Layer {
            name: "core".into(),
            prefix: "src/core".into(),
            purpose: "Platform-independent domain rules and data processing".into(),
            allow_imports_from: vec![],
            forbidden_imports: vec![
                "vue".into(),
                "pinia".into(),
                "discord.js".into(),
                "node:fs".into(),
                "@scope/framework".into(),
            ],
        }],
        ..Default::default()
    };
    let source = "import { ref } from 'vue';\nimport 'vue/server-renderer';\nconst store = require('pinia');\nimport 'discord.js';\nimport 'node:fs/promises';\nimport '@scope/framework/runtime';\nimport 'vue-router';\nimport 'my-vue';\nimport '@scope/framework-extra';\nconst dynamic = import('vue');\nconst computed = require(moduleName);";
    let report = audit(&files("src/core/rules.ts", source), &policy);
    assert_eq!(
        report
            .violations
            .iter()
            .filter(|f| f.code == "forbidden_import")
            .count(),
        6
    );
    assert_eq!(report.imports.external_imports, 9);
    assert_eq!(report.imports.unparsed_import_lines, 2);
    assert!(!report.approval);
    let allowed = audit(
        &files(
            "src/core/rules.ts",
            "import 'vue-router';\nimport 'my-vue';\nimport '@scope/framework-extra';",
        ),
        &policy,
    );
    assert!(
        allowed.approval,
        "Package selectors must not match substrings or sibling package names"
    );
}

#[test]
fn configured_aliases_resolve_layers_and_longest_prefix_without_filesystem_reads() {
    let policy = OrganizationPolicy {
        import_aliases: [
            ("@/".into(), "src/".into()),
            ("@/special/".into(), "packages/contracts/".into()),
        ]
        .into(),
        layers: vec![
            Layer {
                name: "services".into(),
                prefix: "src/services".into(),
                purpose: "Application services consume contracts".into(),
                allow_imports_from: vec!["contracts".into()],
                forbidden_imports: vec![],
            },
            Layer {
                name: "components".into(),
                prefix: "src/components".into(),
                purpose: "View components render application state".into(),
                allow_imports_from: vec![],
                forbidden_imports: vec![],
            },
            Layer {
                name: "contracts".into(),
                prefix: "packages/contracts".into(),
                purpose: "Shared transport data contracts".into(),
                allow_imports_from: vec![],
                forbidden_imports: vec![],
            },
        ],
        ..Default::default()
    };
    let input = vec![
        (
            "src/services/load.ts".into(),
            "import '@/components/View';\nimport '@/special/contract';\nimport '@/missing';".into(),
        ),
        ("src/components/View.ts".into(), "".into()),
        ("packages/contracts/contract.ts".into(), "".into()),
    ];
    let report = audit(&input, &policy);
    assert_eq!(report.imports.alias_imports, 3);
    assert_eq!(report.imports.resolved_imports, 2);
    assert_eq!(report.imports.unresolved_imports, 1);
    assert_eq!(report.imports.external_imports, 0);
    assert_eq!(
        report
            .violations
            .iter()
            .filter(|f| f.code == "layer_dependency")
            .count(),
        1
    );
    let allowed = audit(
        &[
            (
                "src/services/load.ts".into(),
                "import '@/special/contract';".into(),
            ),
            ("packages/contracts/contract.ts".into(), "".into()),
        ],
        &policy,
    );
    assert!(allowed.approval);
    let mut invalid = policy;
    invalid
        .import_aliases
        .insert("escape/".into(), "../outside/".into());
    assert!(validate(&invalid).is_err());
    assert!(!audit(&[], &invalid).approval);
}

#[test]
fn embedded_import_text_and_block_comments_are_not_executable_imports() {
    let policy = OrganizationPolicy {
        layers: vec![Layer {
            name: "core".into(),
            prefix: "src".into(),
            purpose: "Independent domain rules and contracts".into(),
            allow_imports_from: vec![],
            forbidden_imports: vec!["vue".into()],
        }],
        ..Default::default()
    };
    let source = r#"const fixture = "import { ref } from 'vue'";
const other = 'require("vue")';
/* import 'vue';
   import x from 'vue';
*/
// import 'vue';
const plain = `import 'vue'`;
import {
    ref
} from 'vue';"#;
    let report = audit(&files("src/rules.ts", source), &policy);
    assert_eq!(report.imports.external_imports, 1);
    let violations: Vec<_> = report
        .violations
        .iter()
        .filter(|f| f.code == "forbidden_import")
        .collect();
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].line, Some(10));
}
