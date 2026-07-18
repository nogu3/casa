//! `casa invoke` の統合テスト。ダミー子 CLI で引数素通し・envelope・グループ・
//! エラー系を検証する。CI で実 enl / mat は不要。
//!
//! casa のグローバルフラグ（--config）は trailing 引数に呑まれないよう
//! invoke より前に置く（README の規約どおりの呼び方でテストする）。

mod common;

use common::*;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

const MATTER_CONFIG: &str = r#"
version = 1

[devices.living_light]
protocol = "matter"
node_id = "1234"
"#;

const GROUP_CONFIG: &str = r#"
version = 1

[devices.light1]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"

[devices.light2]
protocol = "echonet"
ip = "192.0.2.12"
eoj = "0x029101"

[groups.living]
members = ["light1", "light2"]
"#;

const MIXED_GROUP_CONFIG: &str = r#"
version = 1

[devices.light1]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"

[devices.light2]
protocol = "matter"
node_id = "1234"

[groups.mixed]
members = ["light1", "light2"]
"#;

#[test]
fn invoke_injects_echonet_address_and_passes_args_through() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "living_aircon",
            "raw",
            "0x62",
            "0x80",
        ],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = stdout_json(&out);
    assert_eq!(v["device"], "living_aircon");
    assert_eq!(v["protocol"], "echonet");
    assert_eq!(v["command"], "raw");
    assert!(v["timestamp"].is_string(), "timestamp missing: {v}");
    let expected = serde_json::json!(["raw", "192.0.2.10", "0x013001", "0x62", "0x80"]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn invoke_matter_color_temp_replaces_removed_shortcut() {
    // 旧 `casa color-temp` の代替経路。削除後もこの呼び方で同じ mat 引数列になる。
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "living_light",
            "color-temp",
            "--kelvin",
            "2700",
        ],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );

    assert_eq!(out.status.code(), Some(0));
    let v = stdout_json(&out);
    assert_eq!(v["command"], "color-temp");
    let expected = serde_json::json!(["color-temp", "--node", "1234", "--kelvin", "2700"]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn invoke_group_runs_members_and_tags_command() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), GROUP_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "living",
            "describe",
        ],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );

    assert_eq!(out.status.code(), Some(0));
    let v = stdout_json(&out);
    assert_eq!(v["group"], "living");
    assert_eq!(v["command"], "describe");
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["ok"], true);
    assert_eq!(
        results[0]["value"]["args"],
        serde_json::json!(["describe", "192.0.2.11", "0x029101"])
    );
}

#[test]
fn invoke_mixed_protocol_group_exits_14() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MIXED_GROUP_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "mixed",
            "blink",
        ],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );

    assert_eq!(out.status.code(), Some(14));
    assert_eq!(
        stderr_error_json(&out)["error"]["kind"],
        "protocol_unsupported"
    );
}

#[test]
fn invoke_switchbot_dispatches_to_swb() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "entry_lock",
            "status",
        ],
        &[("CASA_SWB_BIN", &fixture("swb_args.sh"))],
    );

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = stdout_json(&out);
    assert_eq!(v["device"], "entry_lock");
    assert_eq!(v["protocol"], "switchbot");
    assert_eq!(v["command"], "status");
    let expected = serde_json::json!(["status", "DUMMY-XX-XX"]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn invoke_propagates_child_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "living_aircon",
            "blink",
        ],
        &[("CASA_ENL_BIN", &fixture("enl_exit3.sh"))],
    );

    // 子 CLI の exit code（3 = enl タイムアウト）をそのまま伝播する。
    assert_eq!(out.status.code(), Some(3));
}
