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

use crate::dispatch::{distinct_devices, Dispatcher, Job};
use crate::rules::{parse_node_id, ActiveWindow, Rule, RuleFile, Trigger};
use crate::{casa_runner, enl, mat};

/// `casad run` の挙動。
pub struct RunOpts {
    /// true なら 1 回だけ評価して終了する。false なら常駐して毎分 tick する。
    pub once: bool,
    /// 現在時刻の上書き（デバッグ用）。None なら実時計。
    /// `--now` は `--once`/`--listen-once`/`--listen-once-mat` の 3 経路で使えるが、
    /// `RunOpts`（延いてはこのフィールド）が実際に読まれるのは `--once` 経路のみ
    /// （他の 2 経路は `main.rs` が `now` をそのまま `drain_*_once` に渡す）。
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

/// ルールの有効時間帯を HH:MM 2 本として解析する。返り値は (from, to)。
/// 区間は `from` を含み `to` を含まない。`from > to` は日跨ぎを表す。
/// `from == to` は「空区間」とも「全日」とも読めるためエラーにする。
fn parse_active_window(w: &ActiveWindow) -> Result<(NaiveTime, NaiveTime), CasaError> {
    let from = parse_hm(&w.from)?;
    let to = parse_hm(&w.to)?;
    if from == to {
        return Err(CasaError::new(
            ErrorKind::ConfigParse,
            format!(
                "active window from and to are both \"{}\" (an empty window and an all-day window cannot be told apart)",
                w.from
            ),
        ));
    }
    Ok((from, to))
}

/// `now` が半開区間 `[from, to)` に入っているか。`from` を含み `to` を含まない。
/// `from > to` は日跨ぎとして扱う（`from == to` は呼び出し側で事前に弾く前提）。
///
/// [`rule_is_active`] と [`validate_schedule`] の両方がこれを使うことで、
/// 「窓に入っているか」の判定が 2 箇所で書かれてズレることを防ぐ。
fn time_in_window(now: NaiveTime, from: NaiveTime, to: NaiveTime) -> bool {
    if from < to {
        now >= from && now < to
    } else {
        now >= from || now < to // 日跨ぎ
    }
}

/// ルールが now の時点で有効か。`active` 未指定なら常に有効。
///
/// 解析できない窓は「無効」に倒す。起動前に [`validate_schedule`] が弾く前提だが、
/// 万一届いたら実機を動かさない側へ倒すのが安全側（不正な時刻の時刻トリガを
/// [`due_time_rules`] が発火させないのと同じ方針）。
fn rule_is_active(rule: &Rule, now: NaiveTime) -> bool {
    let Some(w) = &rule.active else {
        return true;
    };
    match parse_active_window(w) {
        Ok((from, to)) => time_in_window(now, from, to),
        Err(_) => false,
    }
}

/// 窓外で落としたルールを debug ログに残す。「センサーは反応しているのにルールが
/// 動かない」の切り分けコストを下げるため。**トリガに一致した後にだけ**呼ぶこと
/// （毎イベントで全ルール分のログを出さない）。
fn active_or_log(rule: &Rule, now: NaiveTime) -> bool {
    if rule_is_active(rule, now) {
        return true;
    }
    tracing::debug!(rule = %rule.name, %now, "rule skipped: outside its active window");
    false
}

/// すべての時刻トリガと有効時間帯が正しい HH:MM か検証する。
/// さらに、両方を持つルールについては `at` がその `active` 窓の中にあるかも検証する
/// （窓の外にある `at` は静的に「絶対発火しない」と分かるルールなので設定エラーにする）。
/// `casad check` / `run` の両方が使い、不正なルールで常駐が始まらないようにする。
pub fn validate_schedule(file: &RuleFile) -> Result<(), CasaError> {
    for rule in &file.rules {
        let at_time = match &rule.when {
            Trigger::Time { at } => Some((
                at,
                parse_hm(at).map_err(|e| {
                    CasaError::new(e.kind, format!("rule \"{}\": {}", rule.name, e.detail))
                })?,
            )),
            _ => None,
        };
        let window = match &rule.active {
            Some(w) => Some((
                w,
                parse_active_window(w).map_err(|e| {
                    CasaError::new(e.kind, format!("rule \"{}\": {}", rule.name, e.detail))
                })?,
            )),
            None => None,
        };
        if let (Some((at_str, at)), Some((w, (from, to)))) = (at_time, window) {
            if !time_in_window(at, from, to) {
                return Err(CasaError::new(
                    ErrorKind::ConfigParse,
                    format!(
                        "rule \"{}\": trigger time \"{}\" falls outside its own active window [\"{}\", \"{}\"), so this rule can never fire",
                        rule.name, at_str, w.from, w.to
                    ),
                ));
            }
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
            Trigger::MatterEvent { .. } => false,
        })
        .filter(|r| active_or_log(r, now))
        .collect()
}

/// 1 アクションを casa の spawn で実行する。
/// `config_path` は casa へ渡す `--config`（None なら casa が既定パスを解決）。
pub fn fire(job: Job<'_>, config_path: Option<&Path>) -> Result<i32, CasaError> {
    let args = job.then.casa_args(config_path);
    tracing::info!(
        rule = %job.rule.name,
        device = job.then.device(),
        action = job.then.action_name(),
        "firing rule"
    );
    casa_runner::run_casa(&args)
}

/// now に発火すべき時刻ルールのアクションをすべて実行する。成功したアクション数を
/// 返す（ルール数ではない）。個々のアクション失敗はループを止めず warn ログに残す
/// （常駐の頑健性）。
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

/// 1 バッチの通知に一致するイベントトリガのルールを返す（rules.toml 記載順）。
/// 同じルールが複数通知に一致しても 1 回だけ含む。
fn due_event_rules<'a>(
    file: &'a RuleFile,
    config: &Config,
    events: &[enl::Event],
    now: NaiveTime,
) -> Vec<&'a Rule> {
    file.rules
        .iter()
        .filter(|r| matches!(r.when, Trigger::Event { .. }))
        .filter(|r| events.iter().any(|e| event_matches(r, config, e)))
        .filter(|r| active_or_log(r, now))
        .collect()
}

