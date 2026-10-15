use super::*;
use serde_json::json;

fn dependency(ecosystem: Ecosystem, name: &str) -> Dependency {
    Dependency {
        ecosystem,
        name: name.into(),
        requirement: Some("^1".into()),
        current: Some("1.0.0".into()),
        scope: "direct".into(),
        manifest: "package.json".into(),
        public_registry: true,
    }
}

#[tokio::test]
async fn expired_online_batch_budget_never_polls_queued_work() {
    let dependency = dependency(Ecosystem::Npm, "queued-package");
    let polled = std::sync::atomic::AtomicBool::new(false);
    let work = async {
        polled.store(true, std::sync::atomic::Ordering::SeqCst);
        report(
            &dependency,
            Metadata::default(),
            "unexpectedly_polled",
            None,
            None,
        )
    };
    let result = within_deadline(
        &dependency,
        Some(tokio::time::Instant::now() - std::time::Duration::from_secs(1)),
        work,
    )
    .await;
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(result.status, "budget_exhausted");
    assert_eq!(result.name, "queued-package");
    assert!(result.latest.is_none());
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.contains("60-second"))
    );
}

#[tokio::test]
async fn duplicate_lookup_plans_coalesce_without_reordering_or_sending_private_names() {
    let first = dependency(Ecosystem::Npm, "widget");
    let mut duplicate = first.clone();
    duplicate.manifest = "workspace/package-lock.json".into();
    duplicate.scope = "transitive".into();
    let mut private = first.clone();
    private.public_registry = false;
    let mut another_version = first.clone();
    another_version.current = Some("2.0.0".into());
    let inputs = vec![
        first,
        dependency(Ecosystem::Cargo, "serde"),
        duplicate,
        private,
        another_version,
    ];
    let plans = distinct_indices(&inputs);
    assert_eq!(
        plans.len(),
        4,
        "Five declarations require only four distinct lookup plans"
    );
    assert_eq!(
        plans.iter().filter(|indices| indices.contains(&0)).count(),
        1
    );
    assert!(
        plans.iter().any(|indices| indices == &[0, 2]),
        "Direct and lockfile declarations share exactly one request plan"
    );
    assert!(
        plans.iter().any(|indices| indices == &[3]),
        "Private declaration must never share a public request"
    );
    let directory = tempfile::tempdir().unwrap();
    let reports = lookup_all(&inputs, directory.path(), false, false).await;
    assert_eq!(reports.len(), inputs.len());
    for (report, input) in reports.iter().zip(&inputs) {
        assert_eq!(report.name, input.name);
        assert_eq!(report.current, input.current);
        assert_eq!(report.ecosystem, input.ecosystem);
    }
    assert_eq!(reports[3].status, "nonpublic_skipped");
}

#[test]
fn primary_registry_fixtures_keep_identities_and_current_deprecation_distinct() {
    let npm = dependency(Ecosystem::Npm, "@scope/widget");
    let mut data = Metadata::default();
    parse::latest(&npm, &json!({"name":"@scope/widget","version":"2.0.0","engines":{"node":">=20"},"peerDependencies":{"vue":"^3"}}), &mut data).unwrap();
    parse::npm_current(
        &npm,
        &json!({"name":"@scope/widget","version":"1.0.0","deprecated":"Use v2"}),
        &mut data,
    )
    .unwrap();
    parse::current(&npm, &json!({"versionKey":{"system":"NPM","name":"@scope/widget","version":"1.0.0"},"advisoryKeys":[{"id":"GHSA-example"}]}), &mut data).unwrap();
    assert_eq!(data.latest.as_deref(), Some("2.0.0"));
    assert_eq!(data.current_deprecated, Some(true));
    assert_eq!(data.latest_deprecated, Some(false));
    assert_eq!(data.requires_runtime.as_deref(), Some("node=>=20"));
    assert_eq!(
        data.peer_dependencies.get("vue").map(String::as_str),
        Some("^3")
    );
    assert_eq!(data.advisory_ids, ["GHSA-example"]);
    assert!(parse::latest(&npm, &json!({"name":"different","version":"9"}), &mut data).is_err());
    assert_eq!(http::segment("@scope/widget"), "%40scope%2Fwidget");
    assert_eq!(http::segment("a?b#c%/"), "a%3Fb%23c%25%2F");
    let mut cargo = Metadata::default();
    parse::latest(&dependency(Ecosystem::Cargo,"serde"), &json!({"crate":{"id":"serde","max_stable_version":"1.0.0"},"versions":[{"num":"1.0.0","rust_version":"1.56"}]}), &mut cargo).unwrap();
    assert_eq!(cargo.requires_runtime.as_deref(), Some("rust=1.56"));
    let mut python = Metadata::default();
    parse::latest(
        &dependency(Ecosystem::Pypi, "Some_Package"),
        &json!({"info":{"name":"some-package","version":"2.0","requires_python":">=3.10"}}),
        &mut python,
    )
    .unwrap();
    assert_eq!(python.requires_runtime.as_deref(), Some("python=>=3.10"));
}

