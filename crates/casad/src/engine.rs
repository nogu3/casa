//! ルールエンジン。トリガを評価して casa アクションを発火する。
//!
//! W3a 時点では**時刻トリガのみ**。イベントトリガ（`enl listen` をループで回して
//! 状変通知に反応する）は後段 W3b で「もう一つの入力源」として足す。
//!
//! 発火粒度は分。常駐モード（`casad run`）は毎分の境界で tick し、`--once` モードは
//! 1 回だけ評価して終了する（cron から毎分呼ぶ運用やデバッグに使える）。

use std::path::Path;
use std::time::Duration;

use chrono::{Local, NaiveTime, Timelike};

use casa_core::config::{Config, Device};
use casa_core::error::{CasaError, ErrorKind};

use crate::rules::{Rule, RuleFile, Trigger};
use crate::{casa_runner, enl};

/// `casad run` の挙動。
pub struct RunOpts {
    /// true なら 1 回だけ評価して終了する。false なら常駐して毎分 tick する。
    pub once: bool,
    /// 現在時刻の上書き（`--once` 併用のデバッグ用）。None なら実時計。
    pub now: Option<NaiveTime>,
}

/// "HH:MM" を時刻にパースする。秒は持たない（分粒度で発火）。
pub fn parse_hm(s: &str) -> Result<NaiveTime, CasaError> {
    NaiveTime::parse_from_str(s, "%H:%M").map_err(|e| {
        CasaError::new(
            ErrorKind::ConfigParse,
            format!("invalid time \"{s}\" (expected HH:MM): {e}"),
        )
    })
}

/// すべての時刻トリガが正しい HH:MM か検証する。`casad check` / `run` の両方が使う。
pub fn validate_schedule(file: &RuleFile) -> Result<(), CasaError> {
    for rule in &file.rules {
        if let Trigger::Time { at } = &rule.when {
            parse_hm(at).map_err(|e| {
                CasaError::new(e.kind, format!("rule \"{}\": {}", rule.name, e.detail))
            })?;
        }
    }
    Ok(())
}

/// 現在時刻 now（分粒度）に発火すべき時刻トリガのルールを返す。
/// 不正な時刻のルールはここでは無視する（事前に [`validate_schedule`] で弾く前提）。
pub fn due_time_rules(file: &RuleFile, now: NaiveTime) -> Vec<&Rule> {
    let now_hm = (now.hour(), now.minute());
    file.rules
        .iter()
        .filter(|r| match &r.when {
            Trigger::Time { at } => parse_hm(at)
                .map(|t| (t.hour(), t.minute()) == now_hm)
                .unwrap_or(false),
            Trigger::Event { .. } => false,
        })
        .collect()
}

/// 1 つのルールの `then` を casa の spawn で実行する。
/// `config_path` は casa へ渡す `--config`（None なら casa が既定パスを解決）。
pub fn fire(rule: &Rule, config_path: Option<&Path>) -> Result<i32, CasaError> {
    let args = rule.then.action.casa_args(&rule.then.device, config_path);
    tracing::info!(rule = %rule.name, "firing rule");
    casa_runner::run_casa(&args)
}

/// now に発火すべき時刻ルールをすべて実行する。発火した件数を返す。
/// 個々のアクション失敗はループを止めず warn ログに残す（常駐の頑健性）。
pub fn tick(file: &RuleFile, now: NaiveTime, config_path: Option<&Path>) -> usize {
    fire_all(due_time_rules(file, now), config_path)
}

/// hex 文字列を正規化する（`0x`/`0X` 接頭辞を外し大文字化・トリム）。
/// DSL の "0x30" / "0x013001" と enl の "30" / "013001" を突合できるようにする。
fn norm_hex(s: &str) -> String {
    let t = s.trim();
    t.strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t)
        .to_uppercase()
}

