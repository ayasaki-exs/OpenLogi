//! Discovery of standalone raw-HID devices.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use openlogi_core::device::{Capabilities, DeviceKind, RawDeviceAddress, StandaloneDevice};
use openlogi_device_registry::darmoshark::{DARMOSHARK_DRIVER_ID, find_darmoshark};
use openlogi_device_registry::g600::{G600_DRIVER_ID, G600_NAME, matches_g600};
use openlogi_device_registry::headset::{GAMING_HEADSET_DRIVER_ID, find_gaming_headset};
use openlogi_device_registry::litra::find_litra;

use super::InventoryError;
use crate::backend::HidBackend;
use crate::write::litra_capabilities;

/// Enumerate recognized standalone devices without probing them as HID++.
///
/// The returned descriptors are intentionally separate from receiver
/// inventories. A raw device has no HID++ pairing slot and must be routed by
/// its full HID identity tuple.
pub async fn enumerate_standalone(
    backend: &dyn HidBackend,
) -> Result<Vec<StandaloneDevice>, InventoryError> {
    let devices = backend.enumerate().await?;
    let devices: Vec<_> = devices
        .into_iter()
        .filter_map(|device| standalone_device(&device))
        .collect();
    validate_no_ambiguous_nodes(&devices)?;
    let mut devices = devices;
    for device in &mut devices {
        if device.driver_id != DARMOSHARK_DRIVER_ID {
            continue;
        }
        // A sleeping mouse would otherwise hold the scan for the driver's full
        // retry budget. Two seconds is enough for an awake device (handshake +
        // version) and keeps a cold scan bounded; the fields stay `None` when
        // it does not answer.
        match tokio::time::timeout(
            Duration::from_secs(2),
            crate::write::darmoshark::read_status(backend, &device.route()),
        )
        .await
        {
            Ok(Ok(status)) => {
                device.battery = status.battery;
                device.firmware = status.firmware;
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "Darmoshark status read skipped");
            }
            Err(_) => {
                tracing::debug!("Darmoshark status read timed out");
            }
        }
    }
    Ok(devices)
}

/// Classifies one enumerated HID node against every standalone driver family
/// OpenLogi recognizes, in precedence order. `None` when no family claims it.
pub(super) fn standalone_device(device: &crate::backend::NodeInfo) -> Option<StandaloneDevice> {
    if let Some(descriptor) = find_litra(
        device.vendor_id,
        device.product_id,
        device.usage_page,
        device.usage_id,
    ) {
        return Some(StandaloneDevice {
            address: RawDeviceAddress {
                vendor_id: device.vendor_id,
                product_id: device.product_id,
                usage_page: device.usage_page,
                usage_id: device.usage_id,
                identity: device.identity(),
            },
            display_name: device.name.clone(),
            manufacturer: device.manufacturer.clone(),
            serial_number: device.serial_number.clone(),
            unit_id: [0; 4],
            kind: DeviceKind::Light,
            online: true,
            capabilities: None,
            light_capabilities: Some(litra_capabilities(descriptor.model)),
            driver_id: descriptor.driver_id.to_owned(),
            registry_model_id: Some(descriptor.registry_model_id.to_owned()),
            battery: None,
            firmware: None,
        });
    }

    if let Some(descriptor) = find_gaming_headset(
        device.vendor_id,
        device.product_id,
        device.usage_page,
        device.usage_id,
    ) {
        return Some(StandaloneDevice {
            address: RawDeviceAddress {
                vendor_id: device.vendor_id,
                product_id: device.product_id,
                usage_page: device.usage_page,
                usage_id: device.usage_id,
                identity: device.identity(),
            },
            display_name: descriptor.name.to_owned(),
            manufacturer: device.manufacturer.clone(),
            serial_number: device.serial_number.clone(),
            unit_id: [0; 4],
            kind: DeviceKind::Headset,
            online: true,
            capabilities: None,
            light_capabilities: None,
            driver_id: GAMING_HEADSET_DRIVER_ID.to_owned(),
            registry_model_id: Some(descriptor.registry_model_id.to_owned()),
            battery: None,
            firmware: None,
        });
    }

    if let Some(standalone) = darmoshark_standalone(device) {
        return Some(standalone);
    }

    if matches_g600(
        device.vendor_id,
        device.product_id,
        device.usage_page,
        device.usage_id,
    ) {
        return Some(StandaloneDevice {
            address: RawDeviceAddress {
                vendor_id: device.vendor_id,
                product_id: device.product_id,
                usage_page: device.usage_page,
                usage_id: device.usage_id,
                identity: device.identity(),
            },
            display_name: G600_NAME.to_owned(),
            manufacturer: device.manufacturer.clone(),
            serial_number: device.serial_number.clone(),
            unit_id: [0; 4],
            kind: DeviceKind::Mouse,
            online: true,
            // Measured, not unknown: `None` would make clients presume a
            // mouse's HID++ button and pointer controls this device lacks.
            capabilities: Some(Capabilities::default()),
            light_capabilities: None,
            driver_id: G600_DRIVER_ID.to_owned(),
            registry_model_id: None,
            battery: None,
            firmware: None,
        });
    }

    None
}