/// 1 バッチの通知に対し、一致するイベントトリガのアクションを同期・直列に発火する
/// （`--listen-once` 用）。成功したアクション数を返す（ルール数ではない）。
pub fn fire_due_events(
    file: &RuleFile,
    config: &Config,
    events: &[enl::Event],
    now: NaiveTime,
    config_path: Option<&Path>,
) -> usize {
    fire_all(due_event_rules(file, config, events, now), config_path)
}

/// `enl listen` を 1 回起動し、得た通知でイベントトリガを発火する。成功した
/// アクション数を返す（ルール数ではない）。
pub fn drain_events_once(
    file: &RuleFile,
    config: &Config,
    enl_bin: &str,
    now: Option<NaiveTime>,
    config_path: Option<&Path>,
) -> Result<usize, CasaError> {
    let events = enl::listen_once(enl_bin)?;
    // 窓判定は listen が返った後の時刻で行う（listen は何時間もブロックしうる）。
    // --now が与えられていればそれを優先する（デバッグ用の固定時刻）。
    Ok(fire_due_events(
        file,
        config,
        &events,
        now_or(now),
        config_path,
    ))
}

/// Matter イベントトリガのルールが、与えられた mat listen イベント 1 件に一致するか。
/// priming（matd 再購読時の現在値再配達）は状変ではないので無条件で不一致。
pub fn matter_event_matches(rule: &Rule, config: &Config, event: &mat::Event) -> bool {
    let Trigger::MatterEvent {
        device,
        attribute,
        equals,
    } = &rule.when
    else {
        return false;
    };
    if event.priming {
        return false;
    }
    let (node_id, endpoint) = match config.device(device) {
        Ok(Device::Matter {
            node_id: Some(node_id),
            endpoint,
            ..
        }) => (node_id, endpoint),
        _ => return false,
    };
    if parse_node_id(node_id) != Some(event.node_id) {
        return false;
    }
    if let Some(ep) = endpoint {
        if u64::from(*ep) != event.endpoint {
            return false;
        }
    }
    attribute_matches(&event.attribute, attribute) && event.value == *equals
}

