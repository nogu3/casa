//! ルールファイル（rules.toml）の型・パース・検証。
//!
//! 書き手は LLM / UI を想定し、人間の手書きは前提にしない（ただし直接覗ける可読性は
//! 確保する）。形式は devices.toml と揃えて TOML。デバイス参照は casa-core の Config で
//! 読込時に検証し、発火前に不正なルールを弾く（ハイブリッド構成の link 側の価値）。
//!
//! 中身は `when`（トリガ）→ `then`（アクション）の素朴な対応。複数条件・遅延などの
//! 表現力拡張は後段で必要になったら足す。

use std::path::Path;

use serde::{Deserialize, Serialize};

use casa_core::config::Config;
use casa_core::error::{CasaError, ErrorKind};

/// casad が理解するルールファイルのバージョン。
pub const SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
pub struct RuleFile {
    pub version: u32,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// エンジン全体の動作設定（`[settings]` テーブル、省略可）。
/// 単位は秒の整数（humantime 形式のための依存は増やさない）。
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// イベント由来の off を保留する猶予秒。保留中に同一デバイスへ on が来たら
    /// off は破棄される（スマートプラグの off→on 連射固着の緩和。issue #5）。
    /// 時刻トリガの off には適用しない（取り消したいケースが無く、遅れるだけ）。
    pub off_grace_secs: u64,
    /// 同一デバイスへの連続コマンド送信の最小間隔秒。
    pub min_gap_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            off_grace_secs: 30,
            min_gap_secs: 2,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Rule {
    pub name: String,
    pub when: Trigger,
    /// ルールが有効な時間帯。未指定なら常時有効。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActiveWindow>,
    pub then: Thens,
}

/// ルールの有効な時間帯。`from` を含み `to` を含まない半開区間 [from, to)。
/// `from > to` は日跨ぎ（例 21:00-06:00 = 21:00〜23:59 と 00:00〜05:59）。
///
/// HH:MM の書式検証は engine 側（`validate_schedule`）が担う。`Trigger::Time { at }` を
/// String のまま持ち engine で検証しているのと責務の置き場所を揃える。
#[derive(Debug, Deserialize, Serialize)]
pub struct ActiveWindow {
    pub from: String,
    pub to: String,
}

/// 1 ルールのアクション列。TOML では単一テーブルと配列の両方を受ける。
/// - `then = { action = "on", device = "a" }`
/// - `then = [{ action = "on", device = "a" }, { action = "on", device = "b" }]`
///
/// 呼び出し側は本 enum を直接 match せず、必ず [`Thens::actions`] を経由する
/// （単一形 / 配列形の非対称を呼び出し側に漏らさないため）。
///
/// `Deserialize` は手書き（下記 `impl`）。`#[serde(untagged)]` derive のままだと
/// 中身（`Then`）が variant に一致しない場合のエラーが
/// 「data did not match any variant of untagged enum Thens」に潰れ、内側の
/// `unknown variant` / `missing field` を握り潰してしまう。rules.toml の書き手は
/// LLM / UI（本ファイル冒頭コメント）で、内側のエラーが直す手がかりそのものなので、
/// TOML の形（テーブル/配列）で先に分岐し、内側のエラーをそのまま伝播させる。
/// `Serialize` は形（単一 = オブジェクト、複数 = 配列）をそのまま出したいので
/// `#[serde(untagged)]` を残す（`casad check` の JSON 出力の後方互換）。
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Thens {
    One(Then),
    Many(Vec<Then>),
}

impl<'de> Deserialize<'de> for Thens {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Thens;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an action table or an array of action tables")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, m: A) -> Result<Thens, A::Error> {
                Then::deserialize(serde::de::value::MapAccessDeserializer::new(m)).map(Thens::One)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, s: A) -> Result<Thens, A::Error> {
                Vec::<Then>::deserialize(serde::de::value::SeqAccessDeserializer::new(s))
                    .map(Thens::Many)
            }
        }
        d.deserialize_any(V)
    }
}

impl Thens {
    /// 記載順のアクション列。単一形は 1 要素のスライスとして見せる。
    pub fn actions(&self) -> &[Then] {
        match self {
            Thens::One(t) => std::slice::from_ref(t),
            Thens::Many(v) => v,
        }
    }
}