/// Builds the inventory record for a Darmoshark configuration node. Returns
/// `None` for the product's plain HID collections and for any other vendor.
fn darmoshark_standalone(device: &crate::backend::NodeInfo) -> Option<StandaloneDevice> {
    let descriptor = find_darmoshark(
        device.vendor_id,
        device.product_id,
        device.usage_page,
        device.usage_id,
    )?;
    Some(StandaloneDevice {
        address: RawDeviceAddress {
            vendor_id: device.vendor_id,
            product_id: device.product_id,
            usage_page: device.usage_page,
            usage_id: device.usage_id,
            identity: device.identity(),
        },
        display_name: descriptor.name.to_owned(),
        manufacturer: device.manufacturer.clone(),
        serial_number: device.serial_number.clone(),
        unit_id: [0; 4],
        kind: DeviceKind::Mouse,
        online: true,
        // Driver-advertised capabilities, not HID++ feature bits: the raw
        // driver implements DPI and report-rate reads/writes. Only claim what
        // it can serve — buttons and lighting stay off, so `None` (which would
        // presume a mouse's HID++ controls) is not an option here.
        capabilities: Some(Capabilities {
            pointer: true,
            report_rate: true,
            ..Capabilities::default()
        }),
        light_capabilities: None,
        driver_id: DARMOSHARK_DRIVER_ID.to_owned(),
        registry_model_id: None,
        battery: None,
        firmware: None,
    })
}