/// イベントの attribute（chip-tool 名 or 未知 ID の数値）とルールの属性名の突合。
/// 名前は case-insensitive、数値はルール側の 10 進表記と比較する。
fn attribute_matches(event_attr: &serde_json::Value, rule_attr: &str) -> bool {
    match event_attr {
        serde_json::Value::String(s) => s.eq_ignore_ascii_case(rule_attr),
        serde_json::Value::Number(n) => rule_attr.trim().parse::<u64>().ok() == n.as_u64(),
        _ => false,
    }
}

/// 1 バッチの mat イベントに一致する Matter イベントトリガのルールを返す
/// （rules.toml 記載順・重複なし）。
fn due_matter_event_rules<'a>(
    file: &'a RuleFile,
    config: &Config,
    events: &[mat::Event],
    now: NaiveTime,
) -> Vec<&'a Rule> {
    file.rules
        .iter()
        .filter(|r| matches!(r.when, Trigger::MatterEvent { .. }))
        .filter(|r| events.iter().any(|e| matter_event_matches(r, config, e)))
        .filter(|r| active_or_log(r, now))
        .collect()
}

/// `mat listen` を 1 回起動し、得たイベントで Matter トリガを発火する。成功した
/// アクション数を返す（ルール数ではない）。
pub fn drain_matter_events_once(
    file: &RuleFile,
    config: &Config,
    mat_bin: &str,
    now: Option<NaiveTime>,
    config_path: Option<&Path>,
) -> Result<usize, CasaError> {
    let events = mat::listen_once(mat_bin)?;
    // 窓判定は listen が返った後の時刻で行う（listen は何時間もブロックしうる）。
    // --now が与えられていればそれを優先する（デバッグ用の固定時刻）。
    Ok(fire_all(
        due_matter_event_rules(file, config, &events, now_or(now)),
        config_path,
    ))
}

/// 1 アクションを実行し、成功（casa が exit 0）なら true。失敗は warn ログに残す。
/// 同期経路（`fire_all`）と非同期ワーカー（dispatcher）の両方がこれを使う。
fn run_one(job: Job<'_>, config_path: Option<&Path>) -> bool {
    match fire(job, config_path) {
        Ok(0) => true,
        Ok(code) => {
            tracing::warn!(
                rule = %job.rule.name,
                device = job.then.device(),
                action = job.then.action_name(),
                code,
                "rule action exited nonzero"
            );
            false
        }
        Err(e) => {
            tracing::warn!(
                rule = %job.rule.name,
                device = job.then.device(),
                action = job.then.action_name(),
                error = %e,
                "rule action failed"
            );
            false
        }
    }
}

/// ルール群を (ルール, アクション) の仕事列へ**記載順**に展開する。
fn jobs<'a>(rules: &[&'a Rule]) -> Vec<Job<'a>> {
    rules
        .iter()
        .copied()
        .flat_map(|rule| {
            rule.then
                .actions()
                .iter()
                .map(move |then| Job { rule, then })
        })
        .collect()
}

/// 仕事列を同期・直列にすべて実行する。成功した件数を返す。
/// 失敗はループを止めない（常駐の頑健性）。`run` は本番では [`run_one`]、
/// テストでは記録クロージャを渡す。
fn fire_jobs<'a, F: Fn(Job<'a>) -> bool>(jobs: Vec<Job<'a>>, run: F) -> usize {
    jobs.into_iter().filter(|job| run(*job)).count()
}

/// 与えられたルール群のアクションを同期・直列にすべて発火する
/// （`--once` / `--listen-once` / `--listen-once-mat` 用）。
/// 成功した**アクション**数を返す（ルール数ではない）。
fn fire_all(rules: Vec<&Rule>, config_path: Option<&Path>) -> usize {
    fire_jobs(jobs(&rules), |job| run_one(job, config_path))
}

/// `--now` の上書きを解決する。None なら実時計（ローカルタイム）。
fn now_or(override_now: Option<NaiveTime>) -> NaiveTime {
    override_now.unwrap_or_else(|| Local::now().time())
}

