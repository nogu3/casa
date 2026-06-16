//! casad exec の統合テスト。casa の代役（casa_stub.sh）を CASA_BIN で差し込み、
//! ハイブリッド境界（link 側で名前解決 / spawn 側で casa 起動）を検証する。

mod common;

use common::*;

#[test]
fn exec_on_spawns_casa_with_mapped_args() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casad(
        &[
            "exec",
            "on",
            "living_aircon",
            "--config",
            config.to_str().unwrap(),
        ],
        &[("CASA_BIN", &fixture("casa_stub.sh"))],
    );

    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // casad は casa へ `on living_aircon --config <path>` を渡す。
    assert!(
        stdout.contains("on living_aircon --config"),
        "stdout: {stdout}"
    );
}

#[test]
fn exec_propagates_casa_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casad(
        &[
            "exec",
            "off",
            "living_aircon",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("CASA_FAKE_EXIT", "7"),
        ],
    );

    // casa の exit code はそのまま伝播する（上層がリトライ判断できる）。
    assert_eq!(out.status.code(), Some(7));
}

#[test]
fn exec_unknown_name_fails_without_spawning_casa() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    // casa が起動されれば exit 99 になる細工。実際は link 側の名前解決で弾かれ、
    // casa は起動されず name_not_found(11) になるはず。
    let out = run_casad(
        &["exec", "on", "nope", "--config", config.to_str().unwrap()],
        &[
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("CASA_FAKE_EXIT", "99"),
        ],
    );

    assert_eq!(out.status.code(), Some(11));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "name_not_found");
}

#[test]
fn exec_missing_config_exits_10() {
    let out = run_casad(
        &[
            "exec",
            "on",
            "living_aircon",
            "--config",
            "/no/such/devices.toml",
        ],
        &[("CASA_BIN", &fixture("casa_stub.sh"))],
    );

    assert_eq!(out.status.code(), Some(10));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "config_missing");
}
