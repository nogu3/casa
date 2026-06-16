//! casad run --once の統合テスト。時刻トリガの発火を、casa 代役スタブを差し込んで
//! 決定的に検証する（実時計に依存しないよう --now で時刻を固定）。

mod common;

use common::*;

const SCHEDULE: &str = r#"
version = 1
[[rules]]
name = "9時にエアコンOFF"
when = { at = "09:00" }
then = { action = "off", device = "living_aircon" }
[[rules]]
name = "状変でON"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "living_aircon" }
"#;

fn write_rules(dir: &std::path::Path, text: &str) -> std::path::PathBuf {
    let path = dir.join("rules.toml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn run_once_fires_matching_time_rule_via_casa() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), SCHEDULE);

    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--once",
            "--now",
            "09:00",
            "--config",
            config.to_str().unwrap(),
        ],
        &[("CASA_BIN", &fixture("casa_stub.sh"))],
    );

    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // 時刻一致ルールだけが発火し、casa に `off living_aircon` が渡る。
    assert!(stdout.contains("off living_aircon"), "stdout: {stdout}");
    // イベントトリガ（on）は時刻評価では発火しない。
    assert!(!stdout.contains("on living_aircon"), "stdout: {stdout}");
}

#[test]
fn run_once_fires_nothing_when_no_rule_matches_minute() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), SCHEDULE);

    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--once",
            "--now",
            "10:30",
            "--config",
            config.to_str().unwrap(),
        ],
        &[("CASA_BIN", &fixture("casa_stub.sh"))],
    );

    assert_eq!(out.status.code(), Some(0));
    // casa は一度も起動されない。
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("casa called"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn run_rejects_invalid_time_in_rule() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(
        dir.path(),
        r#"
version = 1
[[rules]]
name = "壊れた時刻"
when = { at = "9am" }
then = { action = "off", device = "living_aircon" }
"#,
    );

    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--once",
            "--config",
            config.to_str().unwrap(),
        ],
        &[("CASA_BIN", &fixture("casa_stub.sh"))],
    );

    assert_eq!(out.status.code(), Some(10));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "config_parse");
}

#[test]
fn run_now_requires_once() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), SCHEDULE);

    // --now を --once 無しで使うと clap が引数エラー（exit 2）。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--now",
            "09:00",
            "--config",
            config.to_str().unwrap(),
        ],
        &[("CASA_BIN", &fixture("casa_stub.sh"))],
    );

    assert_eq!(out.status.code(), Some(2));
}