/// ルールエンジンを走らせる。`--once` は時刻 1 tick で終了、常駐は時刻スケジューラ
/// （毎分 tick）と イベントリスナ（enl listen / mat listen ループ）を並行に回す。
pub fn run(
    file: &RuleFile,
    config: &Config,
    config_path: Option<&Path>,
    enl_bin: &str,
    mat_bin: &str,
    opts: RunOpts,
) -> Result<i32, CasaError> {
    if opts.once {
        let now = now_or(opts.now);
        let fired = tick(file, now, config_path);
        tracing::info!(fired, ?now, "single tick complete");
        return Ok(0);
    }

    tracing::info!("casad resident engine started (time + event)");
    let has_enl_events = file
        .rules
        .iter()
        .any(|r| matches!(r.when, Trigger::Event { .. }));
    let has_matter_events = file
        .rules
        .iter()
        .any(|r| matches!(r.when, Trigger::MatterEvent { .. }));

    // scope で借用を渡し、Arc/clone なしにループ群 + ワーカー群を並行させる。
    // アクション実行はデバイス別ワーカーに非同期投入する（同一デバイス FIFO・
    // 異デバイス並列）。listen / tick ループはアクション完了を待たない。
    std::thread::scope(|s| {
        let dispatcher = Dispatcher::new(s, distinct_devices(file), move |job: Job| {
            run_one(job, config_path);
        });
        if has_enl_events {
            let d = dispatcher.clone();
            s.spawn(move || event_loop(file, config, enl_bin, &d));
        }
        if has_matter_events {
            let d = dispatcher.clone();
            s.spawn(move || matter_event_loop(file, config, mat_bin, &d));
        }
        time_loop(file, &dispatcher);
    });
    Ok(0) // time_loop は戻らないので到達しない。
}

/// 時刻スケジューラ。毎分の境界で該当ルールをワーカーに積む。
fn time_loop<'env>(file: &'env RuleFile, dispatcher: &Dispatcher<'env>) -> ! {
    loop {
        let now = Local::now().time();
        let queued = dispatcher.dispatch_all(due_time_rules(file, now));
        if queued > 0 {
            tracing::debug!(queued, "time rule actions queued");
        }
        sleep_to_next_minute();
    }
}