/// トリガ。TOML ではインラインテーブルで、含まれるキーで種別が決まる（untagged）。
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Trigger {
    /// イベント: あるデバイスの EPC が指定値になったとき。
    /// 例: `when = { device = "entry_motion", epc = "0x80", equals = "0x30" }`
    Event {
        device: String,
        epc: String,
        equals: String,
    },
    /// Matter イベント: あるデバイスの属性が指定値になったとき（mat listen 経由）。
    /// 例: `when = { device = "study_motion", attribute = "occupancy", equals = 0 }`
    /// equals は matd イベントの JSON `value` と等値比較する（数値 / bool / 文字列）。
    MatterEvent {
        device: String,
        attribute: String,
        equals: serde_json::Value,
    },
    /// 時刻: 毎日その時刻（HH:MM）になったとき。
    /// 例: `when = { at = "22:00" }`
    Time { at: String },
}

/// アクション。`action` フィールドが serde の tag。
/// - `then = { action = "on", device = "hallway_light" }`
/// - `then = { action = "invoke", device = "desk_light", command = "color-temp", args = ["--kelvin", "2700"] }`
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Then {
    On {
        device: String,
    },
    Off {
        device: String,
    },
    Invoke {
        device: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl Then {
    /// アクション対象の名前（device または group。名前解決は casa 側が担う）。
    pub fn device(&self) -> &str {
        match self {
            Then::On { device } | Then::Off { device } | Then::Invoke { device, .. } => device,
        }
    }

    /// ログ用のアクション種別名。TOML の `action` の値と一致させる。
    pub fn action_name(&self) -> &'static str {
        match self {
            Then::On { .. } => "on",
            Then::Off { .. } => "off",
            Then::Invoke { .. } => "invoke",
        }
    }

    /// casa の引数列へ変換する。casa の CLI 表面に対する casad の知識はここに閉じる。
    /// `--config` は casa の clap グローバルフラグとしてサブコマンドより**前**に置く
    /// （invoke の trailing 引数に呑まれないため。on/off も並びを揃える）。
    pub fn casa_args(&self, config: Option<&Path>) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(path) = config {
            out.push("--config".to_string());
            out.push(path.to_string_lossy().into_owned());
        }
        match self {
            Then::On { device } => out.extend(["on".to_string(), device.clone()]),
            Then::Off { device } => out.extend(["off".to_string(), device.clone()]),
            Then::Invoke {
                device,
                command,
                args,
            } => {
                out.extend(["invoke".to_string(), device.clone(), command.clone()]);
                out.extend(args.iter().cloned());
            }
        }
        out
    }
}

/// TOML 文字列をパースし、バージョンを検証する。
pub fn parse(text: &str) -> Result<RuleFile, CasaError> {
    let file: RuleFile = toml::from_str(text).map_err(|e| {
        CasaError::new(
            ErrorKind::ConfigParse,
            format!("failed to parse rules: {e}"),
        )
    })?;

    if file.version != SUPPORTED_VERSION {
        return Err(CasaError::new(
            ErrorKind::ConfigParse,
            format!(
                "unsupported rules version {} (expected {SUPPORTED_VERSION})",
                file.version
            ),
        ));
    }

    for rule in &file.rules {
        if rule.then.actions().is_empty() {
            return Err(CasaError::new(
                ErrorKind::ConfigParse,
                format!("rule \"{}\": then is empty", rule.name),
            ));
        }
    }

    Ok(file)
}

/// ルールファイルを読み込み、パース・バージョン検証する。
pub fn load(path: &Path) -> Result<RuleFile, CasaError> {
    tracing::debug!(path = %path.display(), "loading rules");

    let text = std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CasaError::new(
                ErrorKind::ConfigMissing,
                format!("rules file not found: {}", path.display()),
            )
        } else {
            CasaError::new(
                ErrorKind::ConfigParse,
                format!("failed to read rules file {}: {e}", path.display()),
            )
        }
    })?;

    parse(&text).map_err(|mut e| {
        e.detail = format!("{}: {}", path.display(), e.detail);
        e
    })
}

/// devices.toml の `node_id`（文字列）を数値へ。mat listen イベントの
/// node_id（数値）との突合に使う。10 進のみ（mat の alias は不可）。
pub(crate) fn parse_node_id(s: &str) -> Option<u64> {
    s.trim().parse().ok()
}

