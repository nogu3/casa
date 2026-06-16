//! Phase 0: `casa list` と設定ローダ・exit code 規約の統合テスト。

mod common;

use common::*;

#[test]
fn list_outputs_all_devices_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casa(&["list", "--config", config.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    assert!(v["timestamp"].is_string(), "timestamp missing: {v}");
    let devices = v["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 3);

    let aircon = devices
        .iter()
        .find(|d| d["name"] == "living_aircon")
        .unwrap();
    assert_eq!(aircon["protocol"], "echonet");
    assert_eq!(aircon["ip"], "192.0.2.10");
    assert_eq!(aircon["eoj"], "0x013001");

    let lock = devices.iter().find(|d| d["name"] == "entry_lock").unwrap();
    assert_eq!(lock["protocol"], "switchbot");
    assert_eq!(lock["device_id"], "DUMMY-XX-XX");
}

#[test]
fn config_path_can_come_from_env() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casa(&["list"], &[("CASA_CONFIG", config.to_str().unwrap())]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_json(&out)["devices"].as_array().unwrap().len(), 3);
}

#[test]
fn missing_config_exits_10_with_structured_error() {
    let out = run_casa(&["list", "--config", "/nonexistent/casa/devices.toml"], &[]);
    assert_eq!(out.status.code(), Some(10));
    assert!(out.stdout.is_empty());
    let err = stderr_error_json(&out);
    assert_eq!(err["error"]["kind"], "config_missing");
    assert!(err["error"]["detail"].is_string());
}

#[test]
fn broken_toml_exits_10_with_config_parse() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), "version = [broken");

    let out = run_casa(&["list", "--config", config.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(10));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "config_parse");
}

#[test]
fn unknown_protocol_exits_10() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(
        dir.path(),
        "version = 1\n[devices.x]\nprotocol = \"zigbee\"\n",
    );

    let out = run_casa(&["list", "--config", config.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(10));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "config_parse");
}

#[test]
fn invalid_cli_args_exit_2() {
    let out = run_casa(&["no-such-subcommand"], &[]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn example_config_in_repo_is_valid() {
    // examples/ は workspace ルート直下のサンプル（README 参照）。casa crate からは 2 つ上。
    let example = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/devices.toml");
    let out = run_casa(&["list", "--config", example], &[]);
    assert_eq!(out.status.code(), Some(0));
    let v = stdout_json(&out);
    assert_eq!(v["devices"].as_array().unwrap().len(), 3);
}
