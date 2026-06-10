//! 設定ファイル（devices.toml）の読み込みとバリデーション。
//!
//! 探索順: `--config` フラグ > 環境変数 `CASA_CONFIG`（clap が解決）>
//! `$XDG_CONFIG_HOME/casa/devices.toml` > `~/.config/casa/devices.toml`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CasaError, ErrorKind};

/// casa が理解する設定ファイルのバージョン。
pub const SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub devices: BTreeMap<String, Device>,
    /// 子 CLI バイナリのフルパス上書き（例: `enl = "/opt/bin/enl"`）。
    /// 環境変数 `CASA_<BIN>_BIN` の方が優先される。
    #[serde(default)]
    pub binaries: BTreeMap<String, String>,
}

/// デバイス定義。`protocol` フィールドが dispatch の唯一の真実。
/// 未知プロトコル・必須フィールド欠落は serde のタグ付き enum がエラーにする。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum Device {
    Echonet { ip: String, eoj: String },
    Switchbot { device_id: String },
}

impl Device {
    pub fn protocol(&self) -> &'static str {
        match self {
            Device::Echonet { .. } => "echonet",
            Device::Switchbot { .. } => "switchbot",
        }
    }
}

impl Config {
    /// 名前からデバイスを引く。無ければ exit code 11 相当のエラー。
    pub fn device(&self, name: &str) -> Result<&Device, CasaError> {
        self.devices.get(name).ok_or_else(|| {
            CasaError::new(
                ErrorKind::NameNotFound,
                format!("device \"{name}\" is not defined in the config file"),
            )
        })
    }
}

/// 既定の設定ファイルパス。
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("casa").join("devices.toml")
}

/// 設定ファイルを読み込み、バリデーションする。
pub fn load(path_override: Option<&Path>) -> Result<Config, CasaError> {
    let path = path_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_path);
    tracing::debug!(path = %path.display(), "loading config");

    let text = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CasaError::new(
                ErrorKind::ConfigMissing,
                format!("config file not found: {}", path.display()),
            )
        } else {
            CasaError::new(
                ErrorKind::ConfigParse,
                format!("failed to read config file {}: {e}", path.display()),
            )
        }
    })?;

    parse(&text).map_err(|mut e| {
        e.detail = format!("{}: {}", path.display(), e.detail);
        e
    })
}

/// TOML テキストをパースしてバリデーションする（テスト用に分離）。
pub fn parse(text: &str) -> Result<Config, CasaError> {
    let config: Config =
        toml::from_str(text).map_err(|e| CasaError::new(ErrorKind::ConfigParse, e.to_string()))?;

    if config.version != SUPPORTED_VERSION {
        return Err(CasaError::new(
            ErrorKind::ConfigParse,
            format!(
                "unsupported config version {} (casa supports version {}; \
                 migration is explicit, casa never rewrites your config)",
                config.version, SUPPORTED_VERSION
            ),
        ));
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
version = 1

[devices.living_aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"

[devices.entry_lock]
protocol = "switchbot"
device_id = "DUMMY-XX-XX"
"#;

    #[test]
    fn parses_valid_config() {
        let config = parse(VALID).unwrap();
        assert_eq!(config.version, 1);
        assert_eq!(config.devices.len(), 2);
        match config.device("living_aircon").unwrap() {
            Device::Echonet { ip, eoj } => {
                assert_eq!(ip, "192.0.2.10");
                assert_eq!(eoj, "0x013001");
            }
            other => panic!("unexpected device: {other:?}"),
        }
        match config.device("entry_lock").unwrap() {
            Device::Switchbot { device_id } => assert_eq!(device_id, "DUMMY-XX-XX"),
            other => panic!("unexpected device: {other:?}"),
        }
    }

    #[test]
    fn missing_file_is_config_missing() {
        let err = load(Some(Path::new("/nonexistent/casa/devices.toml"))).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigMissing);
    }

    #[test]
    fn invalid_toml_is_config_parse() {
        let err = parse("version = ").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
    }

    #[test]
    fn unknown_protocol_is_config_parse() {
        let text = r#"
version = 1
[devices.x]
protocol = "zigbee"
ip = "192.0.2.10"
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("zigbee"), "detail: {}", err.detail);
    }

    #[test]
    fn missing_required_field_is_config_parse() {
        let text = r#"
version = 1
[devices.x]
protocol = "echonet"
ip = "192.0.2.10"
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("eoj"), "detail: {}", err.detail);
    }

    #[test]
    fn unsupported_version_is_config_parse() {
        let err = parse("version = 2").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("version"), "detail: {}", err.detail);
    }

    #[test]
    fn unknown_name_is_name_not_found() {
        let config = parse(VALID).unwrap();
        let err = config.device("no_such_device").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NameNotFound);
    }
}