impl RuleFile {
    /// 参照する名前がすべて config に存在するか検証する（発火前に弾く）。
    /// 未定義名は `name_not_found`（exit 11）。エラーにはルール名を添える。
    ///
    /// `when.device`（イベントトリガの発火元）は `enl listen` の通知と ip/eoj で
    /// 突き合わせる実デバイスが必要なため、グループは許可しない（`check_device`）。
    /// `then.device`（アクション対象）は casa の `on`/`off` に渡るだけなので、
    /// グループ名も許可する（`check_target`。名前解決は casa 側が担う）。
    pub fn validate(&self, config: &Config) -> Result<(), CasaError> {
        for rule in &self.rules {
            match &rule.when {
                Trigger::Event { device, .. } => {
                    check_device(config, &rule.name, device)?;
                }
                Trigger::MatterEvent { device, .. } => {
                    check_matter_device(config, &rule.name, device)?;
                }
                Trigger::Time { .. } => {}
            }
            for then in rule.then.actions() {
                check_target(config, &rule.name, then.device())?;
            }
        }
        Ok(())
    }
}

fn check_device(config: &Config, rule_name: &str, device: &str) -> Result<(), CasaError> {
    config
        .device(device)
        .map(|_| ())
        .map_err(|e| CasaError::new(e.kind, format!("rule \"{rule_name}\": {}", e.detail)))
}

/// Matter イベントトリガの発火元検証: 存在 + protocol=matter + node_id が数値。
/// mat listen の通知と node_id で突き合わせるため、数値化できない設定は起動前に弾く。
fn check_matter_device(config: &Config, rule_name: &str, device: &str) -> Result<(), CasaError> {
    let dev = config
        .device(device)
        .map_err(|e| CasaError::new(e.kind, format!("rule \"{rule_name}\": {}", e.detail)))?;
    match dev {
        casa_core::config::Device::Matter {
            node_id: Some(node_id),
            ..
        } => {
            if parse_node_id(node_id).is_none() {
                return Err(CasaError::new(
                    ErrorKind::ConfigParse,
                    format!(
                        "rule \"{rule_name}\": device \"{device}\" node_id \"{node_id}\" is not numeric"
                    ),
                ));
            }
            Ok(())
        }
        casa_core::config::Device::Matter { node_id: None, .. } => Err(CasaError::new(
            ErrorKind::ConfigParse,
            format!(
                "rule \"{rule_name}\": device \"{device}\" is a matter group (groupcast) with no node_id; a matter event trigger requires a node_id device"
            ),
        )),
        other => Err(CasaError::new(
            ErrorKind::ProtocolUnsupported,
            format!(
                "rule \"{rule_name}\": matter event trigger requires a matter device, but \"{device}\" is {}",
                other.protocol()
            ),
        )),
    }
}

