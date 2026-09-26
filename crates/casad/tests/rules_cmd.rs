//! casad rules（list / pause / resume）の統合テスト。

mod common;

use common::*;

const RULES: &str = r#"
version = 1
[[rules]]
name = "リビングエアコン 0時OFF"
when = { at = "00:00" }
then = { action = "off", device = "living_aircon" }
[[rules]]
name = "状変でON"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "living_aircon" }
"#;

struct Env {
    _dir: tempfile::TempDir,
    config: String,
    rules: String,
    pause: std::path::PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG)
        .to_str()
        .unwrap()
        .to_string();
    let rules_path = dir.path().join("rules.toml");
    std::fs::write(&rules_path, RULES).unwrap();
    // 親ディレクトリが無い状態から始める（初回 pause でディレクトリを作れること）
    let pause = dir.path().join("state/casa/paused.json");
    Env {
        config,
        rules: rules_path.to_str().unwrap().to_string(),
        pause,
        _dir: dir,
    }
}

fn casad(e: &Env, args: &[&str]) -> std::process::Output {
    let mut full = vec!["--config", e.config.as_str(), "rules"];
    full.extend_from_slice(args);
    run_casad(
        &full,
        &[
            ("CASA_RULES", e.rules.as_str()),
            ("CASA_PAUSE_FILE", e.pause.to_str().unwrap()),
        ],
    )
}

fn list(e: &Env) -> serde_json::Value {
    let out = casad(e, &["list", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn list_shows_all_rules_in_order_with_trigger_kind() {
    let e = env();
    let v = list(&e);
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "リビングエアコン 0時OFF");
    assert_eq!(arr[0]["trigger"], "time");
    assert_eq!(arr[0]["at"], "00:00");
    assert_eq!(arr[0]["paused"], false);
    assert!(arr[0].get("since").is_none());
    assert_eq!(arr[1]["trigger"], "event");
    assert!(arr[1].get("at").is_none());
}

#[test]
fn pause_then_resume_roundtrip_and_idempotent() {
    let e = env();
    assert_eq!(
        casad(&e, &["pause", "リビングエアコン 0時OFF"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        casad(&e, &["pause", "リビングエアコン 0時OFF"])
            .status
            .code(),
        Some(0)
    );
    let v = list(&e);
    assert_eq!(v[0]["paused"], true);
    assert!(v[0]["since"].as_str().unwrap().starts_with("20"));
    assert_eq!(
        casad(&e, &["resume", "リビングエアコン 0時OFF"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        casad(&e, &["resume", "リビングエアコン 0時OFF"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(list(&e)[0]["paused"], false);
}

#[test]
fn pause_unknown_name_is_name_not_found_and_writes_nothing() {
    let e = env();
    let out = casad(&e, &["pause", "無いルール"]);
    assert_eq!(out.status.code(), Some(11));
    assert!(!e.pause.exists());
}

#[test]
fn resume_removes_stale_name_missing_from_rules() {
    let e = env();
    std::fs::create_dir_all(e.pause.parent().unwrap()).unwrap();
    std::fs::write(
        &e.pause,
        r#"{"paused":[{"name":"消えたルール","since":"t"}]}"#,
    )
    .unwrap();
    assert_eq!(
        casad(&e, &["resume", "消えたルール"]).status.code(),
        Some(0)
    );
    let text = std::fs::read_to_string(&e.pause).unwrap();
    assert!(!text.contains("消えたルール"));
    assert_eq!(
        casad(&e, &["resume", "消えたルール"]).status.code(),
        Some(11)
    );
}

#[test]
fn broken_pause_file_is_not_overwritten() {
    let e = env();
    std::fs::create_dir_all(e.pause.parent().unwrap()).unwrap();
    std::fs::write(&e.pause, "{broken").unwrap();
    assert_eq!(
        casad(&e, &["pause", "リビングエアコン 0時OFF"])
            .status
            .code(),
        Some(10)
    );
    assert_eq!(std::fs::read_to_string(&e.pause).unwrap(), "{broken");
}
