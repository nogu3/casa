//! サブコマンドの実処理。名前解決 → アダプタで引数組み立て → 子 CLI 実行 → 再整形。
//!
//! プロトコル固有の知識はすべて adapter 層にあり、ここはプロトコル非依存。
//! 新プロトコルを追加してもこのファイルは変更しない。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Value};

use crate::adapter::{self, Adapter, ColorTemp, Invocation};
use crate::config::{Config, Device, Group};
use crate::error::{CasaError, ErrorKind};
use crate::{output, runner};

/// `casa get <name> <property>`
pub fn get(config: &Config, name: &str, property: &str) -> Result<Value, CasaError> {
    reject_group(config, name, "get")?;
    let device = config.device(name)?;
    let adapter = require_adapter(device, "get")?;
    run_for_value(adapter.get(device, property), config, name, device, "get")
}

/// `casa set <name> <property> <value>`
pub fn set(config: &Config, name: &str, property: &str, value: &str) -> Result<Value, CasaError> {
    if let Some(group) = config.groups.get(name) {
        return run_group(config, name, group, "set", |adapter, device| {
            adapter.set(device, property, value)
        });
    }
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
        config.groups.len(),
        protocols,
        warnings,
    )
}

/// `casa on <name>` / `casa off <name>`
pub fn power(config: &Config, name: &str, on: bool) -> Result<Value, CasaError> {
    let op = if on { "on" } else { "off" };
    if let Some(group) = config.groups.get(name) {
        return run_group(config, name, group, op, |adapter, device| {
            adapter.power(device, on)
        });
    }
    let device = config.device(name)?;
    let adapter = require_adapter(device, op)?;
    run_for_value(adapter.power(device, on), config, name, device, op)
}

