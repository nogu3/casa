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

/// 失敗経路でも常駐 casad を確実に殺すガード（casad は自発終了しないため、
/// kill 前の assert が panic すると不死のプロセスが残る）。
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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

    let _child = KillOnDrop(
        std::process::Command::new(env!("CARGO_BIN_EXE_casad"))
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
            .unwrap(),
    );

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

    assert!(fired, "casa was not fired via worker within 10s");
    // 明示 kill は不要（KillOnDrop がスコープ終了時に必ず殺す）。
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

const MATTER_BURST_RULES: &str = r#"
version = 1
[[rules]]
name = "人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
[[rules]]
name = "人感ONで点灯"
when = { device = "study_motion", attribute = "occupancy", equals = 1 }
then = { action = "on", device = "desk_tape_light" }
"#;

/// 常駐 Matter リスナは `mat listen` を 1 本のストリームとして維持し、バーストの
/// 全イベントを取りこぼさない（2026-07-27 の recovered 取りこぼし事象の再発防止）。
/// mat 代役はバースト 3 行（priming 1 + 実イベント 2）を出した後ブロックし続ける
/// ため、child の終了を待つ one-shot 実装ではひとつも発火できない。
#[test]
fn resident_matter_stream_consumes_burst_without_respawn() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules_path = dir.path().join("rules.toml");
    std::fs::write(&rules_path, MATTER_BURST_RULES).unwrap();

    let _child = KillOnDrop(
        std::process::Command::new(env!("CARGO_BIN_EXE_casad"))
            .args([
                "run",
                rules_path.to_str().unwrap(),
                "--config",
                config.to_str().unwrap(),
            ])
            .env_remove("CASA_CONFIG")
            .env("CASA_MAT_BIN", fixture("mat_listen_stream.sh"))
            .env("CASA_BIN", fixture("casa_record.sh"))
            .env("CASAD_TEST_DIR", dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );

    // バーストの実イベント 2 件（off → on、同一デバイスなので FIFO）が両方
    // 発火するまで待つ。casa 代役は 1 呼び出し 2 秒なので 4 秒 + 余裕。
    let casa_log = dir.path().join("casa.log");
    let deadline = Instant::now() + Duration::from_secs(15);
    let fired = loop {
        let log = std::fs::read_to_string(&casa_log).unwrap_or_default();
        if log.contains("off desk_tape_light") && log.contains("on desk_tape_light") {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        fired,
        "burst events were not all fired within 15s: {:?}",
        std::fs::read_to_string(&casa_log)
    );

    let log = std::fs::read_to_string(&casa_log).unwrap();
    // priming（occupancy=1）は発火しない — on は recovered イベントの 1 回だけ。
    assert_eq!(
        log.matches("on desk_tape_light").count(),
        1,
        "priming event must not fire: {log}"
    );

    // ストリームは 1 本のみ（イベントごとの再 spawn をしない）で、無期限
    // ストリーム指定（--count 0）で起動されている。
    let spawns = std::fs::read_to_string(dir.path().join("mat_spawns.log")).unwrap();
    let spawn_lines: Vec<&str> = spawns.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(spawn_lines.len(), 1, "mat must be spawned once: {spawns}");
    assert!(
        spawn_lines[0].contains("--count 0"),
        "mat listen must run unbounded: {spawns}"
    );
}
