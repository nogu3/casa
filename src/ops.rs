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
