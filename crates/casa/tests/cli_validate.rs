//! `casa validate` の統合テスト。設定を読んで妥当性を JSON 報告し、
//! validate の summary 出力（device_count・プロトコル内訳）と warnings・exit code 規約を確認する。

mod common;

use common::*;

#[test]
fn validate_reports_summary_for_multi_protocol_config() {
    let dir = tempfile::tempdir().unwrap();
    // DUMMY_CONFIG: echonet 2 + switchbot 1。全プロトコルにアダプタがあるので警告は出ない。
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casa(&["validate", "--config", config.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(0));

    let v = stdout_json(&out);
    assert!(v["timestamp"].is_string(), "timestamp missing: {v}");
    assert_eq!(v["valid"], true);
    assert_eq!(v["version"], 1);
    assert_eq!(v["device_count"], 3);
    assert_eq!(v["protocols"]["echonet"], 2);
    assert_eq!(v["protocols"]["switchbot"], 1);

    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 0);
}

#[test]
fn validate_on_invalid_config_exits_10() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), "version = 1\n[devices.x]\nprotocol = \"zigbee\"\n");

    let out = run_casa(&["validate", "--config", config.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(10));
    assert!(out.stdout.is_empty());
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "config_parse");
}

#[test]
fn validate_clean_config_has_no_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(
        dir.path(),
        "version = 1\n[devices.light]\nprotocol = \"matter\"\nnode_id = \"1234\"\n",
    );

    let out = run_casa(&["validate", "--config", config.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(0));
    let v = stdout_json(&out);
    assert_eq!(v["warnings"].as_array().unwrap().len(), 0);
    assert_eq!(v["protocols"]["matter"], 1);
}
