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

/// Matter の addressing mode。node（unicast）か group（groupcast）。
/// ロード時バリデーションでちょうど一方が保証されるので、両立/両欠落は来ない。
enum MatterAddr<'a> {
    Node {
        node_id: &'a str,
        endpoint: Option<u32>,
    },
    Group {
        group: &'a str,
        endpoint: Option<u32>,
    },
}

fn address(device: &Device) -> Option<MatterAddr<'_>> {
    match device {
        Device::Matter {
            node_id: Some(node_id),
            endpoint,
            ..
        } => Some(MatterAddr::Node {
            node_id,
            endpoint: *endpoint,
        }),
        Device::Matter {
            group: Some(group),
            endpoint,
            ..
        } => Some(MatterAddr::Group {
            group,
            endpoint: *endpoint,
        }),
        _ => None,
    }
}

/// `--endpoint <ep>` を（設定にあれば）末尾に足す。node/group 共通。
fn push_endpoint(args: &mut Vec<String>, endpoint: Option<u32>) {
    if let Some(ep) = endpoint {
        args.push("--endpoint".to_string());
        args.push(ep.to_string());
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
        let MatterAddr::Node { node_id, .. } = address(device)? else {
            return None;
        };
        let mut args = vec!["read".to_string(), "--node".to_string(), node_id.to_string()];
        args.extend(selector_flags(property)?);
        Some(invocation(args))
    }

    fn set(&self, device: &Device, property: &str, value: &str) -> Option<Invocation> {
        let MatterAddr::Node { node_id, .. } = address(device)? else {
            return None;
        };
        let mut args = vec!["write".to_string(), "--node".to_string(), node_id.to_string()];
        args.extend(selector_flags(property)?);
        args.push("--value".to_string());
        args.push(value.to_string());
        Some(invocation(args))
    }

    fn describe(&self, device: &Device) -> Option<Invocation> {
        let MatterAddr::Node { node_id, .. } = address(device)? else {
            return None;
        };
        Some(invocation(vec![
            "describe".to_string(),
            "--node".to_string(),
            node_id.to_string(),
        ]))
    }

    fn power(&self, device: &Device, on: bool) -> Option<Invocation> {
        let cmd = if on { "on" } else { "off" };
        match address(device)? {
            MatterAddr::Node { node_id, endpoint } => {
                let mut args =
                    vec![cmd.to_string(), "--node".to_string(), node_id.to_string()];
                push_endpoint(&mut args, endpoint);
                Some(invocation(args))
            }
            MatterAddr::Group { group, endpoint } => {
                // groupcast: `mat group invoke --group <g> --cluster onoff --command on|off`
                let mut args = vec![
                    "group".to_string(),
                    "invoke".to_string(),
                    "--group".to_string(),
                    group.to_string(),
                    "--cluster".to_string(),
                    "onoff".to_string(),
                    "--command".to_string(),
                    cmd.to_string(),
                ];
                push_endpoint(&mut args, endpoint);
                Some(invocation(args))
            }
        }
    }

    /// endpoint は設定にあれば注入する（`power` と同じ流儀）。group では `mat group`
    /// サブコマンドを 1 語 prepend し、`--group` を注入して残りを素通しする。
    fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation> {
        match address(device)? {
            MatterAddr::Node { node_id, endpoint } => {
                let mut all =
                    vec![command.to_string(), "--node".to_string(), node_id.to_string()];
                push_endpoint(&mut all, endpoint);
                all.extend(args.iter().cloned());
                Some(invocation(all))
            }
            MatterAddr::Group { group, endpoint } => {
                let mut all = vec![
                    "group".to_string(),
                    command.to_string(),
                    "--group".to_string(),
                    group.to_string(),
                ];
                push_endpoint(&mut all, endpoint);
                all.extend(args.iter().cloned());
                Some(invocation(all))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device::Matter {
            node_id: Some("1234".into()),
            group: None,
            endpoint: None,
        }
    }

    fn device_on_endpoint(ep: u32) -> Device {
        Device::Matter {
            node_id: Some("1234".into()),
            group: None,
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
            [
                "read",
                "--node",
                "1234",
                "--cluster",
                "onoff",
                "--attribute",
                "on-off"
            ]
        );
    }

    #[test]
    fn get_rejects_malformed_selector() {
        assert!(MatterAdapter.get(&device(), "on-off").is_none());
        assert!(MatterAdapter
            .get(&device(), "1/onoff/on-off/extra")
            .is_none());
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

    #[test]
    fn invoke_injects_node_and_passes_args_through() {
        let extra: Vec<String> = vec!["--kelvin".into(), "2700".into()];
        let inv = MatterAdapter
            .invoke(&device(), "color-temp", &extra)
            .unwrap();
        assert_eq!(inv.bin, "mat");
        assert_eq!(
            args(&inv),
            ["color-temp", "--node", "1234", "--kelvin", "2700"]
        );
    }

    #[test]
    fn invoke_with_endpoint_injects_endpoint_flag() {
        let extra: Vec<String> = vec!["--mireds".into(), "370".into()];
        let inv = MatterAdapter
            .invoke(&device_on_endpoint(2), "color-temp", &extra)
            .unwrap();
        assert_eq!(
            args(&inv),
            [
                "color-temp",
                "--node",
                "1234",
                "--endpoint",
                "2",
                "--mireds",
                "370"
            ]
        );
    }

    fn group_device() -> Device {
        Device::Matter {
            node_id: None,
            group: Some("desk_room_lights".into()),
            endpoint: None,
        }
    }

    #[test]
    fn group_power_on_builds_mat_group_invoke() {
        let inv = MatterAdapter.power(&group_device(), true).unwrap();
        assert_eq!(inv.bin, "mat");
        assert_eq!(
            args(&inv),
            [
                "group",
                "invoke",
                "--group",
                "desk_room_lights",
                "--cluster",
                "onoff",
                "--command",
                "on"
            ]
        );
    }

    #[test]
    fn group_power_off_builds_mat_group_invoke() {
        let inv = MatterAdapter.power(&group_device(), false).unwrap();
        assert_eq!(
            args(&inv),
            [
                "group",
                "invoke",
                "--group",
                "desk_room_lights",
                "--cluster",
                "onoff",
                "--command",
                "off"
            ]
        );
    }

    #[test]
    fn group_power_with_endpoint_passes_flag() {
        let dev = Device::Matter {
            node_id: None,
            group: Some("desk_room_lights".into()),
            endpoint: Some(2),
        };
        let inv = MatterAdapter.power(&dev, true).unwrap();
        assert_eq!(
            args(&inv),
            [
                "group",
                "invoke",
                "--group",
                "desk_room_lights",
                "--cluster",
                "onoff",
                "--command",
                "on",
                "--endpoint",
                "2"
            ]
        );
    }

    #[test]
    fn group_invoke_shortcut_injects_group_and_passes_args() {
        let extra: Vec<String> = vec!["--kelvin".into(), "2700".into()];
        let inv = MatterAdapter
            .invoke(&group_device(), "color-temp", &extra)
            .unwrap();
        assert_eq!(
            args(&inv),
            [
                "group",
                "color-temp",
                "--group",
                "desk_room_lights",
                "--kelvin",
                "2700"
            ]
        );
    }

    #[test]
    fn group_invoke_arbitrary_passes_through() {
        let extra: Vec<String> = vec![
            "--cluster".into(),
            "onoff".into(),
            "--command".into(),
            "on".into(),
        ];
        let inv = MatterAdapter
            .invoke(&group_device(), "invoke", &extra)
            .unwrap();
        assert_eq!(
            args(&inv),
            [
                "group",
                "invoke",
                "--group",
                "desk_room_lights",
                "--cluster",
                "onoff",
                "--command",
                "on"
            ]
        );
    }

    #[test]
    fn group_get_set_describe_are_unsupported() {
        assert!(MatterAdapter.get(&group_device(), "onoff/on-off").is_none());
        assert!(MatterAdapter
            .set(&group_device(), "onoff/on-off", "1")
            .is_none());
        assert!(MatterAdapter.describe(&group_device()).is_none());
    }
}
