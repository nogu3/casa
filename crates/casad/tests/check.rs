//! casad check の統合テスト。ルールファイルのパースとデバイス参照検証を検証する。

mod common;

use common::*;

const RULES_OK: &str = r#"
version = 1
[[rules]]
name = "エアコン起動で点灯"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "living_aircon" }
[[rules]]
name = "22時消灯"
when = { at = "22:00" }
then = { action = "off", device = "living_aircon" }
[[rules]]
name = "起床時に色温度を調整"
when = { at = "07:00" }
then = { action = "invoke", device = "living_aircon", command = "color-temp", args = ["--kelvin", "2700"] }
"#;

fn write_rules(dir: &std::path::Path, text: &str) -> std::path::PathBuf {
    let path = dir.join("rules.toml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn check_valid_rules_reports_count() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), RULES_OK);

    let out = run_casad(
        &[
            "check",
            rules.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ],
        &[],
    );

    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout not JSON: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    assert_eq!(v["ok"], true);
    assert_eq!(v["count"], 3);
    assert_eq!(v["rules"].as_array().unwrap().len(), 3);
    // casad の解釈（パース結果）がそのまま返る。
    assert_eq!(v["rules"][0]["then"]["action"], "on");
    // invoke ルールも then がそのまま JSON 化される（command / args を含む）。
    assert_eq!(v["rules"][2]["then"]["action"], "invoke");
    assert_eq!(v["rules"][2]["then"]["command"], "color-temp");
    assert_eq!(
        v["rules"][2]["then"]["args"],
        serde_json::json!(["--kelvin", "2700"])
    );
}

#[test]
fn check_unknown_device_reference_exits_11() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(
        dir.path(),
        r#"
version = 1
[[rules]]
name = "未定義参照"
when = { at = "07:00" }
then = { action = "on", device = "ghost" }
"#,
    );

    let out = run_casad(
        &[
            "check",
            rules.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ],
        &[],
    );

    assert_eq!(out.status.code(), Some(11));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "name_not_found");
}

#[test]
fn check_malformed_rules_exits_10() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), "version = 1\n[[rules]]\nname = ");

    let out = run_casad(
        &[
            "check",
            rules.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ],
        &[],
    );

    assert_eq!(out.status.code(), Some(10));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "config_parse");
}

#[test]
fn check_example_rules_validate_against_example_config() {
    // examples/ は workspace ルート直下。casad crate からは 2 つ上。
    let config = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/devices.toml");
    let rules = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/rules.toml");

    let out = run_casad(&["check", rules, "--config", config], &[]);

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