/// イベントトリガのルールが、与えられた enl 通知 1 件に一致するか。
/// device 名を設定で解決し、Echonet 以外（enl の対象外）は一致しない。
pub fn event_matches(rule: &Rule, config: &Config, event: &enl::Event) -> bool {
    let Trigger::Event {
        device,
        epc,
        equals,
    } = &rule.when
    else {
        return false;
    };
    let (ip, eoj) = match config.device(device) {
        Ok(Device::Echonet { ip, eoj }) => (ip, eoj),
        _ => return false,
    };
    if event.ip != *ip || norm_hex(&event.seoj) != norm_hex(eoj) {
        return false;
    }
    event
        .properties
        .iter()
        .any(|p| norm_hex(&p.epc) == norm_hex(epc) && norm_hex(&p.edt_hex) == norm_hex(equals))
}

/// 1 バッチの通知に対し、一致するイベントトリガをそれぞれ 1 回ずつ発火する。
/// 同じルールが複数通知に一致しても発火は 1 回。発火した件数を返す。
pub fn fire_due_events(
    file: &RuleFile,
    config: &Config,
    events: &[enl::Event],
    config_path: Option<&Path>,
) -> usize {
    let due: Vec<&Rule> = file
        .rules
        .iter()
        .filter(|r| matches!(r.when, Trigger::Event { .. }))
        .filter(|r| events.iter().any(|e| event_matches(r, config, e)))
        .collect();
    fire_all(due, config_path)
}

/// `enl listen` を 1 回起動し、得た通知でイベントトリガを発火する。発火件数を返す。
pub fn drain_events_once(
    file: &RuleFile,
    config: &Config,
    enl_bin: &str,
    config_path: Option<&Path>,
) -> Result<usize, CasaError> {
    let events = enl::listen_once(enl_bin)?;
    Ok(fire_due_events(file, config, &events, config_path))
}

/// 与えられたルール群をすべて発火する。失敗はループを止めず warn ログに残す。発火数を返す。
fn fire_all(rules: Vec<&Rule>, config_path: Option<&Path>) -> usize {
    let mut fired = 0;
    for rule in rules {
        match fire(rule, config_path) {
            Ok(_) => fired += 1,
            Err(e) => tracing::warn!(rule = %rule.name, error = %e, "rule action failed"),
        }
    }
    fired
}

/// ルールエンジンを走らせる。`--once` は時刻 1 tick で終了、常駐は時刻スケジューラ
/// （毎分 tick）と イベントリスナ（enl listen ループ）を並行に回す。
pub fn run(
    file: &RuleFile,
    config: &Config,
    config_path: Option<&Path>,
    enl_bin: &str,
    opts: RunOpts,
) -> Result<i32, CasaError> {
    if opts.once {
        let now = opts.now.unwrap_or_else(|| Local::now().time());
        let fired = tick(file, now, config_path);
        tracing::info!(fired, ?now, "single tick complete");
        return Ok(0);
    }

    tracing::info!("casad resident engine started (time + event)");
    let has_events = file
        .rules
        .iter()
        .any(|r| matches!(r.when, Trigger::Event { .. }));

    // scope で借用を渡し、Arc/clone なしに 2 ループを並行させる。どちらも無限ループ。
    std::thread::scope(|s| {
        if has_events {
            s.spawn(|| event_loop(file, config, enl_bin, config_path));
        }
        time_loop(file, config_path);
    });
    Ok(0) // time_loop は戻らないので到達しない。
}

/// 時刻スケジューラ。毎分の境界で tick する。
fn time_loop(file: &RuleFile, config_path: Option<&Path>) -> ! {
    loop {
        let now = Local::now().time();
        tick(file, now, config_path);
        sleep_to_next_minute();
    }
}

