//! ルールエンジン。トリガを評価して casa アクションを発火する。
//!
//! W3a 時点では**時刻トリガのみ**。イベントトリガ（`enl listen` をループで回して
//! 状変通知に反応する）は後段 W3b で「もう一つの入力源」として足す。
//!
//! 発火粒度は分。常駐モード（`casad run`）は毎分の境界で tick し、`--once` モードは
//! 1 回だけ評価して終了する（cron から毎分呼ぶ運用やデバッグに使える）。

use std::path::Path;

use chrono::{Local, NaiveTime, Timelike};

use casa_core::error::{CasaError, ErrorKind};

use crate::casa_runner;
use crate::rules::{Rule, RuleFile, Trigger};

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
            Trigger::Time { at } => {
                parse_hm(at).map(|t| (t.hour(), t.minute()) == now_hm).unwrap_or(false)
            }
            Trigger::Event { .. } => false,
        })
        .collect()
}

/// 1 つのルールの `then` を casa の spawn で実行する。
pub fn fire(rule: &Rule, config: Option<&Path>) -> Result<i32, CasaError> {
    let args = rule.then.action.casa_args(&rule.then.device, config);
    tracing::info!(rule = %rule.name, "firing rule");
    casa_runner::run_casa(&args)
}

/// now に発火すべき時刻ルールをすべて実行する。発火した件数を返す。
/// 個々のアクション失敗はループを止めず warn ログに残す（常駐の頑健性）。
pub fn tick(file: &RuleFile, now: NaiveTime, config: Option<&Path>) -> usize {
    let due = due_time_rules(file, now);
    for rule in &due {
        if let Err(e) = fire(rule, config) {
            tracing::warn!(rule = %rule.name, error = %e, "rule action failed");
        }
    }
    due.len()
}

/// ルールエンジンを走らせる。`--once` は 1 tick で終了、常駐は毎分 tick。
pub fn run(file: &RuleFile, config: Option<&Path>, opts: RunOpts) -> Result<i32, CasaError> {
    if opts.once {
        let now = opts.now.unwrap_or_else(|| Local::now().time());
        let fired = tick(file, now, config);
        tracing::info!(fired, ?now, "single tick complete");
        return Ok(0);
    }

    tracing::info!("casad resident scheduler started");
    loop {
        let now = Local::now().time();
        tick(file, now, config);
        sleep_to_next_minute();
    }
}

/// 次の分境界までスリープする。
fn sleep_to_next_minute() {
    let secs = 60u64.saturating_sub(Local::now().second() as u64).max(1);
    std::thread::sleep(std::time::Duration::from_secs(secs));
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
