//! casad が実行できるアクション。casa のサブコマンドへ写像される。
//!
//! `casad exec`（CLI 引数）と rules の `then`（TOML）の両方がこの 1 つの型を共有する。
//! そのため `ValueEnum`（clap）と `Deserialize`（serde）の両方を導出する。

use std::path::Path;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lower")]
pub enum Action {
    On,
    Off,
}

impl Action {
    /// 対応する casa サブコマンド名。
    pub fn subcommand(self) -> &'static str {
        match self {
            Action::On => "on",
            Action::Off => "off",
        }
    }

    /// casa の引数列へ変換する。casa の CLI 表面に対する casad の知識はここに閉じる。
    /// 設定パスは casa へ明示的に渡し、casa 側が XDG を再探索しないようにする。
    pub fn casa_args(self, name: &str, config: Option<&Path>) -> Vec<String> {
        let mut args = vec![self.subcommand().to_string(), name.to_string()];
        if let Some(path) = config {
            args.push("--config".to_string());
            args.push(path.to_string_lossy().into_owned());
        }
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casa_args_maps_action_and_passes_config() {
        let args = Action::On.casa_args("living_aircon", Some(Path::new("/tmp/d.toml")));
        assert_eq!(args, ["on", "living_aircon", "--config", "/tmp/d.toml"]);
    }

    #[test]
    fn casa_args_off_without_config() {
        let args = Action::Off.casa_args("bedroom_light", None);
        assert_eq!(args, ["off", "bedroom_light"]);
    }

    #[test]
    fn deserializes_from_lowercase_string() {
        #[derive(Deserialize)]
        struct Holder {
            action: Action,
        }
        let h: Holder = toml::from_str(r#"action = "on""#).unwrap();
        assert_eq!(h.action, Action::On);
    }
}
