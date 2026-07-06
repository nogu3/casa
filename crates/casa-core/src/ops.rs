//! サブコマンドの実処理。名前解決 → アダプタで引数組み立て → 子 CLI 実行 → 再整形。
//!
//! プロトコル固有の知識はすべて adapter 層にあり、ここはプロトコル非依存。
//! 新プロトコルを追加してもこのファイルは変更しない。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::adapter::{self, Adapter, ColorTemp, Invocation};
use crate::config::{Config, Device};
use crate::error::{CasaError, ErrorKind};
use crate::{output, runner};

/// `casa get <name> <property>`
pub fn get(config: &Config, name: &str, property: &str) -> Result<Value, CasaError> {
    let device = config.device(name)?;
    let adapter = require_adapter(device, "get")?;
    run_for_value(adapter.get(device, property), config, name, device, "get")
}

/// `casa set <name> <property> <value>`
pub fn set(config: &Config, name: &str, property: &str, value: &str) -> Result<Value, CasaError> {
    let device = config.device(name)?;
    let adapter = require_adapter(device, "set")?;
    run_for_value(
        adapter.set(device, property, value),
        config,
        name,
        device,
        "set",
    )
}

/// `casa validate` — 設定の妥当性を JSON で報告する（実機は呼ばない）。
/// `config::load` を通った時点で version・必須フィールド・未知プロトコルは検証済みなので、
/// ここでは追加で「アダプタ未実装のプロトコル」を警告として可視化する。設定としては
/// 妥当だが get/set/on/off が実行時に protocol_unsupported（exit 14）になるため。
pub fn validate(config: &Config, path: &Path) -> Value {
    let mut protocols: BTreeMap<&str, u32> = BTreeMap::new();
    let mut warnings: Vec<Value> = Vec::new();
    for (name, device) in &config.devices {
        *protocols.entry(device.protocol()).or_default() += 1;
        if adapter::adapter_for(device).is_none() {
            warnings.push(json!({
                "kind": "no_adapter",
                "device": name,
                "protocol": device.protocol(),
                "detail": format!(
                    "protocol \"{}\" has no adapter yet; get/set/on/off will fail at runtime",
                    device.protocol()
                ),
            }));
        }
    }
    output::validate_response(
        path,
        config.version,
        config.devices.len(),
        protocols,
        warnings,
    )
}

/// `casa on <name>` / `casa off <name>`
pub fn power(config: &Config, name: &str, on: bool) -> Result<Value, CasaError> {
    let op = if on { "on" } else { "off" };
    let device = config.device(name)?;
    let adapter = require_adapter(device, op)?;
    run_for_value(adapter.power(device, on), config, name, device, op)
}

/// `casa color-temp <name> --kelvin <k> | --mireds <m> [--transition <s>]`
pub fn color_temp(config: &Config, name: &str, color: &ColorTemp) -> Result<Value, CasaError> {
    let device = config.device(name)?;
    let adapter = require_adapter(device, "color-temp")?;
    run_for_value(
        adapter.color_temp(device, color),
        config,
        name,
        device,
        "color-temp",
    )
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
    let Some(adapter) = adapter::adapter_for(device) else {
        return Ok(None);
    };
    let Some(invocation) = adapter.describe(device) else {
        return Ok(None);
    };
    Ok(Some(execute(config, &invocation)?))
}

/// アダプタが組んだ呼び出しを実行し、casa スキーマの応答に包む。
/// 操作未対応（`None`）は `protocol_unsupported` に変換する。
fn run_for_value(
    invocation: Option<Invocation>,
    config: &Config,
    name: &str,
    device: &Device,
    operation: &str,
) -> Result<Value, CasaError> {
    let invocation = invocation.ok_or_else(|| unsupported(device, operation))?;
    let value = execute(config, &invocation)?;
    Ok(output::device_response(name, device, value))
}

/// バイナリ解決と子 CLI 実行。アダプタ層と runner 層の継ぎ目。
fn execute(config: &Config, invocation: &Invocation) -> Result<Value, CasaError> {
    let bin = runner::resolve_bin(invocation.bin, config);
    runner::run(&bin, &invocation.args)
}

fn require_adapter(device: &Device, operation: &str) -> Result<&'static dyn Adapter, CasaError> {
    adapter::adapter_for(device).ok_or_else(|| unsupported(device, operation))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    /// 仮想プロトコルのアダプタ。サブコマンドハンドラ（main.rs / ops の公開関数）を
    /// 一切触らずに、アダプタ trait の実装だけで ops のパイプラインを通せることを示す。
    struct VirtualAdapter;

    impl Adapter for VirtualAdapter {
        fn get(&self, _device: &Device, _property: &str) -> Option<Invocation> {
            // `echo` は引数をそのまま stdout に出すので、子 CLI の代役になる。
            Some(Invocation {
                bin: "echo",
                args: vec![r#"{"virtual": true}"#.into()],
            })
        }
        // set / describe / power は既定の None（未対応）のまま。
    }

    fn virtual_device() -> Device {
        Device::Echonet {
            ip: "192.0.2.99".into(),
            eoj: "0x000000".into(),
        }
    }

    #[test]
    fn virtual_adapter_runs_through_generic_pipeline() {
        let config = config::parse("version = 1").unwrap();
        let device = virtual_device();
        let invocation = VirtualAdapter.get(&device, "0x00");

        let response =
            run_for_value(invocation, &config, "virtual_device", &device, "get").unwrap();
        assert_eq!(response["device"], "virtual_device");
        assert_eq!(response["value"]["virtual"], true);
        assert!(response["timestamp"].is_string());
    }

    #[test]
    fn validate_reports_summary_and_flags_protocols_without_adapter() {
        let text = r#"
version = 1
[devices.aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"
[devices.lock]
protocol = "switchbot"
device_id = "DUMMY-XX-XX"
"#;
        let config = config::parse(text).unwrap();
        let report = validate(&config, std::path::Path::new("/tmp/devices.toml"));

        assert_eq!(report["valid"], true);
        assert_eq!(report["version"], 1);
        assert_eq!(report["device_count"], 2);
        assert_eq!(report["config"], "/tmp/devices.toml");
        assert_eq!(report["protocols"]["echonet"], 1);
        assert_eq!(report["protocols"]["switchbot"], 1);
        assert!(report["timestamp"].is_string());

        // switchbot はアダプタ未実装なので no_adapter 警告が 1 件だけ出る。
        let warnings = report["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["kind"], "no_adapter");
        assert_eq!(warnings[0]["device"], "lock");
        assert_eq!(warnings[0]["protocol"], "switchbot");
    }

    #[test]
    fn unsupported_operation_becomes_protocol_unsupported() {
        let config = config::parse("version = 1").unwrap();
        let device = virtual_device();
        let invocation = VirtualAdapter.set(&device, "0x80", "0x30"); // 既定: None

        let err = run_for_value(invocation, &config, "virtual_device", &device, "set").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
    }
}