/// イベントリスナ。`enl listen` を回し続け、通知でイベントトリガを発火する。
/// enl 起動失敗・異常終了はバックオフして再試行（常駐の頑健性）。
fn event_loop(file: &RuleFile, config: &Config, enl_bin: &str, config_path: Option<&Path>) -> ! {
    loop {
        match enl::listen_once(enl_bin) {
            Ok(events) => {
                fire_due_events(file, config, &events, config_path);
            }
            Err(e) => {
                tracing::warn!(error = %e, "enl listen failed; backing off");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

/// 次の分境界までスリープする。
fn sleep_to_next_minute() {
    let secs = 60u64.saturating_sub(Local::now().second() as u64).max(1);
    std::thread::sleep(Duration::from_secs(secs));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(toml: &str) -> RuleFile {
        crate::rules::parse(toml).unwrap()
    }

    fn at(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn parse_hm_accepts_padded_time() {
        assert_eq!(parse_hm("22:00").unwrap(), at(22, 0));
        assert_eq!(parse_hm("07:05").unwrap(), at(7, 5));
    }

    #[test]
    fn parse_hm_rejects_garbage() {
        assert_eq!(parse_hm("9am").unwrap_err().kind, ErrorKind::ConfigParse);
        assert_eq!(parse_hm("25:00").unwrap_err().kind, ErrorKind::ConfigParse);
    }

    const SCHEDULE: &str = r#"
version = 1
[[rules]]
name = "朝点灯"
when = { at = "07:00" }
then = { action = "on", device = "living_aircon" }
[[rules]]
name = "夜消灯"
when = { at = "22:00" }
then = { action = "off", device = "living_aircon" }
[[rules]]
name = "状変イベント"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "living_aircon" }
"#;

    #[test]
    fn due_returns_only_rules_matching_the_minute() {
        let file = rules(SCHEDULE);
        let due = due_time_rules(&file, at(22, 0));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "夜消灯");
    }

    #[test]
    fn due_ignores_non_matching_minute_and_events() {
        let file = rules(SCHEDULE);
        assert!(due_time_rules(&file, at(12, 34)).is_empty());
        // イベントトリガは時刻評価では決して発火しない。
        assert!(due_time_rules(&file, at(7, 1)).is_empty());
    }

    fn config_living() -> Config {
        casa_core::config::parse(
            r#"
version = 1
[devices.living_aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"
[devices.entry_lock]
protocol = "switchbot"
device_id = "DUMMY"
"#,
        )
        .unwrap()
    }

    fn event(ip: &str, seoj: &str, epc: &str, edt: &str) -> enl::Event {
        enl::Event {
            ip: ip.into(),
            seoj: seoj.into(),
            properties: vec![enl::Prop {
                epc: epc.into(),
                edt_hex: edt.into(),
            }],
        }
    }

    const EVENT_RULE: &str = r#"
version = 1
[[rules]]
name = "電源ONで点灯"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "living_aircon" }
"#;

    #[test]
    fn event_matches_with_hex_normalization() {
        let file = rules(EVENT_RULE);
        let cfg = config_living();
        // DSL "0x80"/"0x30"/"0x013001" と enl "80"/"30"/"013001" が正規化で一致する。
        let ev = event("192.0.2.10", "013001", "80", "30");
        assert!(event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn event_does_not_match_on_value_or_source_mismatch() {
        let file = rules(EVENT_RULE);
        let cfg = config_living();
        // EDT 違い（OFF 通知）。
        assert!(!event_matches(
            &file.rules[0],
            &cfg,
            &event("192.0.2.10", "013001", "80", "31")
        ));
        // EPC 違い。
        assert!(!event_matches(
            &file.rules[0],
            &cfg,
            &event("192.0.2.10", "013001", "B0", "30")
        ));
        // 送信元 IP 違い。
        assert!(!event_matches(
            &file.rules[0],
            &cfg,
            &event("192.0.2.99", "013001", "80", "30")
        ));
        // EOJ 違い。
        assert!(!event_matches(
            &file.rules[0],
            &cfg,
            &event("192.0.2.10", "029101", "80", "30")
        ));
    }

    #[test]
    fn fire_due_events_matches_event_rules_only() {
        let file = rules(SCHEDULE); // 時刻 2 + イベント 1
        let cfg = config_living();
        let ev = event("192.0.2.10", "013001", "80", "30");
        // casa を起動しない範囲で「一致するイベントルールが1件ある」ことを確認する。
        let due: Vec<_> = file
            .rules
            .iter()
            .filter(|r| matches!(r.when, Trigger::Event { .. }))
            .filter(|r| event_matches(r, &cfg, &ev))
            .collect();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "状変イベント");
    }

    #[test]
    fn validate_schedule_flags_bad_time_with_rule_name() {
        let file = rules(
            r#"
version = 1
[[rules]]
name = "壊れた時刻"
when = { at = "7am" }
then = { action = "on", device = "living_aircon" }
"#,
        );
        let err = validate_schedule(&file).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("壊れた時刻"));
    }
}
