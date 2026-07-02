use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

fn cmd() -> Command {
    Command::cargo_bin("trakt-letterboxd").unwrap()
}

/// Command with a clean environment — prevents host TRAKT_* or HOME from leaking into tests.
fn clean_cmd() -> Command {
    let mut c = Command::cargo_bin("trakt-letterboxd").unwrap();
    c.env_clear();
    c
}

/// Command with valid credentials injected via env vars and no config file on disk.
fn authed_cmd() -> Command {
    let mut c = clean_cmd();
    c.env("TRAKT_CLIENT_ID", "test_id_integration")
        .env("TRAKT_CLIENT_SECRET", "test_secret_integration");
    c
}

// ── 1. --help ────────────────────────────────────────────────────────────────

#[test]
fn help_exits_zero() {
    cmd().arg("--help").assert().success();
}

#[test]
fn help_lists_auth_subcommand() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("auth"));
}

#[test]
fn help_lists_sync_subcommand() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("sync"));
}

#[test]
fn sync_help_lists_from_letterboxd() {
    cmd()
        .args(["sync", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("from-letterboxd"));
}

#[test]
fn sync_help_lists_to_letterboxd() {
    cmd()
        .args(["sync", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("to-letterboxd"));
}

// ── 2. Subcommand smoke tests ─────────────────────────────────────────────────

#[test]
fn auth_attempts_device_flow_with_configured_credentials() {
    // With fake credentials the Trakt API returns 4xx; the CLI should exit
    // non-zero with a human-readable error — not a panic.
    authed_cmd()
        .arg("auth")
        .assert()
        .failure()
        .stderr(predicate::str::contains("device code").or(predicate::str::contains("HTTP")))
        .stderr(predicate::str::contains("panicked at").not());
}

#[test]
fn sync_from_letterboxd_requires_auth_when_no_token() {
    // Without a saved token file the command must exit non-zero with a clear auth error.
    authed_cmd()
        .args(["sync", "from-letterboxd", "."])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not authenticated").or(predicate::str::contains("auth")));
}

#[test]
fn sync_to_letterboxd_exits_zero_and_prints_not_implemented() {
    authed_cmd()
        .args(["sync", "to-letterboxd"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not yet implemented"));
}

// ── 3. Missing credentials → non-zero exit with human-readable error ──────────

#[test]
fn missing_credentials_exits_nonzero() {
    clean_cmd().arg("auth").assert().failure();
}

#[test]
fn missing_credentials_names_the_missing_field_on_stderr() {
    clean_cmd()
        .arg("auth")
        .assert()
        .failure()
        .stderr(predicate::str::contains("trakt_client_id"));
}

#[test]
fn missing_credentials_error_is_not_a_rust_panic() {
    clean_cmd()
        .arg("auth")
        .assert()
        .failure()
        .stderr(predicate::str::contains("panicked at").not())
        .stderr(predicate::str::contains("stack backtrace").not());
}

// ── 4. Valid config file loads without error ───────────────────────────────────

#[test]
fn valid_config_file_loads_without_error() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "trakt_client_id = \"cfg_id\"").unwrap();
    writeln!(f, "trakt_client_secret = \"cfg_secret\"").unwrap();

    // Use sync (not auth) to verify config loads correctly without making HTTP calls.
    clean_cmd()
        .arg("--config")
        .arg(f.path())
        .args(["sync", "to-letterboxd"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not yet implemented"));
}

// ── 5. Env var overrides take effect ──────────────────────────────────────────

#[test]
fn env_var_credentials_alone_are_sufficient() {
    clean_cmd()
        .env("TRAKT_CLIENT_ID", "env_only_id")
        .env("TRAKT_CLIENT_SECRET", "env_only_secret")
        .args(["sync", "to-letterboxd"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not yet implemented"));
}

#[test]
fn env_vars_override_config_file_values() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "trakt_client_id = \"file_id\"").unwrap();
    writeln!(f, "trakt_client_secret = \"file_secret\"").unwrap();

    clean_cmd()
        .arg("--config")
        .arg(f.path())
        .env("TRAKT_CLIENT_ID", "override_id")
        .env("TRAKT_CLIENT_SECRET", "override_secret")
        .args(["sync", "to-letterboxd"])
        .assert()
        .success();
}

#[test]
fn trakt_config_file_env_var_points_to_config() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "trakt_client_id = \"via_config_file_env\"").unwrap();
    writeln!(f, "trakt_client_secret = \"via_config_file_env_secret\"").unwrap();

    clean_cmd()
        .env("TRAKT_CONFIG_FILE", f.path().to_str().unwrap())
        .args(["sync", "to-letterboxd"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not yet implemented"));
}
