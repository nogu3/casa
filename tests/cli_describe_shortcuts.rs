//! Phase 2: describe / on / off / list --describe の統合テスト。

mod common;

use common::*;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn setup() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    (config.to_str().unwrap().to_string(), dir)
}

#[test]
fn describe_builds_expected_enl_args() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["describe", "living_aircon", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let expected = serde_json::json!(["describe", "--ip", "192.0.2.10", "--eoj", "0x013001"]);
    assert_eq!(v["properties"]["args"], expected);
}

#[test]
fn describe_outputs_property_map_in_casa_schema() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["describe", "living_aircon", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_describe.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    assert!(v["timestamp"].is_string());
    assert_eq!(v["device"], "living_aircon");
    assert_eq!(v["protocol"], "echonet");
    assert_eq!(v["properties"]["get"][0], "0x80");
}

#[test]
fn describe_unsupported_protocol_exits_14() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["describe", "entry_lock", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_describe.sh"))],
    );
    assert_eq!(out.status.code(), Some(14));
    assert_eq!(
        stderr_error_json(&out)["error"]["kind"],
        "protocol_unsupported"
    );
}

#[test]
fn on_maps_to_echonet_epc_0x80_value_0x30() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["on", "living_aircon", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let expected = serde_json::json!([
        "set",
        "--ip",
        "192.0.2.10",
        "--eoj",
        "0x013001",
        "--epc",
        "0x80",
        "--value",
        "0x30"
    ]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn off_maps_to_echonet_epc_0x80_value_0x31() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["off", "bedroom_light", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let expected = serde_json::json!([
        "set",
        "--ip",
        "192.0.2.11",
        "--eoj",
        "0x029101",
        "--epc",
        "0x80",
        "--value",
        "0x31"
    ]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn on_unsupported_protocol_exits_14() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["on", "entry_lock", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(14));
    assert_eq!(
        stderr_error_json(&out)["error"]["kind"],
        "protocol_unsupported"
    );
}

#[test]
fn list_describe_includes_property_maps() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["list", "--describe", "--config", &config],
        &[
            ("CASA_ENL_BIN", &fixture("enl_describe.sh")),
            ("CASA_MAT_BIN", &fixture("mat_describe.sh")),
        ],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let devices = v["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 5);

    let aircon = devices
        .iter()
        .find(|d| d["name"] == "living_aircon")
        .unwrap();
    assert_eq!(aircon["properties"]["get"][0], "0x80");

    let hall = devices.iter().find(|d| d["name"] == "hall_light").unwrap();
    assert_eq!(hall["properties"]["endpoints"][0]["endpoint"], 0);

    // introspection 未対応プロトコルは properties: null。
    let lock = devices.iter().find(|d| d["name"] == "entry_lock").unwrap();
    assert!(lock["properties"].is_null());
}

#[test]
fn plain_list_does_not_call_child_cli() {
    let (config, _dir) = setup();
    // 子 CLI が存在しなくても list は成功する＝呼んでいない。
    let out = run_casa(
        &["list", "--config", &config],
        &[("CASA_ENL_BIN", "/nonexistent/enl")],
    );
    assert_eq!(out.status.code(), Some(0));
    let v = stdout_json(&out);
    assert!(v["devices"][0]["properties"].is_null() || v["devices"][0].get("properties").is_none());
}
