//! End-to-end smoke tests that exercise the compiled binary against a temp config,
//! without requiring a real IMAP/SMTP server. These verify:
//!
//! - `agent-info --json` prints a stable, jq-friendly manifest
//! - `account add/list/remove` round-trips through the TOML file
//! - Duplicate account names, empty passwords, missing accounts are rejected
//!   with the correct exit codes
//! - The delete two-gate rejects the caller before any network I/O
//! - The send allowlist rejects the caller before any network I/O
//!
//! Anything requiring live IMAP/SMTP is out of scope — those paths are exercised
//! manually against a real account. Cf. README §"Quick start".

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

fn bin() -> PathBuf {
    // CARGO_BIN_EXE_mail-cli is set by cargo for integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_mail-cli"))
}

/// Unique per-test account name — avoids OS keyring collisions when tests run in parallel.
fn unique_name(prefix: &str) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("test-{prefix}-{pid}-{n}")
}

fn run(args: &[&str], stdin: Option<&str>, config_path: &Path) -> (i32, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .env("MAIL_CLI_CONFIG", config_path)
        .env_remove("MAIL_CLI_DELETE_ENABLED")
        .env_remove("MAIL_CLI_READ_ONLY")
        .env_remove("MAIL_CLI_ACCOUNT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().expect("spawn mail-cli");
    if let Some(s) = stdin {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(s.as_bytes()).ok();
    }
    let out = child.wait_with_output().expect("wait mail-cli");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn temp_config() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    (dir, path)
}

/// Add a test account with the given unique name. Returns (name, cfg_path, _dir).
fn add_test_account(prefix: &str) -> (String, PathBuf, tempfile::TempDir) {
    let (dir, cfg) = temp_config();
    let name = unique_name(prefix);
    let (code, out, err) = run(
        &[
            "account",
            "add",
            "--name",
            &name,
            "--email",
            "me@example.com",
            "--imap-host",
            "imap.example.com",
            "--smtp-host",
            "smtp.example.com",
            "--login",
            "me@example.com",
            "--password-stdin",
            "--json",
        ],
        Some("test-pw"),
        &cfg,
    );
    assert_eq!(code, 0, "seed account add: stdout={out} stderr={err}");
    (name, cfg, dir)
}

/// Remove keyring entries so we don't leak into the developer keychain.
fn cleanup_account(name: &str, cfg: &Path) {
    let _ = run(&["account", "remove", "--name", name, "--json"], None, cfg);
}

#[test]
fn agent_info_prints_a_valid_manifest() {
    let (_dir, cfg) = temp_config();
    let (code, stdout, _) = run(&["agent-info", "--json"], None, &cfg);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["name"], "mail-cli");
    assert_eq!(v["protocol"], "cli");
    assert!(v["commands"].as_array().unwrap().len() >= 10);
    assert!(v["exit_codes"]["0"].is_string());
    assert!(v["exit_codes"]["4"].is_string());
    assert!(v["safety"]["mitigations"].as_array().unwrap().len() >= 3);
}

#[test]
fn account_add_then_list_then_remove() {
    let (name, cfg, _dir) = add_test_account("basic");

    let (code, out, _) = run(&["account", "list", "--json"], None, &cfg);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["accounts"][0]["name"], name);
    assert_eq!(v["accounts"][0]["default"], true);

    // Duplicate add rejected
    let (code, out, _) = run(
        &[
            "account",
            "add",
            "--name",
            &name,
            "--email",
            "x@x",
            "--imap-host",
            "h",
            "--smtp-host",
            "h",
            "--login",
            "x@x",
            "--password-stdin",
            "--json",
        ],
        Some("pw2"),
        &cfg,
    );
    assert_eq!(code, 3, "duplicate should be exit 3 (input)");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["error"]["kind"], "input");

    cleanup_account(&name, &cfg);
    let (code, out, _) = run(&["account", "list", "--json"], None, &cfg);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["accounts"].as_array().unwrap().len(), 0);
}

#[test]
fn empty_password_rejected() {
    let (_dir, cfg) = temp_config();
    let name = unique_name("emptypw");
    let (code, out, _) = run(
        &[
            "account",
            "add",
            "--name",
            &name,
            "--email",
            "e@e",
            "--imap-host",
            "h",
            "--smtp-host",
            "h",
            "--login",
            "e@e",
            "--password-stdin",
            "--json",
        ],
        Some(""),
        &cfg,
    );
    assert_eq!(code, 3);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["error"]["kind"], "input");
}

#[test]
fn missing_account_is_config_error() {
    let (_dir, cfg) = temp_config();
    let (code, out, _) = run(&["message", "list", "--json"], None, &cfg);
    assert_eq!(code, 2, "no default account should be config error");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["error"]["kind"], "config");
}

#[test]
fn delete_gate_missing_flag_is_input_error() {
    let (name, cfg, _dir) = add_test_account("delgate");

    let (code, out, _) = run(&["message", "delete", "--id", "1", "--json"], None, &cfg);
    assert_eq!(code, 3);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["error"]["kind"], "input");
    assert!(v["error"]["message"].as_str().unwrap().contains("delete"));

    cleanup_account(&name, &cfg);
}

#[test]
fn send_dry_run_needs_no_network() {
    let (name, cfg, _dir) = add_test_account("dryrun");

    let (code, out, _) = run(
        &[
            "message",
            "send",
            "--to",
            "alice@example.com",
            "--subject",
            "hi",
            "--body-file",
            "-",
            "--json",
        ],
        Some("Hello!"),
        &cfg,
    );
    assert_eq!(code, 0, "dry-run must not touch the network");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["status"], "dry-run");
    assert_eq!(v["to"][0], "alice@example.com");

    cleanup_account(&name, &cfg);
}

#[test]
fn send_allowlist_rejects_before_network() {
    let (name, cfg, _dir) = add_test_account("allowlist");

    let (code, out, _) = run(
        &[
            "message",
            "send",
            "--to",
            "alice@example.com",
            "--subject",
            "hi",
            "--body-file",
            "-",
            "--send",
            "--json",
        ],
        Some("Hello!"),
        &cfg,
    );
    assert_eq!(code, 3);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["error"]["kind"], "input");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("allowlist")
    );

    cleanup_account(&name, &cfg);
}
