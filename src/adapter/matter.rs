//! Matter アダプタ。実体は `mat`（https://github.com/nogu3/mat）のサブプロセス呼び出し。
//!
//! mat は node_id ベースで動く（commission 済みであることが前提。未 commission は
//! mat が exit 11 で落とし、casa はそれをそのまま伝播する）。
//!
//! casa の `get`/`set` はプロパティ指定が 1 引数なので、Matter では
//! `<cluster>/<attribute>`（chip-tool 表記、例: `onoff/on-off`）の形で受け取り、
//! ここで分解して `mat read/write <node_id> <endpoint> <cluster> <attribute>` を組む。
//!
//! ON/OFF は属性 write ではなく OnOff クラスタのコマンド invoke だが、その
//! 非対称性は mat 側のショートカット（`mat on` / `mat off`）が吸収している。

use super::{Adapter, Invocation};
use crate::config::Device;
use crate::error::{CasaError, ErrorKind};

const BIN: &str = "mat";

pub struct MatterAdapter;

/// デバイス定義から (node_id, endpoint) を取り出す。dispatch は `adapter_for` が
/// variant で行うので、他 variant が来ることはない。
fn address(device: &Device) -> Result<(u64, u16), CasaError> {
    match device {
        Device::Matter { node_id, endpoint } => Ok((*node_id, *endpoint)),
        other => Err(CasaError::new(
            ErrorKind::ProtocolUnsupported,
            format!(
                "matter adapter received a \"{}\" device (dispatch bug)",
                other.protocol()
            ),
        )),
    }
}

/// `<cluster>/<attribute>` を分解する。
fn split_property(property: &str) -> Result<(&str, &str), CasaError> {
    match property.split_once('/') {
        Some((cluster, attribute)) if !cluster.is_empty() && !attribute.is_empty() => {
            Ok((cluster, attribute))
        }
        _ => Err(CasaError::new(
            ErrorKind::InvalidArgument,
            format!(
                "matter property must be \"<cluster>/<attribute>\" in chip-tool form \
                 (e.g. \"onoff/on-off\"), got \"{property}\""
            ),
        )),
    }
}

fn invocation(parts: &[&str]) -> Invocation {
    Invocation {
        bin: BIN,
        args: parts.iter().map(|s| s.to_string()).collect(),
    }
}

impl Adapter for MatterAdapter {
    fn protocol(&self) -> &'static str {
        "matter"
    }

    fn get(&self, device: &Device, property: &str) -> Result<Invocation, CasaError> {
        let (node_id, endpoint) = address(device)?;
        let (cluster, attribute) = split_property(property)?;
        Ok(invocation(&[
            "read",
            &node_id.to_string(),
            &endpoint.to_string(),
            cluster,
            attribute,
        ]))
    }

    fn set(&self, device: &Device, property: &str, value: &str) -> Result<Invocation, CasaError> {
        let (node_id, endpoint) = address(device)?;
        let (cluster, attribute) = split_property(property)?;
        Ok(invocation(&[
            "write",
            &node_id.to_string(),
            &endpoint.to_string(),
            cluster,
            attribute,
            value,
        ]))
    }

    fn describe(&self, device: &Device) -> Result<Invocation, CasaError> {
        let (node_id, _) = address(device)?;
        Ok(invocation(&["describe", &node_id.to_string()]))
    }

    fn power(&self, device: &Device, on: bool) -> Result<Invocation, CasaError> {
        let (node_id, endpoint) = address(device)?;
        let command = if on { "on" } else { "off" };
        Ok(invocation(&[
            command,
            &node_id.to_string(),
            "--endpoint",
            &endpoint.to_string(),
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device::Matter {
            node_id: 5,
            endpoint: 1,
        }
    }

    fn args(inv: &Invocation) -> Vec<&str> {
        inv.args.iter().map(String::as_str).collect()
    }

    #[test]
    fn get_builds_mat_read_args() {
        let inv = MatterAdapter.get(&device(), "onoff/on-off").unwrap();
        assert_eq!(inv.bin, "mat");
        assert_eq!(args(&inv), ["read", "5", "1", "onoff", "on-off"]);
    }

    #[test]
    fn set_builds_mat_write_args() {
        let inv = MatterAdapter
            .set(&device(), "levelcontrol/on-level", "128")
            .unwrap();
        assert_eq!(
            args(&inv),
            ["write", "5", "1", "levelcontrol", "on-level", "128"]
        );
    }

    #[test]
    fn describe_builds_mat_describe_args() {
        let inv = MatterAdapter.describe(&device()).unwrap();
        assert_eq!(args(&inv), ["describe", "5"]);
    }

    #[test]
    fn power_maps_to_mat_on_off_shortcuts() {
        let on = MatterAdapter.power(&device(), true).unwrap();
        assert_eq!(args(&on), ["on", "5", "--endpoint", "1"]);

        let off = MatterAdapter
            .power(
                &Device::Matter {
                    node_id: 7,
                    endpoint: 2,
                },
                false,
            )
            .unwrap();
        assert_eq!(args(&off), ["off", "7", "--endpoint", "2"]);
    }

    #[test]
    fn property_without_slash_is_invalid_argument() {
        let err = MatterAdapter.get(&device(), "on-off").unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidArgument);
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn property_with_empty_part_is_invalid_argument() {
        let err = MatterAdapter.set(&device(), "onoff/", "1").unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidArgument);
    }
}
