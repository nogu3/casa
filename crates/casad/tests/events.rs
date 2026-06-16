//! casad run --listen-once の統合テスト。enl 代役（enl_listen.sh）が固定の INF 通知を
//! 出し、casad がそれをイベントトリガに突合して casa 代役を発火することを検証する。

mod common;

use common::*;

const EVENT_RULES: &str = r#"
version = 1
[[rules]]
name = "エアコン電源ONで点灯"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "living_aircon" }
"#;

fn write_rules(dir: &std::path::Path, text: &str) -> std::path::PathBuf {
    let path = dir.join("rules.toml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn listen_once_fires_event_rule_via_casa() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), EVENT_RULES);

    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_ENL_BIN", &fixture("enl_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
        ],
    );

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // 通知（EPC80=30）が rule に一致し、casa に `on living_aircon` が渡る。
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("on living_aircon"), "stdout: {stdout}");
}

#[test]
fn listen_once_does_not_fire_on_value_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), EVENT_RULES);

    // 通知の EDT を 31(OFF) に上書き。rule は 0x30 を待つので一致しない。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_ENL_BIN", &fixture("enl_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("CASAD_ENL_EDT", "31"),
        ],
    );

    assert_eq!(out.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("casa called"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}
