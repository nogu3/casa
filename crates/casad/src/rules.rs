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

use crate::action::Action;

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
    /// 時刻: 毎日その時刻（HH:MM）になったとき。
    /// 例: `when = { at = "22:00" }`
    Time { at: String },
}

/// アクション。`then = { action = "on", device = "hallway_light" }`
#[derive(Debug, Deserialize, Serialize)]
pub struct Then {
    pub action: Action,
    pub device: String,
}

/// TOML 文字列をパースし、バージョンを検証する。
pub fn parse(text: &str) -> Result<RuleFile, CasaError> {
    let file: RuleFile = toml::from_str(text)
        .map_err(|e| CasaError::new(ErrorKind::ConfigParse, format!("failed to parse rules: {e}")))?;

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

impl RuleFile {
    /// 参照するデバイス名がすべて config に存在するか検証する（発火前に弾く）。
    /// 未定義名は `name_not_found`（exit 11）。エラーにはルール名を添える。
    pub fn validate(&self, config: &Config) -> Result<(), CasaError> {
        for rule in &self.rules {
            if let Trigger::Event { device, .. } = &rule.when {
                check_device(config, &rule.name, device)?;
            }
            check_device(config, &rule.name, &rule.then.device)?;
        }
        Ok(())
    }
}

fn check_device(config: &Config, rule_name: &str, device: &str) -> Result<(), CasaError> {
    config.device(device).map(|_| ()).map_err(|e| {
        CasaError::new(e.kind, format!("rule \"{rule_name}\": {}", e.detail))
    })
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
        assert_eq!(file.rules[0].then.action, Action::On);
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
}
