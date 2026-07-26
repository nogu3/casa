//! casad run --listen-once の統合テスト。enl 代役（enl_listen.sh）が固定の INF 通知を
//! 出し、casad がそれをイベントトリガに突合して casa 代役を発火することを検証する。

mod common;

use common::*;

const EVENT_RULES: &str = r#"
version = 1
[[rules]]
name = "エアコン電源ONで点灯"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "living_aircon" }
"#;

fn write_rules(dir: &std::path::Path, text: &str) -> std::path::PathBuf {
    let path = dir.join("rules.toml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn listen_once_fires_event_rule_via_casa() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), EVENT_RULES);

    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_ENL_BIN", &fixture("enl_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
        ],
    );

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // 通知（EPC80=30）が rule に一致し、casa に `on living_aircon` が渡る。
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("on living_aircon"), "stdout: {stdout}");
}

#[test]
fn listen_once_warns_when_casa_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), EVENT_RULES);

    // casa 代役が exit 3（enl のタイムアウト相当）で失敗するケース。
    // エンジンは止まらない（exit 0）が、失敗は warn として stderr に残ること。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_ENL_BIN", &fixture("enl_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("CASA_FAKE_EXIT", "3"),
            ("RUST_LOG", "warn"),
        ],
    );

    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("rule action exited nonzero"),
        "stderr: {stderr}"
    );
    // どのルールが何の exit code で失敗したか特定できること。
    assert!(stderr.contains("エアコン電源ONで点灯"), "stderr: {stderr}");
    assert!(stderr.contains("\"code\":3"), "stderr: {stderr}");
}

#[test]
fn listen_once_does_not_fire_on_value_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), EVENT_RULES);

    // 通知の EDT を 31(OFF) に上書き。rule は 0x30 を待つので一致しない。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_ENL_BIN", &fixture("enl_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("CASAD_ENL_EDT", "31"),
        ],
    );

    assert_eq!(out.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("casa called"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

const WINDOWED_EVENT_RULES: &str = r#"
version = 1
[[rules]]
name = "エアコン電源ONで点灯（昼だけ）"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
active = { from = "06:00", to = "21:00" }
then = { action = "on", device = "living_aircon" }
"#;

#[test]
fn listen_once_fires_inside_active_window() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), WINDOWED_EVENT_RULES);

    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once",
            "--now",
            "12:00",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_ENL_BIN", &fixture("enl_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
        ],
    );

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // 12:00 は 06:00-21:00 の窓の中。通知が一致し casa に `on living_aircon` が渡る。
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("on living_aircon"), "stdout: {stdout}");
}

#[test]
fn listen_once_does_not_fire_outside_active_window() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), WINDOWED_EVENT_RULES);

    // 22:00 は 06:00-21:00 の窓の外。通知が一致しても発火しない。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once",
            "--now",
            "22:00",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_ENL_BIN", &fixture("enl_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
        ],
    );

    assert_eq!(out.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("casa called"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

const MATTER_CONFIG: &str = r#"
version = 1

[devices.study_motion]
protocol = "matter"
node_id = "16"

[devices.desk_tape_light]
protocol = "matter"
node_id = "6"
"#;

const MATTER_RULES: &str = r#"
version = 1
[[rules]]
name = "書斎 人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#;

#[test]
fn listen_once_mat_fires_matter_rule_via_casa() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), MATTER_RULES);

    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once-mat",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_MAT_BIN", &fixture("mat_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
        ],
    );

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // occupancy=0 が rule に一致し、casa に `off desk_tape_light` が渡る。
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("off desk_tape_light"), "stdout: {stdout}");
}

#[test]
fn listen_once_mat_does_not_fire_on_priming() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), MATTER_RULES);

    // priming（matd 再購読時の現在値再配達）は値が一致しても発火しない。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once-mat",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_MAT_BIN", &fixture("mat_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("CASAD_MAT_PRIMING", "true"),
        ],
    );

    assert_eq!(out.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("casa called"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn listen_once_mat_does_not_fire_on_value_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), MATTER_RULES);

    // occupancy=1（在室）は「OFF で消灯」ルールに一致しない。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once-mat",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_MAT_BIN", &fixture("mat_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("CASAD_MAT_VALUE", "1"),
        ],
    );

    assert_eq!(out.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("casa called"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

const WINDOWED_MATTER_RULES: &str = r#"
version = 1
[[rules]]
name = "書斎 不在で消灯（昼だけ）"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
active = { from = "06:00", to = "21:00" }
then = { action = "off", device = "desk_tape_light" }
"#;

#[test]
fn listen_once_mat_fires_inside_active_window() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), WINDOWED_MATTER_RULES);

    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once-mat",
            "--now",
            "12:00",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_MAT_BIN", &fixture("mat_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
        ],
    );

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("off desk_tape_light"), "stdout: {stdout}");
}

#[test]
fn listen_once_mat_does_not_fire_outside_active_window() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), WINDOWED_MATTER_RULES);

    // 22:00 は 06:00-21:00 の窓の外。イベントが一致しても発火しない。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once-mat",
            "--now",
            "22:00",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_MAT_BIN", &fixture("mat_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("RUST_LOG", "debug"),
        ],
    );

    assert_eq!(out.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("casa called"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    // 「窓外でルールがスキップされた」ことが debug ログで追える（切り分けコスト低減が目的）。
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("outside its active window"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("書斎 不在で消灯（昼だけ）"),
        "stderr: {stderr}"
    );
}
