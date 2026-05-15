//! `ShadowBackend` smoke tests.
//!
//! - `auto_with_no_config_errors_when_docker_unavailable`: verifies that
//!   `resolve` returns a descriptive error when neither Docker nor a DSN is
//!   available.
//! - `explicit_dsn_backend_round_trips`: verifies the DSN backend when
//!   `PGEVOLVE_TEST_PG_URL` is set.
//! - `testcontainers_backend_checkouts_when_docker_available`: full
//!   container round-trip (skipped when Docker is absent).

use pgevolve::config::ShadowConfig;
use pgevolve::shadow::resolve;

#[tokio::test]
async fn auto_with_no_config_errors_when_docker_unavailable() {
    if std::process::Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        eprintln!("Docker available; skipping no-Docker assertion");
        return;
    }
    let cfg = ShadowConfig::default();
    let err = resolve(&cfg).err().expect("should error when Docker unavailable and no DSN");
    assert!(
        err.to_string().contains("no shadow backend"),
        "expected 'no shadow backend' in error, got: {err}",
    );
}

#[tokio::test]
async fn explicit_dsn_backend_round_trips() {
    let Ok(url) = std::env::var("PGEVOLVE_TEST_PG_URL") else {
        eprintln!("PGEVOLVE_TEST_PG_URL not set; skipping");
        return;
    };
    let cfg = ShadowConfig {
        backend: Some("dsn".to_string()),
        url: Some(url),
        reset: Some("drop_schema_cascade".to_string()),
        ..Default::default()
    };
    let backend = resolve(&cfg).expect("resolve should succeed");
    let guard = backend.checkout(17).await.expect("checkout should succeed");
    assert!(
        guard.url().starts_with("postgres"),
        "url should be a real DSN: {}",
        guard.url(),
    );
}

#[tokio::test]
async fn testcontainers_backend_checkouts_when_docker_available() {
    if !std::process::Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        eprintln!("Docker unavailable; skipping");
        return;
    }
    let cfg = ShadowConfig {
        backend: Some("testcontainers".to_string()),
        postgres_version: Some("17".to_string()),
        ..Default::default()
    };
    let backend = resolve(&cfg).expect("resolve should succeed");
    let guard = backend.checkout(17).await.expect("checkout should succeed");
    assert!(
        guard.url().starts_with("postgres://"),
        "url should be a real DSN: {}",
        guard.url(),
    );
}
