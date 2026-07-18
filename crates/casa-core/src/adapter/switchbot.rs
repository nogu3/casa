//! SwitchBot アダプタ。実体は自作 `swb`（SwitchBot クラウド API v1.1 ラッパ）の
//! サブプロセス呼び出し。**公式 CLI ではない**。
//!
//! SwitchBot クラウド API には単一プロパティ read/write が無い（`status` は全状態を
//! 一括で返す GET のみ、制御は `cmd` によるコマンド送信）。そのため casa の
//! `get`/`set`/`describe` はこのプロトコルでは未対応（trait 既定の `None` = exit 14）。
//! 読み取りは `casa invoke <name> status`、制御は `on`/`off` と `casa invoke <name> cmd <command>`。
//!
//! アドレス（device_id）は swb では常にサブコマンド直後の第 1 位置引数に来る
//! （`status <device>` / `cmd <device> <command>`）。Matter の `--node` フラグ注入とは
//! 対照的に、swb は位置引数注入。認証は swb 側の責務で、casa は何も渡さない。

use super::{Adapter, Invocation};
use crate::config::Device;

const BIN: &str = "swb";

pub struct SwitchbotAdapter;

/// デバイス定義から device_id を取り出す。dispatch は `adapter_for` が variant で
/// 行うので、他 variant が来ることはない。
fn device_id(device: &Device) -> Option<&str> {
    match device {
        Device::Switchbot { device_id } => Some(device_id),
        _ => None,
    }
}

fn invocation(args: Vec<String>) -> Invocation {
    Invocation { bin: BIN, args }
}

impl Adapter for SwitchbotAdapter {
    /// `on`/`off` は SwitchBot の turnOn/turnOff コマンド送信。
    fn power(&self, device: &Device, on: bool) -> Option<Invocation> {
        let id = device_id(device)?;
        let command = if on { "turnOn" } else { "turnOff" };
        Some(invocation(vec![
            "cmd".to_string(),
            id.to_string(),
            command.to_string(),
        ]))
    }

    /// 長尾のプロトコル固有操作。`command` は swb のサブコマンド名（`status` / `cmd` 等）を
    /// そのまま受け取り、device_id をサブコマンド直後の第 1 位置引数に注入して後続 `args` を
    /// 素通しする。
    fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation> {
        let id = device_id(device)?;
        let mut all = vec![command.to_string(), id.to_string()];
        all.extend(args.iter().cloned());
        Some(invocation(all))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device::Switchbot {
            device_id: "DUMMY-XX-XX".into(),
        }
    }

    fn args(inv: &Invocation) -> Vec<&str> {
        inv.args.iter().map(String::as_str).collect()
    }

    #[test]
    fn power_on_sends_turn_on_command() {
        let inv = SwitchbotAdapter.power(&device(), true).unwrap();
        assert_eq!(inv.bin, "swb");
        assert_eq!(args(&inv), ["cmd", "DUMMY-XX-XX", "turnOn"]);
    }

    #[test]
    fn power_off_sends_turn_off_command() {
        let inv = SwitchbotAdapter.power(&device(), false).unwrap();
        assert_eq!(args(&inv), ["cmd", "DUMMY-XX-XX", "turnOff"]);
    }

    #[test]
    fn invoke_status_injects_device_id_as_positional() {
        let inv = SwitchbotAdapter.invoke(&device(), "status", &[]).unwrap();
        assert_eq!(inv.bin, "swb");
        assert_eq!(args(&inv), ["status", "DUMMY-XX-XX"]);
    }

    #[test]
    fn invoke_cmd_passes_command_and_args_through() {
        let extra: Vec<String> = vec!["turnOn".into()];
        let inv = SwitchbotAdapter.invoke(&device(), "cmd", &extra).unwrap();
        assert_eq!(args(&inv), ["cmd", "DUMMY-XX-XX", "turnOn"]);
    }

    #[test]
    fn invoke_passes_trailing_flags_through() {
        let extra: Vec<String> = vec!["setBrightness".into(), "--param".into(), "50".into()];
        let inv = SwitchbotAdapter.invoke(&device(), "cmd", &extra).unwrap();
        assert_eq!(
            args(&inv),
            ["cmd", "DUMMY-XX-XX", "setBrightness", "--param", "50"]
        );
    }

    #[test]
    fn get_set_describe_are_unsupported() {
        assert!(SwitchbotAdapter.get(&device(), "power").is_none());
        assert!(SwitchbotAdapter.set(&device(), "power", "on").is_none());
        assert!(SwitchbotAdapter.describe(&device()).is_none());
    }
}
