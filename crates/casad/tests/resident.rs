//! 常駐モード（`casad run`、--once / --listen-once なし）の統合テスト。
//! enl 代役が 1 件通知 → デバイス別ワーカー経由で casa 代役が発火することを
//! ファイル観測で検証する。常駐は終了しないため、発火確認後に kill する。
//!
//! casa 代役は 2 秒 sleep してから記録するよう遅延させてある。これにより
//! 「casa アクションの実行中に enl が再 spawn される」という順序が観測できる。
//! 旧同期実装では発火（casa 呼び出し）が完了するまでイベントループが次の
//! enl 起動に進めないため、この順序は非同期ワーカー経由でのみ成立する。
//! よって本テストは同期実装では（2 回目の enl spawn を待つうちに 10 秒の
//! デッドラインで）タイムアウトし、非同期実装のみを判別する。

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

    let spawns_log = dir.path().join("enl_spawns.log");
    let casa_log = dir.path().join("casa.log");

    // (a) enl_spawns.log が 2 行になるまでポーリングする。casa 代役は 2 秒
    // sleep してから casa.log に書くので、この時点でまだ casa アクションは
    // 実行中のはず。同期実装では発火完了まで次の enl 起動に進めないため、
    // 2 行目が現れるとしても必ず casa.log 書き込み後になる。
    let spawn_deadline = Instant::now() + Duration::from_secs(10);
    let respawned = loop {
        let count = std::fs::read_to_string(&spawns_log)
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0);
        if count >= 2 {
            break true;
        }
        if Instant::now() > spawn_deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(respawned, "enl was not re-spawned within 10s");

    // (b) 判別の核心: 2 回目の enl spawn が観測できた瞬間、casa アクション
    // （2 秒 sleep 後に書かれる）はまだ完了していないはず。同期実装なら
    // ここで casa.log が既に存在してしまう（enl の再起動より前に発火が
    // 完了していなければ 2 回目の spawn 自体に到達できないため）。
    let casa_already_done = std::fs::read_to_string(&casa_log)
        .map(|s| s.contains("on living_aircon"))
        .unwrap_or(false);
    assert!(
        !casa_already_done,
        "casa action already completed before enl was re-spawned; \
         this does not discriminate async from sync firing"
    );

    // (c) それでもアクション自体はきちんと完了することを確認する。
    let fire_deadline = Instant::now() + Duration::from_secs(10);
    let fired = loop {
        if std::fs::read_to_string(&casa_log)
            .map(|s| s.contains("on living_aircon"))
            .unwrap_or(false)
        {
            break true;
        }
        if Instant::now() > fire_deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    child.kill().unwrap();
    let _ = child.wait();
    assert!(fired, "casa was not fired via worker within 10s");
}
