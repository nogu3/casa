//! ルールファイル（rules.toml）の型・パース・検証。
//!
//! 書き手は LLM / UI を想定し、人間の手書きは前提にしない（ただし直接覗ける可読性は
//! 確保する）。形式は devices.toml と揃えて TOML。デバイス参照は casa-core の Config で
//! 読込時に検証し、発火前に不正なルールを弾く（ハイブリッド構成の link 側の価値）。
//!
//! 中身は `when`（トリガ）→ `then`（アクション）の素朴な対応。複数条件・遅延・複数
//! アクションなどの表現力拡張は後段で必要になったら足す。

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
    pub rules: Vec<Rule>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Rule {
    pub name: String,
    pub when: Trigger,
    pub then: Then,
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
            check_target(config, &rule.name, rule.then.device())?;
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
        casa_core::config::Device::Matter { node_id, .. } => {
            if parse_node_id(node_id).is_none() {
                return Err(CasaError::new(
                    ErrorKind::ConfigParse,
                    format!(
                        "rule \"{rule_name}\": device \"{device}\" node_id \"{node_id}\" は数値でないnode_id"
                    ),
                ));
            }
            Ok(())
        }
        _other => Err(CasaError::new(
            ErrorKind::ProtocolUnsupported,
            format!(
                "rule \"{rule_name}\": matter event trigger requires a matter device, but \"{device}\" is プロトコル不一致"
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
    fn parses_event_and_time_rules() {
        let file = parse(VALID).unwrap();
        assert_eq!(file.rules.len(), 2);
        assert!(matches!(file.rules[0].when, Trigger::Event { .. }));
        assert!(matches!(file.rules[1].when, Trigger::Time { .. }));
        assert!(matches!(file.rules[0].then, Then::On { .. }));
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
        match &file.rules[0].then {
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
        match &file.rules[0].then {
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
}
