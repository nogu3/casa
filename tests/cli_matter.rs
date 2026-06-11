//! Matter（mat アダプタ）の統合テスト。ダミー mat を使い、CI で実 mat は不要。

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
fn get_builds_mat_read_args() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "hall_light", "onoff/on-off", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v = stdout_json(&out);
    assert_eq!(v["device"], "hall_light");
    assert_eq!(v["protocol"], "matter");
    assert_eq!(
        v["value"]["args"],
        serde_json::json!(["read", "5", "1", "onoff", "on-off"])
    );
}

#[test]
fn set_builds_mat_write_args_with_configured_endpoint() {
    let (config, _dir) = setup();
    let out = run_casa(
        &[
            "set",
            "desk_lamp",
            "levelcontrol/on-level",
            "128",
            "--config",
            &config,
        ],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    assert_eq!(
        v["value"]["args"],
        serde_json::json!(["write", "7", "2", "levelcontrol", "on-level", "128"])
    );
}

#[test]
fn on_maps_to_mat_on_shortcut() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["on", "hall_light", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stdout_json(&out)["value"]["args"],
        serde_json::json!(["on", "5", "--endpoint", "1"])
    );
}

#[test]
fn off_maps_to_mat_off_shortcut_with_endpoint() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["off", "desk_lamp", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stdout_json(&out)["value"]["args"],
        serde_json::json!(["off", "7", "--endpoint", "2"])
    );
}

#[test]
fn describe_builds_mat_describe_args_and_reshapes() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["describe", "hall_light", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("mat_describe.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    assert_eq!(v["device"], "hall_light");
    assert_eq!(v["protocol"], "matter");
    assert_eq!(v["properties"]["endpoints"][1]["clusters"][0], 6);
}

#[test]
fn malformed_property_exits_2_with_invalid_argument() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "hall_light", "on-off", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "invalid_argument");
}

#[test]
fn mat_exit_code_is_propagated() {
    let (config, _dir) = setup();
    // mat のタイムアウト（exit 3）を enl 用フィクスチャで代用（中身は exit 3 するだけ）。
    let out = run_casa(
        &["get", "hall_light", "onoff/on-off", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("enl_exit3.sh"))],
    );
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "child_failed");
}

#[test]
fn missing_mat_binary_exits_12() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["on", "hall_light", "--config", &config],
        &[("CASA_MAT_BIN", "/nonexistent/mat")],
    );
    assert_eq!(out.status.code(), Some(12));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "child_not_found");
}

#[test]
fn list_includes_matter_devices() {
    let (config, _dir) = setup();
    let out = run_casa(&["list", "--config", &config], &[]);
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let lamp = v["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "desk_lamp")
        .unwrap()
        .clone();
    assert_eq!(lamp["protocol"], "matter");
    assert_eq!(lamp["node_id"], 7);
    assert_eq!(lamp["endpoint"], 2);
}