fn check_target(config: &Config, rule_name: &str, name: &str) -> Result<(), CasaError> {
    config
        .ensure_target(name)
        .map_err(|e| CasaError::new(e.kind, format!("rule \"{rule_name}\": {}", e.detail)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
version = 1

[[rules]]
name = "帰宅で廊下灯"
when = { device = "entry_motion", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "hallway_light" }

[[rules]]
name = "就寝時刻で消灯"
when = { at = "22:00" }
then = { action = "off", device = "hallway_light" }
"#;

    fn config_with_devices() -> Config {
        casa_core::config::parse(
            r#"
version = 1
[devices.entry_motion]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x000701"
[devices.hallway_light]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029001"
"#,
        )
        .unwrap()
    }

    #[test]
    fn settings_default_when_absent() {
        let file = parse(VALID).unwrap();
        assert_eq!(file.settings.off_grace_secs, 30);
        assert_eq!(file.settings.min_gap_secs, 2);
    }

    #[test]
    fn parses_explicit_settings() {
        let file = parse(
            r#"
version = 1
[settings]
off_grace_secs = 10
min_gap_secs = 0
"#,
        )
        .unwrap();
        assert_eq!(file.settings.off_grace_secs, 10);
        assert_eq!(file.settings.min_gap_secs, 0);
    }

    #[test]
    fn partial_settings_fill_remaining_defaults() {
        let file = parse("version = 1\n[settings]\noff_grace_secs = 5\n").unwrap();
        assert_eq!(file.settings.off_grace_secs, 5);
        assert_eq!(file.settings.min_gap_secs, 2);
    }

    #[test]
    fn parses_event_and_time_rules() {
        let file = parse(VALID).unwrap();
        assert_eq!(file.rules.len(), 2);
        assert!(matches!(file.rules[0].when, Trigger::Event { .. }));
        assert!(matches!(file.rules[1].when, Trigger::Time { .. }));
        assert!(matches!(file.rules[0].then.actions()[0], Then::On { .. }));
    }

    #[test]
    fn parses_invoke_rule_with_args() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "日没で電球色"
when = { at = "18:00" }
then = { action = "invoke", device = "hallway_light", command = "color-temp", args = ["--kelvin", "2700"] }
"#,
        )
        .unwrap();
        match &file.rules[0].then.actions()[0] {
            Then::Invoke {
                device,
                command,
                args,
            } => {
                assert_eq!(device, "hallway_light");
                assert_eq!(command, "color-temp");
                assert_eq!(args, &["--kelvin", "2700"]);
            }
            other => panic!("unexpected then: {other:?}"),
        }
    }

    #[test]
    fn invoke_rule_args_default_to_empty() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "argsなし"
when = { at = "18:00" }
then = { action = "invoke", device = "hallway_light", command = "blink" }
"#,
        )
        .unwrap();
        match &file.rules[0].then.actions()[0] {
            Then::Invoke { args, .. } => assert!(args.is_empty()),
            other => panic!("unexpected then: {other:?}"),
        }
    }

    #[test]
    fn casa_args_places_config_before_subcommand() {
        use std::path::Path;
        let then = Then::Invoke {
            device: "hallway_light".into(),
            command: "color-temp".into(),
            args: vec!["--kelvin".into(), "2700".into()],
        };
        assert_eq!(
            then.casa_args(Some(Path::new("/tmp/d.toml"))),
            [
                "--config",
                "/tmp/d.toml",
                "invoke",
                "hallway_light",
                "color-temp",
                "--kelvin",
                "2700"
            ]
        );
        // on/off も --config が先頭（casa の clap グローバルフラグ）。
        let on = Then::On {
            device: "hallway_light".into(),
        };
        assert_eq!(
            on.casa_args(Some(Path::new("/tmp/d.toml"))),
            ["--config", "/tmp/d.toml", "on", "hallway_light"]
        );
        assert_eq!(on.casa_args(None), ["on", "hallway_light"]);
    }

    #[test]
    fn rejects_unsupported_version() {
        let err = parse("version = 2").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
    }

    #[test]
    fn rejects_malformed_toml() {
        let err = parse("this is not = = toml").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
    }

    #[test]
    fn rejects_unknown_trigger_shape() {
        // device も at も無いトリガはどの variant にも一致しない。
        let err = parse(
            r#"
version = 1
[[rules]]
name = "壊れたトリガ"
when = { nonsense = "x" }
then = { action = "on", device = "hallway_light" }
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
    }

    #[test]
    fn validate_accepts_existing_devices() {
        let file = parse(VALID).unwrap();
        file.validate(&config_with_devices()).unwrap();
    }

    #[test]
    fn validate_accepts_group_as_then_device() {
        let config = casa_core::config::parse(
            r#"
version = 1
[devices.entry_motion]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x000701"
[devices.hallway_light]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029001"
[devices.hallway_light2]
protocol = "echonet"
ip = "192.0.2.12"
eoj = "0x029001"
[groups.hallway]
members = ["hallway_light", "hallway_light2"]
"#,
        )
        .unwrap();
        let file = parse(
            r#"
version = 1
[[rules]]
name = "帰宅で廊下灯グループ点灯"
when = { device = "entry_motion", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "hallway" }
"#,
        )
        .unwrap();
        file.validate(&config).unwrap();
    }

    #[test]
    fn validate_rejects_unknown_device_reference() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "未定義デバイス参照"
when = { at = "07:00" }
then = { action = "on", device = "ghost_device" }
"#,
        )
        .unwrap();
        let err = file.validate(&config_with_devices()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NameNotFound);
        assert!(err.detail.contains("未定義デバイス参照"));
    }

    #[test]
    fn parses_matter_event_rule() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "書斎 人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        match &file.rules[0].when {
            Trigger::MatterEvent {
                device,
                attribute,
                equals,
            } => {
                assert_eq!(device, "study_motion");
                assert_eq!(attribute, "occupancy");
                assert_eq!(equals, &serde_json::json!(0));
            }
            other => panic!("unexpected trigger: {other:?}"),
        }
    }

    #[test]
    fn matter_equals_accepts_bool_and_string() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "bool"