/// イベントリスナ。`enl listen` を回し続け、一致ルールをワーカーに積んで即再 listen する。
/// enl 起動失敗・異常終了はバックオフして再試行（常駐の頑健性）。
fn event_loop<'env>(
    file: &'env RuleFile,
    config: &Config,
    enl_bin: &str,
    dispatcher: &Dispatcher<'env>,
) -> ! {
    loop {
        match enl::listen_once(enl_bin) {
            Ok(events) => {
                // 窓判定は listen が返った後の時刻で行う（listen は何時間もブロックしうる）。
                let now = Local::now().time();
                let queued = dispatcher.dispatch_all(due_event_rules(file, config, &events, now));
                if queued > 0 {
                    tracing::debug!(queued, "event rule actions queued");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "enl listen failed; backing off");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

/// Matter イベントリスナ。`mat listen` を回し続け、一致ルールをワーカーに積んで
/// 即再 listen する。mat 起動失敗・matd 不在（exit 13）はバックオフして再試行。
fn matter_event_loop<'env>(
    file: &'env RuleFile,
    config: &Config,
    mat_bin: &str,
    dispatcher: &Dispatcher<'env>,
) -> ! {
    loop {
        match mat::listen_once(mat_bin) {
            Ok(events) => {
                // 同上。listen が返った時刻で窓を判定する。
                let now = Local::now().time();
                let queued =
                    dispatcher.dispatch_all(due_matter_event_rules(file, config, &events, now));
                if queued > 0 {
                    tracing::debug!(queued, "matter event rule actions queued");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "mat listen failed; backing off");
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
    use crate::rules::Then;

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

    fn config_matter() -> Config {
        casa_core::config::parse(
            r#"
version = 1
[devices.study_motion]
protocol = "matter"
node_id = "16"
[devices.desk_tape_light]
protocol = "matter"
node_id = "6"
[devices.outlet2]
protocol = "matter"
node_id = "5678"
endpoint = 2
"#,
        )
        .unwrap()
    }

    fn mat_event(
        node_id: u64,
        endpoint: u64,
        attribute: &str,
        value: serde_json::Value,
    ) -> mat::Event {
        mat::Event {
            node_id,
            endpoint,
            cluster: serde_json::json!("occupancysensing"),
            attribute: serde_json::json!(attribute),
            value,
            priming: false,
        }
    }

    const MATTER_RULE: &str = r#"
version = 1
[[rules]]
name = "人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#;

    #[test]
    fn matter_event_matches_on_node_attribute_value() {
        let file = rules(MATTER_RULE);
        let cfg = config_matter();
        let ev = mat_event(16, 1, "occupancy", serde_json::json!(0));
        assert!(matter_event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn matter_event_attribute_is_case_insensitive() {
        let file = rules(MATTER_RULE);
        let cfg = config_matter();
        let ev = mat_event(16, 1, "Occupancy", serde_json::json!(0));
        assert!(matter_event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn matter_event_does_not_match_on_mismatch() {
        let file = rules(MATTER_RULE);
        let cfg = config_matter();
        // node_id 違い。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(6, 1, "occupancy", serde_json::json!(0))
        ));
        // attribute 違い。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(16, 1, "onoff", serde_json::json!(0))
        ));
        // 値違い（在室 ON）。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(16, 1, "occupancy", serde_json::json!(1))
        ));
        // 型違い（数値 0 vs 文字列 "0"）。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(16, 1, "occupancy", serde_json::json!("0"))
        ));
    }

    #[test]
    fn matter_event_priming_never_matches() {
        let file = rules(MATTER_RULE);
        let cfg = config_matter();
        let mut ev = mat_event(16, 1, "occupancy", serde_json::json!(0));
        ev.priming = true;
        // matd 再購読時の現在値再配達で発火してはならない。
        assert!(!matter_event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn matter_event_endpoint_filter_applies_only_when_configured() {
        let cfg = config_matter();
        // endpoint = 2 を持つ outlet2 のルール: endpoint 一致のみマッチ。
        let file = rules(
            r#"
version = 1
[[rules]]
name = "outlet2"
when = { device = "outlet2", attribute = "onoff", equals = true }
then = { action = "off", device = "desk_tape_light" }
"#,
        );
        assert!(matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(5678, 2, "onoff", serde_json::json!(true))
        ));
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(5678, 1, "onoff", serde_json::json!(true))
        ));
        // study_motion は endpoint 未指定なのでどの endpoint でもマッチ。
        let file = rules(MATTER_RULE);
        assert!(matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(16, 3, "occupancy", serde_json::json!(0))
        ));
    }

    #[test]
    fn matter_numeric_attribute_matches_numeric_rule() {
        // matd は ids テーブルに無い属性を数値のまま流す。ルール側も数値文字列で書けば突合できる。
        let cfg = config_matter();
        let file = rules(
            r#"
version = 1
[[rules]]
name = "数値属性"
when = { device = "study_motion", attribute = "0", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        );
        let mut ev = mat_event(16, 1, "occupancy", serde_json::json!(0));
        ev.attribute = serde_json::json!(0);
        assert!(matter_event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn due_matter_event_rules_ignores_echonet_and_time_rules() {
        let cfg = config_matter();
        let file = rules(
            r#"
version = 1
[[rules]]
name = "時刻"
when = { at = "22:00" }
then = { action = "off", device = "desk_tape_light" }
[[rules]]
name = "人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        );
        let ev = mat_event(16, 1, "occupancy", serde_json::json!(0));
        let due = due_matter_event_rules(&file, &cfg, &[ev], at(12, 0));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "人感OFFで消灯");
    }

    const MULTI: &str = r#"
version = 1
[[rules]]
name = "まとめて点灯"
when = { at = "07:00" }
then = [
  { action = "on", device = "living_aircon" },
  { action = "invoke", device = "living_aircon", command = "color-temp", args = ["--kelvin", "2700"] },
  { action = "on", device = "entry_lock" },
]
"#;

    #[test]
    fn jobs_expands_actions_in_declaration_order() {
        let file = rules(MULTI);
        let due: Vec<&Rule> = file.rules.iter().collect();
        let jobs = jobs(&due);
        assert_eq!(jobs.len(), 3);
        assert_eq!(jobs[0].then.device(), "living_aircon");
        assert!(matches!(jobs[0].then, Then::On { .. }));
        assert!(matches!(jobs[1].then, Then::Invoke { .. }));
        assert_eq!(jobs[2].then.device(), "entry_lock");
        // ルール名は全 Job が持ち回る（発火ログ用）。
        assert!(jobs.iter().all(|j| j.rule.name == "まとめて点灯"));
    }

    #[test]
    fn fire_jobs_continues_after_a_failing_action() {
        let file = rules(MULTI);
        let due: Vec<&Rule> = file.rules.iter().collect();
        let attempted = std::sync::Mutex::new(Vec::new());
        let ok = fire_jobs(jobs(&due), |job| {
            attempted
                .lock()
                .unwrap()
                .push(job.then.device().to_string());
            // 2 番目（invoke）だけ失敗させる。
            !matches!(job.then, Then::Invoke { .. })
        });
        // 失敗しても後続は実行される。
        assert_eq!(
            *attempted.lock().unwrap(),
            ["living_aircon", "living_aircon", "entry_lock"]
        );
        // 戻り値は成功したアクション数。
        assert_eq!(ok, 2);
    }

    fn rule_with_window(from: &str, to: &str) -> RuleFile {
        rules(&format!(
            r#"
version = 1
[[rules]]
name = "窓つき"
when = {{ at = "12:00" }}
active = {{ from = "{from}", to = "{to}" }}
then = {{ action = "on", device = "living_aircon" }}
"#
        ))
    }

    #[test]
    fn active_window_includes_from_and_excludes_to() {
        let file = rule_with_window("06:00", "21:00");
        let r = &file.rules[0];
        assert!(rule_is_active(r, at(6, 0)), "from 境界は含む");
        assert!(rule_is_active(r, at(12, 34)));
        assert!(rule_is_active(r, at(20, 59)));
        assert!(!rule_is_active(r, at(21, 0)), "to 境界は含まない");
        assert!(!rule_is_active(r, at(5, 59)));
        assert!(!rule_is_active(r, at(23, 0)));
    }

    #[test]
    fn active_window_wraps_over_midnight() {
        let file = rule_with_window("21:00", "06:00");
        let r = &file.rules[0];
        assert!(rule_is_active(r, at(21, 0)));
        assert!(rule_is_active(r, at(23, 59)));
        assert!(rule_is_active(r, at(0, 0)));
        assert!(rule_is_active(r, at(5, 59)));
        assert!(!rule_is_active(r, at(6, 0)));
        assert!(!rule_is_active(r, at(12, 0)));
    }

    #[test]
    fn rule_without_active_window_is_always_active() {
        let file = rules(SCHEDULE);
        for r in &file.rules {
            assert!(rule_is_active(r, at(0, 0)));
            assert!(rule_is_active(r, at(13, 37)));
            assert!(rule_is_active(r, at(23, 59)));
        }
    }

    #[test]
    fn validate_schedule_rejects_malformed_active_window() {
        let file = rule_with_window("6am", "21:00");
        let err = validate_schedule(&file).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("窓つき"), "detail: {}", err.detail);
    }

    #[test]
    fn validate_schedule_rejects_zero_width_active_window() {
        // from == to は「空区間」とも「全日」とも読めるので弾く。
        let file = rule_with_window("06:00", "06:00");
        let err = validate_schedule(&file).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("窓つき"), "detail: {}", err.detail);
    }

    fn rule_with_at_and_window(at: &str, from: &str, to: &str) -> RuleFile {
        rules(&format!(
            r#"
version = 1
[[rules]]
name = "窓外トリガ"
when = {{ at = "{at}" }}
active = {{ from = "{from}", to = "{to}" }}
then = {{ action = "off", device = "living_aircon" }}
"#
        ))
    }

    #[test]
    fn validate_schedule_rejects_at_equal_to_window_to() {
        // to は排他境界。at == to は「窓の外」＝この時刻トリガは絶対に発火しない。
        let file = rule_with_at_and_window("21:00", "06:00", "21:00");
        let err = validate_schedule(&file).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("窓外トリガ"), "detail: {}", err.detail);
        assert!(err.detail.contains("21:00"), "detail: {}", err.detail);
        assert!(err.detail.contains("06:00"), "detail: {}", err.detail);
    }

    #[test]
    fn validate_schedule_accepts_at_equal_to_window_from() {
        // from は包含境界。at == from は窓の中なので有効。
        let file = rule_with_at_and_window("06:00", "06:00", "21:00");
        assert!(validate_schedule(&file).is_ok());
    }

    #[test]
    fn validate_schedule_accepts_at_inside_wrapping_window() {
        assert!(validate_schedule(&rule_with_at_and_window("23:00", "21:00", "06:00")).is_ok());
        assert!(validate_schedule(&rule_with_at_and_window("03:00", "21:00", "06:00")).is_ok());
    }

    #[test]
    fn validate_schedule_rejects_at_outside_wrapping_window() {
        let file = rule_with_at_and_window("12:00", "21:00", "06:00");
        let err = validate_schedule(&file).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("窓外トリガ"), "detail: {}", err.detail);
    }

    #[test]
    fn validate_schedule_allows_event_trigger_with_active_window() {
        // イベントトリガは at を持たないので、この静的な at-vs-window チェックの対象外。
        let file = rules(
            r#"
version = 1
[[rules]]
name = "窓つきイベント"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
active = { from = "06:00", to = "21:00" }
then = { action = "on", device = "living_aircon" }
"#,
        );
        assert!(validate_schedule(&file).is_ok());
    }

    #[test]
    fn due_time_rules_respects_active_window() {
        let file = rules(
            r#"
version = 1
[[rules]]
name = "窓外の時刻トリガ"
when = { at = "22:00" }
active = { from = "06:00", to = "21:00" }
then = { action = "off", device = "living_aircon" }
[[rules]]
name = "窓内の時刻トリガ"
when = { at = "12:00" }
active = { from = "06:00", to = "21:00" }
then = { action = "off", device = "living_aircon" }
"#,
        );
        // 22:00 は窓外なので、時刻が一致しても発火対象にならない。
        assert!(due_time_rules(&file, at(22, 0)).is_empty());
        let due = due_time_rules(&file, at(12, 0));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "窓内の時刻トリガ");
    }

    #[test]
    fn due_event_rules_respects_active_window() {
        let file = rules(
            r#"
version = 1
[[rules]]
name = "昼だけ電源ONで点灯"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
active = { from = "06:00", to = "21:00" }
then = { action = "on", device = "living_aircon" }
"#,
        );
        let cfg = config_living();
        let inside = due_event_rules(
            &file,
            &cfg,
            &[event("192.0.2.10", "013001", "80", "30")],
            at(12, 0),
        );
        assert_eq!(inside.len(), 1);
        let outside = due_event_rules(
            &file,
            &cfg,
            &[event("192.0.2.10", "013001", "80", "30")],
            at(3, 0),
        );
        assert!(outside.is_empty());
    }

    #[test]
    fn due_matter_event_rules_respects_active_window() {
        let file = rules(
            r#"
version = 1
[[rules]]
name = "書斎 不在で扇風機ON"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
active = { from = "06:00", to = "21:00" }
then = { action = "on", device = "desk_tape_light" }
"#,
        );
        let cfg = config_matter();
        let inside = due_matter_event_rules(
            &file,
            &cfg,
            &[mat_event(16, 1, "occupancy", serde_json::json!(0))],
            at(12, 0),
        );
        assert_eq!(inside.len(), 1);
        let outside = due_matter_event_rules(
            &file,
            &cfg,
            &[mat_event(16, 1, "occupancy", serde_json::json!(0))],
            at(21, 30),
        );
        assert!(outside.is_empty());
    }

    #[test]
    fn windowless_rules_still_fire_at_any_time() {
        // 後方互換: active を持たないルールは従来どおりどの時刻でも発火する。
        let file = rules(SCHEDULE);
        assert_eq!(due_time_rules(&file, at(22, 0)).len(), 1);
        let cfg = config_living();
        assert_eq!(
            due_event_rules(
                &file,
                &cfg,
                &[event("192.0.2.10", "013001", "80", "30")],
                at(3, 0)
            )
            .len(),
            1
        );
        // Matter イベントトリガも同様（3経路すべてを後方互換で覆う）。
        let matter_file = rules(MATTER_RULE);
        let matter_cfg = config_matter();
        assert_eq!(
            due_matter_event_rules(
                &matter_file,
                &matter_cfg,
                &[mat_event(16, 1, "occupancy", serde_json::json!(0))],
                at(3, 0)
            )
            .len(),
            1
        );
    }
}
