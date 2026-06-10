//! サブコマンドの実処理。名前解決 → 子 CLI 呼び出し → casa スキーマへの再整形。

use serde_json::Value;

use crate::config::{Config, Device};
use crate::error::{CasaError, ErrorKind};
use crate::{output, runner};

/// `casa get <name> <epc>`
pub fn get(config: &Config, name: &str, epc: &str) -> Result<Value, CasaError> {
    let device = config.device(name)?;
    match device {
        Device::Echonet { ip, eoj } => {
            let args = string_vec(&["get", "--ip", ip, "--eoj", eoj, "--epc", epc]);
            let value = run_enl(config, &args)?;
            Ok(output::device_response(name, device, value))
        }
        Device::Switchbot { .. } => Err(unsupported(device, "get")),
    }
}

/// ECHONET Lite の ON/OFF ショートカットのマッピング先。
/// プロトコルロジックではなく UX としてのハードコード（CLAUDE.md 参照）。
pub const ECHONET_POWER_EPC: &str = "0x80";
pub const ECHONET_POWER_ON: &str = "0x30";
pub const ECHONET_POWER_OFF: &str = "0x31";

/// `casa on <name>` / `casa off <name>`
pub fn power(config: &Config, name: &str, on: bool) -> Result<Value, CasaError> {
    let device = config.device(name)?;
    match device {
        Device::Echonet { .. } => {
            let value = if on {
                ECHONET_POWER_ON
            } else {
                ECHONET_POWER_OFF
            };
            set(config, name, ECHONET_POWER_EPC, value)
        }
        Device::Switchbot { .. } => Err(unsupported(device, if on { "on" } else { "off" })),
    }
}

/// `casa describe <name>`
pub fn describe(config: &Config, name: &str) -> Result<Value, CasaError> {
    let device = config.device(name)?;
    match describe_device(config, device)? {
        Some(properties) => Ok(output::describe_response(name, device, properties)),
        None => Err(unsupported(device, "describe")),
    }
}

/// プロパティマップを子 CLI から取得する。introspection 未対応のプロトコルは `None`。
/// `list --describe` がそのデバイスをスキップできるよう、未対応をエラーにしない。
pub fn describe_device(config: &Config, device: &Device) -> Result<Option<Value>, CasaError> {
    match device {
        Device::Echonet { ip, eoj } => {
            let args = string_vec(&["describe", "--ip", ip, "--eoj", eoj]);
            Ok(Some(run_enl(config, &args)?))
        }
        Device::Switchbot { .. } => Ok(None),
    }
}

/// `casa set <name> <epc> <value>`
pub fn set(config: &Config, name: &str, epc: &str, set_value: &str) -> Result<Value, CasaError> {
    let device = config.device(name)?;
    match device {
        Device::Echonet { ip, eoj } => {
            let args = string_vec(&[
                "set", "--ip", ip, "--eoj", eoj, "--epc", epc, "--value", set_value,
            ]);
            let value = run_enl(config, &args)?;
            Ok(output::device_response(name, device, value))
        }
        Device::Switchbot { .. } => Err(unsupported(device, "set")),
    }
}

fn run_enl(config: &Config, args: &[String]) -> Result<Value, CasaError> {
    let bin = runner::resolve_bin("enl", config);
    runner::run(&bin, args)
}

fn unsupported(device: &Device, operation: &str) -> CasaError {
    CasaError::new(
        ErrorKind::ProtocolUnsupported,
        format!(
            "operation \"{operation}\" is not yet supported for protocol \"{}\"",
            device.protocol()
        ),
    )
}

fn string_vec(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}