when = { device = "d", attribute = "onoff", equals = true }
then = { action = "on", device = "d" }
[[rules]]
name = "string"
when = { device = "d", attribute = "mode", equals = "auto" }
then = { action = "on", device = "d" }
"#,
        )
        .unwrap();
        match &file.rules[0].when {
            Trigger::MatterEvent { equals, .. } => assert_eq!(equals, &serde_json::json!(true)),
            other => panic!("unexpected trigger: {other:?}"),
        }
        match &file.rules[1].when {
            Trigger::MatterEvent { equals, .. } => assert_eq!(equals, &serde_json::json!("auto")),
            other => panic!("unexpected trigger: {other:?}"),
        }
    }

    #[test]
    fn echonet_event_rule_still_parses_as_event() {
        // epc キーは従来どおり Event variant に落ちる（MatterEvent に吸われない）。
        let file = parse(VALID).unwrap();
        assert!(matches!(file.rules[0].when, Trigger::Event { .. }));
    }

    #[test]
    fn parse_node_id_accepts_decimal_only() {
        assert_eq!(parse_node_id("16"), Some(16));
        assert_eq!(parse_node_id(" 16 "), Some(16));
        assert_eq!(parse_node_id("0x10"), None);
        assert_eq!(parse_node_id("study_motion"), None);
    }

    fn config_with_matter() -> Config {
        casa_core::config::parse(
            r#"
version = 1
[devices.study_motion]
protocol = "matter"
node_id = "16"
[devices.desk_tape_light]
protocol = "matter"
node_id = "6"
[devices.washstand_light]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"
[devices.alias_node]
protocol = "matter"
node_id = "not_a_number"
"#,
        )
        .unwrap()
    }

    #[test]
    fn validate_accepts_matter_event_rule() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        file.validate(&config_with_matter()).unwrap();
    }

    #[test]
    fn validate_rejects_matter_trigger_on_echonet_device() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "プロトコル不一致"
when = { device = "washstand_light", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        let err = file.validate(&config_with_matter()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
        assert!(err.detail.contains("プロトコル不一致"));
    }

    #[test]
    fn validate_rejects_non_numeric_node_id_for_matter_trigger() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "数値でないnode_id"
when = { device = "alias_node", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        let err = file.validate(&config_with_matter()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("数値でないnode_id"));
    }

    #[test]
    fn parses_then_array_in_declaration_order() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "人感ONでまとめて点灯"
when = { at = "18:00" }
then = [
  { action = "on", device = "hallway_light" },
  { action = "invoke", device = "hallway_light", command = "color-temp", args = ["--kelvin", "2700"] },
  { action = "off", device = "entry_motion" },
]
"#,
        )
        .unwrap();
        let actions = file.rules[0].then.actions();
        assert_eq!(actions.len(), 3);
        assert!(matches!(actions[0], Then::On { .. }));
        assert!(matches!(actions[1], Then::Invoke { .. }));
        assert!(matches!(actions[2], Then::Off { .. }));
        assert_eq!(actions[0].device(), "hallway_light");
        assert_eq!(actions[2].device(), "entry_motion");
    }

    #[test]
    fn single_then_table_yields_one_action() {
        let file = parse(VALID).unwrap();
        let actions = file.rules[0].then.actions();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Then::On { .. }));
    }

    #[test]
    fn single_and_array_forms_can_coexist_in_one_file() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "単一形"
