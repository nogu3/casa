//! Matter アダプタ。実体は `mat`（chip-tool ラッパ）のサブプロセス呼び出し。
//!
//! Matter のアドレッシングは (node_id, endpoint, cluster, attribute) で、ECHONET の
//! 単一 EPC とはモデルが違う。casa の `get`/`set` が渡す単一セレクタ文字列を
//! `endpoint/cluster/attribute`（chip-tool 表記、例 `1/onoff/on-off`）として解釈し、
//! `mat read`/`write` の位置引数に割り当てる。casa 自身はこのセレクタを理解しない
//! ——プロトコル知識はこのアダプタに閉じる。
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

/// `endpoint/cluster/attribute` セレクタを位置引数列に分解する。
/// 要素数・属性名の妥当性は `mat`（chip-tool）側が検証し、不正は exit code 伝播で返る。
fn selector_parts(selector: &str) -> impl Iterator<Item = String> + '_ {
    selector.split('/').map(str::to_string)
}

impl Adapter for MatterAdapter {
    fn get(&self, device: &Device, epc: &str) -> Option<Invocation> {
        let (node, _) = address(device)?;
        let mut args = vec!["read".to_string(), node.to_string()];
        args.extend(selector_parts(epc));
        Some(invocation(args))
    }

    fn set(&self, device: &Device, epc: &str, value: &str) -> Option<Invocation> {
        let (node, _) = address(device)?;
        let mut args = vec!["write".to_string(), node.to_string()];
        args.extend(selector_parts(epc));
        args.push(value.to_string());
        Some(invocation(args))
    }

    fn describe(&self, device: &Device) -> Option<Invocation> {
        let (node, _) = address(device)?;
        Some(invocation(vec!["describe".to_string(), node.to_string()]))
    }

    fn power(&self, device: &Device, on: bool) -> Option<Invocation> {
        let (node, endpoint) = address(device)?;
        let cmd = if on { "on" } else { "off" };
        let mut args = vec![cmd.to_string(), node.to_string()];
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
    fn get_splits_selector_into_mat_read_args() {
        let inv = MatterAdapter.get(&device(), "1/onoff/on-off").unwrap();
        assert_eq!(inv.bin, "mat");
        assert_eq!(args(&inv), ["read", "1234", "1", "onoff", "on-off"]);
    }

    #[test]
    fn set_splits_selector_and_appends_value() {
        let inv = MatterAdapter
            .set(&device(), "1/levelcontrol/current-level", "128")
            .unwrap();
        assert_eq!(
            args(&inv),
            ["write", "1234", "1", "levelcontrol", "current-level", "128"]
        );
    }

    #[test]
    fn describe_builds_mat_describe_args() {
        let inv = MatterAdapter.describe(&device()).unwrap();
        assert_eq!(args(&inv), ["describe", "1234"]);
    }

    #[test]
    fn power_on_without_endpoint_relies_on_mat_default() {
        let inv = MatterAdapter.power(&device(), true).unwrap();
        assert_eq!(args(&inv), ["on", "1234"]);
    }

    #[test]
    fn power_off_with_endpoint_passes_flag() {
        let inv = MatterAdapter.power(&device_on_endpoint(2), false).unwrap();
        assert_eq!(args(&inv), ["off", "1234", "--endpoint", "2"]);
    }
}
