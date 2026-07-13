//! Phase 4: ダミー mat バイナリを使った Matter アダプタの統合テスト。
//! CI で実 mat / 実機（chip-tool）は不要。実機相手の手動 E2E は README を参照。

mod common;

use common::*;

const MATTER_CONFIG: &str = r#"
version = 1

[devices.living_light]
protocol = "matter"
node_id = "1234"

[devices.power_strip_outlet2]
protocol = "matter"
node_id = "5678"
endpoint = 2
"#;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn setup() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    (config.to_str().unwrap().to_string(), dir)
}

#[test]
fn get_reshapes_mat_output_into_casa_schema() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "living_light", "1/onoff/on-off", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("mat_ok.sh"))],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v = stdout_json(&out);
    assert_eq!(v["device"], "living_light");
    assert_eq!(v["protocol"], "matter");
    assert_eq!(v["value"]["value"], "on");
    assert!(v["timestamp"].is_string(), "timestamp missing: {v}");
}

#[test]
fn get_splits_selector_into_mat_read_args() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "living_light", "1/onoff/on-off", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let expected = serde_json::json!([
        "read",
        "--node",
        "1234",
        "--endpoint",
        "1",
        "--cluster",
        "onoff",
        "--attribute",
        "on-off"
    ]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn set_builds_mat_write_args() {
    let (config, _dir) = setup();
    let out = run_casa(
        &[
            "set",
            "living_light",
            "1/levelcontrol/current-level",
            "128",
            "--config",
            &config,
        ],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let expected = serde_json::json!([
        "write",
        "--node",
        "1234",
        "--endpoint",
        "1",
        "--cluster",
        "levelcontrol",
        "--attribute",
        "current-level",
        "--value",
        "128"
    ]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn describe_builds_mat_describe_args() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["describe", "living_light", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let expected = serde_json::json!(["describe", "--node", "1234"]);
    assert_eq!(v["properties"]["args"], expected);
}

#[test]
fn on_without_endpoint_omits_flag() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["on", "living_light", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let expected = serde_json::json!(["on", "--node", "1234"]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn off_with_endpoint_passes_flag() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["off", "power_strip_outlet2", "--config", &config],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let expected = serde_json::json!(["off", "--node", "5678", "--endpoint", "2"]);
    assert_eq!(v["value"]["args"], expected);
}