when = { at = "07:00" }
then = { action = "on", device = "hallway_light" }
[[rules]]
name = "配列形"
when = { at = "22:00" }
then = [
  { action = "off", device = "hallway_light" },
  { action = "off", device = "entry_motion" },
]
"#,
        )
        .unwrap();
        assert_eq!(file.rules[0].then.actions().len(), 1);
        assert_eq!(file.rules[1].then.actions().len(), 2);
    }

    #[test]
    fn empty_then_array_is_config_parse_error_naming_the_rule() {
        let err = parse(
            r#"
version = 1
[[rules]]
name = "空のthen"
when = { at = "07:00" }
then = []
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(
            err.detail.contains("空のthen"),
            "detail should name the rule: {}",
            err.detail
        );
    }

    #[test]
    fn undefined_device_anywhere_in_then_array_fails_validation() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "2番目が未定義"
when = { at = "07:00" }
then = [
  { action = "on", device = "hallway_light" },
  { action = "on", device = "no_such_device" },
]
"#,
        )
        .unwrap();
        let err = file.validate(&config_with_devices()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NameNotFound);
        assert!(
            err.detail.contains("2番目が未定義") && err.detail.contains("no_such_device"),
            "detail should name rule and device: {}",
            err.detail
        );
    }

    #[test]
    fn action_name_matches_the_toml_action_value() {
        assert_eq!(Then::On { device: "a".into() }.action_name(), "on");
        assert_eq!(Then::Off { device: "a".into() }.action_name(), "off");
        assert_eq!(
            Then::Invoke {
                device: "a".into(),
                command: "blink".into(),
                args: vec![],
            }
            .action_name(),
            "invoke"
        );
    }

    #[test]
    fn then_round_trips_through_json_preserving_shape() {
        // casad check は RuleFile をそのまま JSON 出力する。単一形はオブジェクト、
        // 配列形は配列のまま出ること（既存出力の互換維持）。形だけでなく中身
        // （action / device の値、配列要素数）も検証する。
        let file = parse(
            r#"
version = 1
[[rules]]
name = "単一形"
when = { at = "07:00" }
then = { action = "on", device = "hallway_light" }
[[rules]]
name = "配列形"
when = { at = "22:00" }
then = [
  { action = "off", device = "hallway_light" },
  { action = "on", device = "entry_motion" },
]
"#,
        )
        .unwrap();
        let json = serde_json::to_value(&file).unwrap();
        let single = &json["rules"][0]["then"];
        assert!(single.is_object());
        assert_eq!(single["action"], "on");
        assert_eq!(single["device"], "hallway_light");

        let many = &json["rules"][1]["then"];
        assert!(many.is_array());
        assert_eq!(many.as_array().unwrap().len(), 2);
        assert_eq!(many[0]["action"], "off");
        assert_eq!(many[0]["device"], "hallway_light");
        assert_eq!(many[1]["action"], "on");
        assert_eq!(many[1]["device"], "entry_motion");
    }

    #[test]
    fn invalid_action_name_keeps_the_inner_serde_error() {
        // untagged derive に戻すと "data did not match any variant" になり、
        // 書き手（LLM / UI）が直せなくなる。内側のエラーを保つことを守る。
        let err = parse(
            r#"
version = 1
[[rules]]
name = "typo"
when = { at = "07:00" }
then = { action = "onn", device = "hallway_light" }
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(
            err.detail.contains("unknown variant"),
            "inner error should survive: {}",
            err.detail
        );
    }

    #[test]
    fn missing_device_field_keeps_the_inner_serde_error() {
        let err = parse(
            r#"
version = 1
[[rules]]
name = "device なし"
when = { at = "07:00" }
then = { action = "on" }
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(
            err.detail.contains("missing field"),
            "inner error should survive: {}",
            err.detail
        );
    }

    #[test]
    fn parses_active_window() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "昼だけ扇風機"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
active = { from = "06:00", to = "21:00" }
then = { action = "on", device = "study_fan" }
"#,
        )
        .unwrap();
        let w = file.rules[0].active.as_ref().expect("active が None");
        assert_eq!(w.from, "06:00");
        assert_eq!(w.to, "21:00");
    }

    #[test]
    fn active_window_is_none_when_absent() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "常時"
when = { at = "22:00" }
then = { action = "off", device = "living_aircon" }
"#,
        )
        .unwrap();
        assert!(file.rules[0].active.is_none());
    }

    #[test]
    fn active_window_requires_both_ends() {
        // 片側だけの窓は解釈が割れるので受け付けない。
        let err = parse(
            r#"
version = 1
[[rules]]
name = "片側だけ"
when = { at = "22:00" }
active = { from = "06:00" }
then = { action = "off", device = "living_aircon" }
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
    }

    #[test]
    fn serialized_rule_omits_absent_active_window() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "常時"
when = { at = "22:00" }
then = { action = "off", device = "living_aircon" }
[[rules]]
name = "昼だけ"
when = { at = "12:00" }
active = { from = "06:00", to = "21:00" }
then = { action = "off", device = "living_aircon" }
"#,
        )
        .unwrap();
        let v = serde_json::to_value(&file.rules).unwrap();
        // active を持たないルールの JSON は従来どおり（casad check の後方互換）。
        assert!(v[0].get("active").is_none());
        assert_eq!(v[1]["active"]["from"], "06:00");
        assert_eq!(v[1]["active"]["to"], "21:00");
    }
}
