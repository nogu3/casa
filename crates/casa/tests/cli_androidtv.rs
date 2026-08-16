//! ダミー atv バイナリを使った Android TV アダプタの統合テスト。
//! CI で実 atv / 実機（TV）は不要。実機相手の手動 E2E は README を参照。

mod common;

use common::*;

const ANDROIDTV_CONFIG: &str = r#"
version = 1

[devices.living_tv]
protocol = "androidtv"
host = "192.0.2.10"
"#;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn setup() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), ANDROIDTV_CONFIG);
    (config.to_str().unwrap().to_string(), dir)
}

#[test]
fn on_builds_atv_on_with_host_flag() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["on", "living_tv", "--config", &config],
        &[("CASA_ATV_BIN", &fixture("atv_args.sh"))],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v = stdout_json(&out);
    assert_eq!(v["device"], "living_tv");
    assert_eq!(v["protocol"], "androidtv");
    assert_eq!(
        v["value"]["args"],
        serde_json::json!(["on", "--host", "192.0.2.10"])
    );
    assert!(v["timestamp"].is_string(), "timestamp missing: {v}");
}

#[test]
fn off_builds_atv_off_with_host_flag() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["off", "living_tv", "--config", &config],
        &[("CASA_ATV_BIN", &fixture("atv_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    assert_eq!(
        v["value"]["args"],
        serde_json::json!(["off", "--host", "192.0.2.10"])
    );
}

#[test]
fn invoke_status_injects_host_flag() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["invoke", "--config", &config, "living_tv", "status"],
        &[("CASA_ATV_BIN", &fixture("atv_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    assert_eq!(v["command"], "status");
    assert_eq!(
        v["value"]["args"],
        serde_json::json!(["status", "--host", "192.0.2.10"])
    );
}

#[test]
fn get_is_protocol_unsupported() {
    let (config, _dir) = setup();
    let out = run_casa(&["get", "living_tv", "power", "--config", &config], &[]);
    assert_eq!(out.status.code(), Some(14));
    assert_eq!(
        stderr_error_json(&out)["error"]["kind"],
        "protocol_unsupported"
    );
}