/// Reject multiple nodes that the route cannot distinguish safely.
///
/// A serial-bearing pair is distinguishable even when two identical lights
/// share the same VID/PID/usage tuple. An OS-node identity (`id:…`) is only a
/// transient re-find hint, so two such nodes with the same tuple are
/// indistinguishable and must not be exposed as independently selectable
/// devices.
fn validate_no_ambiguous_nodes(devices: &[StandaloneDevice]) -> Result<(), InventoryError> {
    let mut groups: HashMap<(u16, u16, u16, u16), Vec<&StandaloneDevice>> = HashMap::new();
    for device in devices {
        let address = &device.address;
        groups
            .entry((
                address.vendor_id,
                address.product_id,
                address.usage_page,
                address.usage_id,
            ))
            .or_default()
            .push(device);
    }
    if groups.values().any(|group| {
        if group.len() < 2 {
            return false;
        }
        let identities: HashSet<&str> = group
            .iter()
            .map(|device| device.address.identity.as_str())
            .collect();
        identities.len() != group.len()
            || group
                .iter()
                .any(|device| !device.address.identity.starts_with("serial:"))
    }) {
        return Err(InventoryError::AmbiguousRawDevice);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use openlogi_core::device::{Capabilities, DeviceKind, RawDeviceAddress, StandaloneDevice};
    use openlogi_device_registry::darmoshark::DARMOSHARK_DRIVER_ID;

    use crate::backend::{NodeId, NodeInfo};
    use crate::write::matches_litra;

    use super::{InventoryError, standalone_device, validate_no_ambiguous_nodes};

    #[test]
    fn glow_fixture_matches_standalone_driver() {
        assert!(matches_litra(0x046d, 0xc900, 0xff43, 0x0202));
    }

    fn g600_node(usage_page: u16, usage_id: u16) -> NodeInfo {
        NodeInfo {
            id: NodeId::from("g600-node".to_string()),
            vendor_id: 0x046d,
            product_id: 0xc24a,
            usage_page,
            usage_id,
            name: "Logitech Gaming Mouse G600".into(),
            manufacturer: Some("Logitech".into()),
            serial_number: None,
        }
    }

    #[test]
    fn g600_vendor_collection_is_an_identity_only_mouse() {
        let device = standalone_device(&g600_node(0xff80, 0x0080)).expect("G600 is recognized");

        assert_eq!(device.kind, DeviceKind::Mouse);
        assert_eq!(device.display_name, "G600 Gaming Mouse");
        assert_eq!(device.driver_id, "logitech-g600");
        assert_eq!(device.address.identity, "id:g600-node");
        assert_eq!(device.capabilities, Some(Capabilities::default()));
        assert!(device.light_capabilities.is_none());
    }

    #[test]
    fn g600_mouse_and_keyboard_collections_are_ignored() {
        assert!(standalone_device(&g600_node(0x0001, 0x0002)).is_none());
        assert!(standalone_device(&g600_node(0x0001, 0x0006)).is_none());
    }

    fn darmoshark_node(usage_page: u16, usage_id: u16) -> NodeInfo {
        NodeInfo {
            id: NodeId::from("darmoshark-node".to_string()),
            vendor_id: 0x1915,
            product_id: 0x0723,
            usage_page,
            usage_id,
            name: "MOJI 1K NRF Receiver".into(),
            manufacturer: Some("Darmoshark".into()),
            serial_number: None,
        }
    }

    #[test]
    fn darmoshark_configuration_collection_advertises_the_raw_driver() {
        let device =
            standalone_device(&darmoshark_node(0xff0a, 0x0001)).expect("config channel recognized");

        assert_eq!(device.kind, DeviceKind::Mouse);
        assert_eq!(device.display_name, "Darmoshark M3S Pro (2.4 GHz)");
        assert_eq!(device.driver_id, DARMOSHARK_DRIVER_ID);
        assert_eq!(device.address.identity, "id:darmoshark-node");
        assert_eq!(
            device.capabilities,
            Some(Capabilities {
                pointer: true,
                report_rate: true,
                ..Capabilities::default()
            })
        );
        assert!(device.light_capabilities.is_none());
    }

    #[test]
    fn darmoshark_plain_hid_collections_are_ignored() {
        assert!(standalone_device(&darmoshark_node(0x0001, 0x0002)).is_none());
        assert!(standalone_device(&darmoshark_node(0x0001, 0x0006)).is_none());
        assert!(standalone_device(&darmoshark_node(0x000c, 0x0001)).is_none());
    }

    fn raw(identity: &str) -> StandaloneDevice {
        StandaloneDevice {
            address: RawDeviceAddress {
                vendor_id: 0x046d,
                product_id: 0xc900,
                usage_page: 0xff43,
                usage_id: 0x0202,
                identity: identity.into(),
            },
            display_name: "Litra Glow".into(),
            manufacturer: Some("Logi".into()),
            serial_number: identity.strip_prefix("serial:").map(str::to_string),
            unit_id: [0; 4],
            kind: DeviceKind::Light,
            online: true,
            capabilities: None,
            light_capabilities: None,
            driver_id: "litra".into(),
            registry_model_id: None,
            battery: None,
            firmware: None,
        }
    }

    #[test]
    fn duplicate_transient_nodes_are_rejected() {
        let devices = vec![raw("id:old"), raw("id:new")];
        assert!(matches!(
            validate_no_ambiguous_nodes(&devices),
            Err(InventoryError::AmbiguousRawDevice)
        ));
    }

    #[test]
    fn distinct_serials_can_share_the_same_hid_tuple() {
        let devices = vec![raw("serial:one"), raw("serial:two")];
        let validation = validate_no_ambiguous_nodes(&devices);
        assert!(
            validation.is_ok(),
            "serial-backed nodes stay distinguishable: {validation:?}"
        );
    }
}
