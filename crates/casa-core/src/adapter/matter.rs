//! Matter アダプタ。実体は `mat`（chip-tool ラッパ）のサブプロセス呼び出し。
//!
//! Matter のアドレッシングは (node_id, endpoint, cluster, attribute) で、ECHONET の
//! 単一 EPC とはモデルが違う。casa の `get`/`set` が渡す単一セレクタ文字列を
//! `endpoint/cluster/attribute`（chip-tool 表記、例 `1/onoff/on-off`）または
//! `cluster/attribute`（endpoint は mat の既定 1）として解釈し、`mat read`/`write` の
//! フラグ引数（`--node`/`--endpoint`/`--cluster`/`--attribute`）に割り当てる。
//! casa 自身はクラスタ・属性名を理解しない——プロトコル知識はこのアダプタに閉じる。
//!
//! セレクタの要素数が 2/3 以外のときはフラグに割り当てようがないため
//! `None`（protocol_unsupported）を返す。属性名等の妥当性は `mat`（chip-tool）側が
//! 検証し、不正は exit code 伝播で返る。
//!
//! `on`/`off` は属性 write ではなく OnOff コマンドの invoke なので、`mat` の
//! 高頻度ショートカット（`mat on`/`off`）に委ねる。エンドポイントは設定の
//! `endpoint`（未指定なら `mat` の既定 1）。

use super::{Adapter, Invocation};
use crate::config::Device;

const BIN: &str = "mat";

pub struct MatterAdapter;

/// デバイス定義から (node_id, on/off 用エンドポイント) を取り出す。dispatch は
/// `adapter_for` が variant で行うので、他 variant が来ることはない。
fn address(device: &Device) -> Option<(&str, Option<u32>)> {
    match device {
        Device::Matter { node_id, endpoint } => Some((node_id, *endpoint)),
        _ => None,
    }
}

fn invocation(args: Vec<String>) -> Invocation {
    Invocation { bin: BIN, args }
}

/// セレクタを mat のフラグ引数列に変換する。
/// `endpoint/cluster/attribute` → `--endpoint <ep> --cluster <c> --attribute <a>`、
/// `cluster/attribute` → `--cluster <c> --attribute <a>`（endpoint は mat 既定）。
fn selector_flags(selector: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = selector.split('/').collect();
    let (endpoint, cluster, attribute) = match parts.as_slice() {
        [ep, c, a] => (Some(*ep), *c, *a),
        [c, a] => (None, *c, *a),
        _ => return None,
    };
    let mut flags = Vec::new();
    if let Some(ep) = endpoint {
        flags.push("--endpoint".to_string());
        flags.push(ep.to_string());
    }
    flags.push("--cluster".to_string());
    flags.push(cluster.to_string());
    flags.push("--attribute".to_string());
    flags.push(attribute.to_string());
    Some(flags)
}

impl Adapter for MatterAdapter {
    fn get(&self, device: &Device, property: &str) -> Option<Invocation> {
        let (node, _) = address(device)?;
        let mut args = vec!["read".to_string(), "--node".to_string(), node.to_string()];
        args.extend(selector_flags(property)?);
        Some(invocation(args))
    }

    fn set(&self, device: &Device, property: &str, value: &str) -> Option<Invocation> {
        let (node, _) = address(device)?;
        let mut args = vec!["write".to_string(), "--node".to_string(), node.to_string()];
        args.extend(selector_flags(property)?);
        args.push("--value".to_string());
        args.push(value.to_string());
        Some(invocation(args))
    }

    fn describe(&self, device: &Device) -> Option<Invocation> {
        let (node, _) = address(device)?;
        Some(invocation(vec![
            "describe".to_string(),
            "--node".to_string(),
            node.to_string(),
        ]))
    }

    fn power(&self, device: &Device, on: bool) -> Option<Invocation> {
        let (node, endpoint) = address(device)?;
        let cmd = if on { "on" } else { "off" };
        let mut args = vec![cmd.to_string(), "--node".to_string(), node.to_string()];
        if let Some(ep) = endpoint {
            args.push("--endpoint".to_string());
            args.push(ep.to_string());
        }
        Some(invocation(args))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device::Matter {
            node_id: "1234".into(),
            endpoint: None,
        }
    }

    fn device_on_endpoint(ep: u32) -> Device {
        Device::Matter {
            node_id: "1234".into(),
            endpoint: Some(ep),
        }
    }

    fn args(inv: &Invocation) -> Vec<&str> {
        inv.args.iter().map(String::as_str).collect()
    }

    #[test]
    fn get_maps_selector_to_mat_read_flags() {
        let inv = MatterAdapter.get(&device(), "1/onoff/on-off").unwrap();
        assert_eq!(inv.bin, "mat");
        assert_eq!(
            args(&inv),
            [
                "read",
                "--node",
                "1234",
                "--endpoint",
                "1",
                "--cluster",
                "onoff",
                "--attribute",
                "on-off"
            ]
        );
    }

    #[test]
    fn get_without_endpoint_relies_on_mat_default() {
        let inv = MatterAdapter.get(&device(), "onoff/on-off").unwrap();
        assert_eq!(
            args(&inv),
            ["read", "--node", "1234", "--cluster", "onoff", "--attribute", "on-off"]
        );
    }

    #[test]
    fn get_rejects_malformed_selector() {
        assert!(MatterAdapter.get(&device(), "on-off").is_none());
        assert!(MatterAdapter.get(&device(), "1/onoff/on-off/extra").is_none());
    }

    #[test]
    fn set_maps_selector_and_value_to_mat_write_flags() {
        let inv = MatterAdapter
            .set(&device(), "1/levelcontrol/current-level", "128")
            .unwrap();
        assert_eq!(
            args(&inv),
            [
                "write",
                "--node",
                "1234",
                "--endpoint",
                "1",
                "--cluster",
                "levelcontrol",
                "--attribute",
                "current-level",
                "--value",
                "128"
            ]
        );
    }

    #[test]
    fn describe_builds_mat_describe_args() {
        let inv = MatterAdapter.describe(&device()).unwrap();
        assert_eq!(args(&inv), ["describe", "--node", "1234"]);
    }

    #[test]
    fn power_on_without_endpoint_relies_on_mat_default() {
        let inv = MatterAdapter.power(&device(), true).unwrap();
        assert_eq!(args(&inv), ["on", "--node", "1234"]);
    }

    #[test]
    fn power_off_with_endpoint_passes_flag() {
        let inv = MatterAdapter.power(&device_on_endpoint(2), false).unwrap();
        assert_eq!(args(&inv), ["off", "--node", "1234", "--endpoint", "2"]);
    }
}