/// `casa color-temp <name> --kelvin <k> | --mireds <m> [--transition <s>]`
pub fn color_temp(config: &Config, name: &str, color: &ColorTemp) -> Result<Value, CasaError> {
    if let Some(group) = config.groups.get(name) {
        return run_group(config, name, group, "color-temp", |adapter, device| {
            adapter.color_temp(device, color)
        });
    }
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

/// `casa invoke <name> <command> [args...]` — 長尾のプロトコル固有操作の汎用動詞。
/// `command` は子 CLI のサブコマンド名そのままで、casa は解釈しない。
pub fn invoke(
    config: &Config,
    name: &str,
    command: &str,
    args: &[String],
) -> Result<Value, CasaError> {
    if let Some(group) = config.groups.get(name) {
        ensure_uniform_protocol(config, name, group, command)?;
        return match run_group(config, name, group, command, |adapter, device| {
            adapter.invoke(device, command, args)
        }) {
            Ok(mut response) => {
                response["command"] = json!(command);
                Ok(response)
            }
            Err(mut err) => {
                if let Some(response) = err.response.as_mut() {
                    response["command"] = json!(command);
                }
                Err(err)
            }
        };
    }
    let device = config.device(name)?;
    let adapter = require_adapter(device, command)?;
    let invocation = adapter
        .invoke(device, command, args)
        .ok_or_else(|| unsupported(device, command))?;
    let value = execute(config, &invocation)?;
    Ok(output::invoke_response(name, device, command, value))
}

/// invoke のコマンド解釈はプロトコル依存なので、混在プロトコルのグループは
/// 「同名コマンドが別プロトコルで別の意味に実行される」事故を防ぐため spawn 前に拒否する。
fn ensure_uniform_protocol(
    config: &Config,
    group_name: &str,
    group: &Group,
    command: &str,
) -> Result<(), CasaError> {
    let protocols: BTreeSet<&str> = group
        .members
        .iter()
        .map(|m| Ok(config.device(m)?.protocol()))
        .collect::<Result<_, CasaError>>()?;
    if protocols.len() > 1 {
        let found: Vec<&str> = protocols.into_iter().collect();
        return Err(CasaError::new(
            ErrorKind::ProtocolUnsupported,
            format!(
                "invoke \"{command}\" on group \"{group_name}\" requires all members to \
                 share one protocol (found: {})",
                found.join(", ")
            ),
        ));
    }
    Ok(())
}

/// `casa describe <name>`
pub fn describe(config: &Config, name: &str) -> Result<Value, CasaError> {
    reject_group(config, name, "describe")?;
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

/// グループ書き系操作の共通パイプライン。各メンバーの Invocation を組み、
/// 全子プロセスを並列に spawn してから、設定ファイル上のメンバー記載順に回収する。
///
/// - Invocation を組めないメンバー（アダプタ未実装 / 操作未対応）は spawn せず
///   メンバー別エラーとして results に載せる。
/// - 1 件でも失敗があれば `group_partial_failure`（exit 15）。stdout に出すべき
///   メンバー別結果は `CasaError::response` に載せて main まで運ぶ。
fn run_group(
    config: &Config,
    group_name: &str,
    group: &Group,
    operation: &str,
    build: impl Fn(&'static dyn Adapter, &Device) -> Option<Invocation>,
) -> Result<Value, CasaError> {
    // メンバー名はロード時に検証済みなので device() は失敗しない。
    let members: Vec<(&String, &Device)> = group
        .members
        .iter()
        .map(|m| Ok((m, config.device(m)?)))
        .collect::<Result<_, CasaError>>()?;

    let prepared: Vec<Result<Invocation, CasaError>> = members
        .iter()
        .map(|(_, device)| {
            let adapter = require_adapter(device, operation)?;
            build(adapter, device).ok_or_else(|| unsupported(device, operation))
        })
        .collect();

    let commands: Vec<(String, Vec<String>)> = prepared
        .iter()
        .filter_map(|p| p.as_ref().ok())
        .map(|inv| (runner::resolve_bin(inv.bin, config), inv.args.clone()))
        .collect();
    let mut spawned = runner::run_parallel(&commands).into_iter();

    let outcomes: Vec<Result<Value, CasaError>> = prepared
        .into_iter()
        .map(|p| match p {
            Ok(_) => spawned
                .next()
                .expect("run_parallel returns one result per command"),
            Err(e) => Err(e),
        })
        .collect();

    let failed = outcomes.iter().filter(|o| o.is_err()).count();
    let results: Vec<Value> = members
        .iter()
        .zip(&outcomes)
        .map(|((name, device), outcome)| output::group_member_result(name, device, outcome))
        .collect();
    let response = output::group_response(group_name, results);

    if failed == 0 {
        Ok(response)
    } else {
        Err(CasaError::new(
            ErrorKind::GroupPartialFailure,
            format!(
                "{failed}/{} member(s) of group \"{group_name}\" failed during \"{operation}\"",
                group.members.len()
            ),
        )
        .with_response(response))
    }
}

/// 読み系（get / describe）はグループ非対応。グループ名なら明示エラーにする
/// （黙って name_not_found にすると「なぜ list には出るのに」と混乱するため）。
fn reject_group(config: &Config, name: &str, operation: &str) -> Result<(), CasaError> {
    if config.groups.contains_key(name) {
        return Err(CasaError::new(
            ErrorKind::ProtocolUnsupported,
            format!("groups are not supported for \"{operation}\"; specify a device name"),
        ));
    }
    Ok(())
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
        assert_eq!(report["group_count"], 0);
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

    const GROUP_CONFIG: &str = r#"
version = 1

[devices.light1]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"

[devices.light2]
protocol = "echonet"
ip = "192.0.2.12"
eoj = "0x029101"

[groups.living]
members = ["light1", "light2"]
"#;

    /// echo を子 CLI の代役にして、run_group の成功パスを通す。
    fn echo_invocation(device: &Device) -> Option<Invocation> {
        let Device::Echonet { ip, .. } = device else {
            panic!("test config only has echonet devices");
        };
        Some(Invocation {
            bin: "echo",
            args: vec![format!(r#"{{"ip": "{ip}"}}"#)],
        })
    }

    #[test]
    fn run_group_collects_member_results_in_config_order() {
        let config = config::parse(GROUP_CONFIG).unwrap();
        let group = config.groups.get("living").unwrap();

        let response =
            run_group(&config, "living", group, "on", |_, device| echo_invocation(device))
                .unwrap();

        assert_eq!(response["group"], "living");
        assert!(response["timestamp"].is_string());
        let results = response["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["device"], "light1");
        assert_eq!(results[0]["ok"], true);
        assert_eq!(results[0]["value"]["ip"], "192.0.2.11");
        assert_eq!(results[1]["device"], "light2");
        assert_eq!(results[1]["value"]["ip"], "192.0.2.12");
    }

    #[test]
    fn run_group_partial_failure_is_exit_15_with_response() {
        let config = config::parse(GROUP_CONFIG).unwrap();
        let group = config.groups.get("living").unwrap();

        // light2 だけ操作未対応（None）にして部分失敗を作る。
        let err = run_group(&config, "living", group, "on", |_, device| match device {
            Device::Echonet { ip, .. } if ip == "192.0.2.11" => echo_invocation(device),
            _ => None,
        })
        .unwrap_err();

        assert_eq!(err.kind, ErrorKind::GroupPartialFailure);
        assert_eq!(err.exit_code(), 15);
        let response = err.response.unwrap();
        let results = response["results"].as_array().unwrap();
        assert_eq!(results[0]["ok"], true);
        assert_eq!(results[1]["ok"], false);
        assert_eq!(results[1]["error"]["kind"], "protocol_unsupported");
    }

    #[test]
    fn power_dispatches_group_names_to_group_pipeline() {
        // enl を存在しないパスに向けることで、「グループ経路に入り、メンバーごとに
        // child_not_found で失敗し、exit 15 が返る」ことを実機なしで検証する。
        let text = format!("{GROUP_CONFIG}\n[binaries]\nenl = \"/nonexistent/enl\"\n");
        let config = config::parse(&text).unwrap();

        let err = power(&config, "living", true).unwrap_err();

        assert_eq!(err.kind, ErrorKind::GroupPartialFailure);
        let response = err.response.unwrap();
        let results = response["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["error"]["kind"], "child_not_found");
        assert_eq!(results[0]["error"]["exit_code"], 12);
    }

    #[test]
    fn get_and_describe_reject_group_names() {
        let config = config::parse(GROUP_CONFIG).unwrap();

        let err = get(&config, "living", "0x80").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
        assert!(err.detail.contains("get"), "detail: {}", err.detail);

        let err = describe(&config, "living").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
        assert!(err.detail.contains("describe"), "detail: {}", err.detail);
    }

    const MIXED_GROUP_CONFIG: &str = r#"
version = 1

[devices.light1]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"

[devices.light2]
protocol = "matter"
node_id = "1234"

[groups.mixed]
members = ["light1", "light2"]
"#;

    #[test]
    fn invoke_rejects_mixed_protocol_group_before_spawn() {
        let config = config::parse(MIXED_GROUP_CONFIG).unwrap();

        let err = invoke(&config, "mixed", "blink", &[]).unwrap_err();

        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
        assert_eq!(err.exit_code(), 14);
        assert!(err.detail.contains("mixed"), "detail: {}", err.detail);
        assert!(err.detail.contains("echonet"), "detail: {}", err.detail);
        assert!(err.detail.contains("matter"), "detail: {}", err.detail);
        // spawn 前に拒否されるのでメンバー別結果は無い。
        assert!(err.response.is_none());
    }

    #[test]
    fn invoke_group_enters_group_pipeline_and_tags_command() {
        // enl を存在しないパスに向け、「グループ経路に入り exit 15 が返る」ことを
        // 実機なしで検証する（power のグループテストと同じ手法）。
        let text = format!("{GROUP_CONFIG}\n[binaries]\nenl = \"/nonexistent/enl\"\n");
        let config = config::parse(&text).unwrap();

        let err = invoke(&config, "living", "blink", &[]).unwrap_err();

        assert_eq!(err.kind, ErrorKind::GroupPartialFailure);
        let response = err.response.unwrap();
        assert_eq!(response["command"], "blink");
        assert_eq!(response["group"], "living");
        assert_eq!(response["results"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn invoke_on_protocol_without_adapter_is_protocol_unsupported() {
        let text = r#"
version = 1
[devices.lock]
protocol = "switchbot"
device_id = "DUMMY-XX-XX"
"#;
        let config = config::parse(text).unwrap();

        let err = invoke(&config, "lock", "press", &[]).unwrap_err();

        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
    }
}
