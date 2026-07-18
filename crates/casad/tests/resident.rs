//! 常駐モード（`casad run`、--once / --listen-once なし）の統合テスト。
//! enl 代役が 1 件通知 → デバイス別ワーカー経由で casa 代役が発火することを
//! ファイル観測で検証する。常駐は終了しないため、発火確認後に kill する。

mod common;

use std::time::{Duration, Instant};

use common::*;

const EVENT_RULES: &str = r#"
version = 1
[[rules]]
name = "エアコン電源ONで点灯"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "living_aircon" }
"#;

#[test]
fn resident_event_loop_fires_casa_via_worker() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules_path = dir.path().join("rules.toml");
    std::fs::write(&rules_path, EVENT_RULES).unwrap();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_casad"))
        .args([
            "run",
            rules_path.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ])
        .env_remove("CASA_CONFIG")
        .env("CASA_ENL_BIN", fixture("enl_listen_once_then_block.sh"))
        .env("CASA_BIN", fixture("casa_record.sh"))
        .env("CASAD_TEST_DIR", dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    // ワーカー経由の発火は非同期なのでファイル出現をポーリングで待つ。
    let log = dir.path().join("casa.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    let fired = loop {
        if std::fs::read_to_string(&log)
            .map(|s| s.contains("on living_aircon"))
            .unwrap_or(false)
        {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    child.kill().unwrap();
    let _ = child.wait();
    assert!(fired, "casa was not fired via worker within 10s");
}