#[test]
fn deps_dev_requires_an_actual_default_not_semver_guessing() {
    let dep = dependency(Ecosystem::Maven, "group:artifact");
    let no_default = json!({"packageKey":{"system":"MAVEN","name":"group:artifact"},"versions":[{"versionKey":{"system":"MAVEN","name":"group:artifact","version":"99"}}]});
    let mut data = Metadata::default();
    assert!(parse::latest(&dep, &no_default, &mut data).is_err());
    assert!(data.latest.is_none());
    let default = json!({"packageKey":{"system":"MAVEN","name":"group:artifact"},"versions":[{"versionKey":{"system":"MAVEN","name":"group:artifact","version":"2.1"},"isDefault":true}]});
    parse::latest(&dep, &default, &mut data).unwrap();
    assert_eq!(data.latest.as_deref(), Some("2.1"));
}

#[test]
fn malformed_deprecation_and_advisory_fields_are_not_clean_metadata() {
    let dep = dependency(Ecosystem::Npm, "widget");
    let mut data = Metadata::default();
    assert!(
        parse::npm_current(
            &dep,
            &json!({"name":"widget","version":"1.0.0","deprecated":true}),
            &mut data
        )
        .is_err()
    );
    assert!(data.current_deprecated.is_none());
    assert!(parse::current(&dep, &json!({"versionKey":{"system":"NPM","name":"widget","version":"1.0.0"},"isDeprecated":"true"}), &mut data).is_err());
    assert!(parse::current(&dep, &json!({"versionKey":{"system":"NPM","name":"widget","version":"1.0.0"},"advisoryKeys":false}), &mut data).is_err());
}

#[tokio::test]
async fn cache_hit_offline_stale_refresh_and_integrity_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let dep = dependency(Ecosystem::Npm, "widget");
    let data = Metadata {
        latest: Some("2.0.0".into()),
        ..Default::default()
    };
    let now = chrono::Utc::now().to_rfc3339();
    cache::write(dir.path(), &identity(&dep), &data, &now).unwrap();
    let hit = lookup(&dep, dir.path(), true, false, Err("would fetch")).await;
    assert_eq!(hit.status, "fresh_cache");
    let refresh = lookup(&dep, dir.path(), true, true, Err("would fetch")).await;
    assert_eq!(refresh.status, "error");
    assert!(refresh.errors.iter().any(|e| e == "would fetch"));
    let old = (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
    cache::write(dir.path(), &identity(&dep), &data, &old).unwrap();
    let stale = lookup(&dep, dir.path(), false, true, Err("must not fetch")).await;
    assert_eq!(stale.status, "stale_cache");
    assert_eq!(stale.latest.as_deref(), Some("2.0.0"));
    assert!(!stale.errors.iter().any(|e| e == "must not fetch"));
    let entry = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&entry).unwrap()).unwrap();
    value["identity"] = json!("mismatched");
    std::fs::write(&entry, serde_json::to_vec(&value).unwrap()).unwrap();
    let invalid = lookup(&dep, dir.path(), false, false, Err("must not fetch")).await;
    assert_eq!(invalid.status, "offline_missing");
    assert!(
        invalid
            .errors
            .iter()
            .any(|e| e.contains("identity/integrity"))
    );
}

#[tokio::test]
async fn mock_http_redirect_and_oversize_are_errors_without_following_links() {
    for (headers, body) in [
        (
            "302 Found\r\nLocation: https://example.invalid/secret\r\nContent-Length: 0",
            "",
        ),
        ("200 OK\r\nContent-Length: 2097153", ""),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!("HTTP/1.1 {headers}\r\nConnection: close\r\n\r\n{body}");
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request).await;
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let response = http::client()
            .unwrap()
            .get(format!("http://{address}/fixture"))
            .send()
            .await
            .unwrap();
        assert!(http::decode(response).await.is_err());
        server.await.unwrap();
    }
    assert!(!http::allowed(
        &reqwest::Url::parse("https://registry.npmjs.org.evil.test/x").unwrap()
    ));
    assert!(!http::allowed(
        &reqwest::Url::parse("https://user:secret@registry.npmjs.org/x").unwrap()
    ));
}
