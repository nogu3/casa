//! Phase 1: ダミー enl バイナリを使った get / set の統合テスト。
//! CI で実 enl は不要。実機相手の手動 E2E は README を参照。

mod common;

use common::*;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// ダミー設定を書き、(設定パス, _tempdir ガード) を返す。
fn setup() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    (config.to_str().unwrap().to_string(), dir)
}

#[test]
fn get_reshapes_enl_output_into_casa_schema() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "living_aircon", "0x80", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_ok.sh"))],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v = stdout_json(&out);
    assert!(v["timestamp"].is_string());
    assert_eq!(v["device"], "living_aircon");
    assert_eq!(v["protocol"], "echonet");
    assert_eq!(v["value"]["power"], "on");
}

#[test]
fn get_builds_expected_enl_args() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "living_aircon", "0x80", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    let expected = serde_json::json!([
        "get",
        "--ip",
        "192.0.2.10",
        "--eoj",
        "0x013001",
        "--epc",
        "0x80"
    ]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn set_builds_expected_enl_args() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["set", "bedroom_light", "0x80", "0x30", "--config", &config],
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
        "0x30"
    ]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn unknown_name_exits_11() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "no_such_device", "0x80", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_ok.sh"))],
    );
    assert_eq!(out.status.code(), Some(11));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "name_not_found");
}

#[test]
fn missing_child_binary_exits_12() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "living_aircon", "0x80", "--config", &config],
        &[("CASA_ENL_BIN", "/nonexistent/enl")],
    );
    assert_eq!(out.status.code(), Some(12));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "child_not_found");
}

#[test]
fn child_exit_code_is_propagated() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "living_aircon", "0x80", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_exit3.sh"))],
    );
    // enl がタイムアウト（3）で終了したら casa も 3 で終了する。
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "child_failed");
}

#[test]
fn child_stderr_is_forwarded_at_debug_level() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "living_aircon", "0x80", "--config", &config],
        &[
            ("CASA_ENL_BIN", &fixture("enl_exit3.sh")),
            ("RUST_LOG", "debug"),
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("device did not respond"),
        "child stderr not forwarded: {stderr}"
    );
}

#[test]
fn invalid_child_json_exits_13() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "living_aircon", "0x80", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_badjson.sh"))],
    );
    assert_eq!(out.status.code(), Some(13));
    assert_eq!(
        stderr_error_json(&out)["error"]["kind"],
        "child_invalid_output"
    );
}

#[test]
fn unsupported_protocol_exits_14() {
    let (config, _dir) = setup();
    let out = run_casa(
        &["get", "entry_lock", "0x80", "--config", &config],
        &[("CASA_ENL_BIN", &fixture("enl_ok.sh"))],
    );
    assert_eq!(out.status.code(), Some(14));
    assert_eq!(
        stderr_error_json(&out)["error"]["kind"],
        "protocol_unsupported"
    );
}

#[test]
fn binary_can_be_overridden_via_config_binaries() {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!(
        "{DUMMY_CONFIG}\n[binaries]\nenl = \"{}\"\n",
        fixture("enl_ok.sh")
    );
    let config = write_config(dir.path(), &toml);

    let out = run_casa(
        &[
            "get",
            "living_aircon",
            "0x80",
            "--config",
            config.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout_json(&out)["value"]["power"], "on");
}
