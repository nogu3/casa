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
    /// デバイスをまとめて操作するグループ。書き系（on/off/set/invoke）のみ対応。
    /// メンバー整合性はロード時に検証済みなので、実行時の名前解決は失敗しない。
    #[serde(default)]
    pub groups: BTreeMap<String, Group>,
    /// 子 CLI バイナリのフルパス上書き（例: `enl = "/opt/bin/enl"`）。
    /// 環境変数 `CASA_<BIN>_BIN` の方が優先される。
    #[serde(default)]
    pub binaries: BTreeMap<String, String>,
}

/// デバイスグループ。ネスト（メンバーにグループ名）は不可。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub members: Vec<String>,
}

/// デバイス定義。`protocol` フィールドが dispatch の唯一の真実。
/// 未知プロトコル・必須フィールド欠落は serde のタグ付き enum がエラーにする。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum Device {
    Echonet {
        ip: String,
        eoj: String,
    },
    Switchbot {
        device_id: String,
    },
    Matter {
        node_id: String,
        /// OnOff ショートカット（`casa on`/`off`）が使うエンドポイント。
        /// 未指定なら `mat` 側の既定（1）に委ねる。`get`/`set` は
        /// `endpoint/cluster/attribute` セレクタ側で endpoint を持つのでここは使わない。
        #[serde(default)]
        endpoint: Option<u32>,
    },
}

impl Device {
    pub fn protocol(&self) -> &'static str {
        match self {
            Device::Echonet { .. } => "echonet",
            Device::Switchbot { .. } => "switchbot",
            Device::Matter { .. } => "matter",
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

    /// 名前が device / group のどちらかとして存在するかだけを検証する。
    /// グループはメンバー解決を伴う実行を casa 側（`Config::device` を使わない経路）に
    /// 委ねるため `&Device` を返せない。呼び出し前検証（casad の spawn 前チェックなど）で
    /// 「casa に投げてよい名前か」だけを確認したい場合に使う。
    pub fn ensure_target(&self, name: &str) -> Result<(), CasaError> {
        if self.devices.contains_key(name) || self.groups.contains_key(name) {
            Ok(())
        } else {
            Err(CasaError::new(
                ErrorKind::NameNotFound,
                format!("\"{name}\" is not defined as a device or group in the config file"),
            ))
        }
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

    // グループのバリデーション
    for (name, group) in &config.groups {
        if config.devices.contains_key(name) {
            return Err(CasaError::new(
                ErrorKind::ConfigParse,
                format!("group \"{name}\" collides with a device of the same name"),
            ));
        }
        if group.members.is_empty() {
            return Err(CasaError::new(
                ErrorKind::ConfigParse,
                format!("group \"{name}\" has no members"),
            ));
        }
        for member in &group.members {
            if config.devices.contains_key(member) {
                continue;
            }
            let detail = if config.groups.contains_key(member) {
                format!("group \"{name}\" member \"{member}\" is a group; groups cannot be nested")
            } else {
                format!("group \"{name}\" member \"{member}\" is not defined in [devices]")
            };
            return Err(CasaError::new(ErrorKind::ConfigParse, detail));
        }
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

    const VALID_WITH_GROUPS: &str = r#"
version = 1

[devices.living_light]
protocol = "matter"
node_id = "1234"

[devices.living_aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"

[groups.living]
members = ["living_light", "living_aircon"]
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
    fn parses_matter_device_with_optional_endpoint() {
        let text = r#"
version = 1

[devices.living_light]
protocol = "matter"
node_id = "1234"

[devices.strip_outlet2]
protocol = "matter"
node_id = "5678"
endpoint = 2
"#;
        let config = parse(text).unwrap();
        match config.device("living_light").unwrap() {
            Device::Matter { node_id, endpoint } => {
                assert_eq!(node_id, "1234");
                assert_eq!(*endpoint, None);
            }
            other => panic!("unexpected device: {other:?}"),
        }
        match config.device("strip_outlet2").unwrap() {
            Device::Matter { node_id, endpoint } => {
                assert_eq!(node_id, "5678");
                assert_eq!(*endpoint, Some(2));
            }
            other => panic!("unexpected device: {other:?}"),
        }
    }

    #[test]
    fn matter_missing_node_id_is_config_parse() {
        let text = r#"
version = 1
[devices.x]
protocol = "matter"
endpoint = 1
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("node_id"), "detail: {}", err.detail);
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

    #[test]
    fn ensure_target_accepts_device_name() {
        let config = parse(VALID).unwrap();
        config.ensure_target("living_aircon").unwrap();
    }

    #[test]
    fn ensure_target_accepts_group_name() {
        let config = parse(VALID_WITH_GROUPS).unwrap();
        config.ensure_target("living").unwrap();
    }

    #[test]
    fn ensure_target_unknown_name_is_name_not_found() {
        let config = parse(VALID_WITH_GROUPS).unwrap();
        let err = config.ensure_target("no_such_target").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NameNotFound);
    }

    #[test]
    fn parses_groups() {
        let config = parse(VALID_WITH_GROUPS).unwrap();
        let group = config.groups.get("living").unwrap();
        assert_eq!(group.members, vec!["living_light", "living_aircon"]);
    }

    #[test]
    fn config_without_groups_stays_compatible() {
        let config = parse(VALID).unwrap();
        assert!(config.groups.is_empty());
    }

    #[test]
    fn group_member_not_in_devices_is_config_parse() {
        let text = r#"
version = 1
[devices.a]
protocol = "matter"
node_id = "1"
[groups.g]
members = ["a", "ghost"]
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("ghost"), "detail: {}", err.detail);
    }

    #[test]
    fn group_name_colliding_with_device_is_config_parse() {
        let text = r#"
version = 1
[devices.living]
protocol = "matter"
node_id = "1"
[groups.living]
members = ["living"]
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("living"), "detail: {}", err.detail);
    }

    #[test]
    fn empty_group_is_config_parse() {
        let text = r#"
version = 1
[groups.g]
members = []
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("no members"), "detail: {}", err.detail);
    }

    #[test]
    fn nested_group_is_config_parse() {
        let text = r#"
version = 1
[devices.a]
protocol = "matter"
node_id = "1"
[groups.inner]
members = ["a"]
[groups.outer]
members = ["inner"]
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("nested"), "detail: {}", err.detail);
    }
}
