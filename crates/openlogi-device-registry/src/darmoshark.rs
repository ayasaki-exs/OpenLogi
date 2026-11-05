//! Darmoshark (MOJI) raw-HID identities.
//!
//! Darmoshark's mice expose their vendor configuration channel on usage page
//! `0xff0a`, usage `0x0001`, as a 64-byte non-numbered input/output report
//! pair. The wired mouse and its 2.4 GHz receiver publish the same collection,
//! so one driver family addresses both.
//!
//! These identities are deliberately separate from Logitech's: the vendor id
//! is `0x1915`, and the config collection is not a HID++ collection. The
//! generic HID++ candidate filter therefore never sees these nodes, and the
//! standalone-device inventory is the one place that claims them.

/// Darmoshark's USB vendor id (shared by the mice and their receivers).
pub const DARMOSHARK_VENDOR_ID: u16 = 0x1915;

/// Stable driver-family identifier carried by standalone Darmoshark inventory.
pub const DARMOSHARK_DRIVER_ID: &str = "darmoshark";

/// HID usage page of the vendor configuration channel.
pub const DARMOSHARK_CONFIG_USAGE_PAGE: u16 = 0xff0a;

/// HID usage id of the vendor configuration channel.
pub const DARMOSHARK_CONFIG_USAGE_ID: u16 = 0x0001;

/// A known Darmoshark raw-HID identity.
///
/// Matching uses the complete `(vendor, product, usage page, usage id)` tuple:
/// the same product publishes several plain HID collections (pointer,
/// keyboard, consumer) and only the vendor collection carries configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DarmosharkDescriptor {
    /// USB vendor id.
    pub vendor_id: u16,
    /// USB product id.
    pub product_id: u16,
    /// HID usage page of the configuration collection.
    pub usage_page: u16,
    /// HID usage id of the configuration collection.
    pub usage_id: u16,
    /// Marketed product name shown when the OS reports no better one.
    pub name: &'static str,
}

impl DarmosharkDescriptor {
    const fn new(product_id: u16, name: &'static str) -> Self {
        Self {
            vendor_id: DARMOSHARK_VENDOR_ID,
            product_id,
            usage_page: DARMOSHARK_CONFIG_USAGE_PAGE,
            usage_id: DARMOSHARK_CONFIG_USAGE_ID,
            name,
        }
    }
}

/// All Darmoshark raw-HID identities OpenLogi recognizes.
///
/// `1915:073c` is the wired M3S Pro. `1915:0723` is the "1K NRF Receiver":
/// when the mouse is wireless, the receiver's configuration collection is the
/// one that reaches the mouse, so it is presented as the same product over
/// 2.4 GHz rather than as a receiver card of its own.
pub const DARMOSHARK_DEVICES: &[DarmosharkDescriptor] = &[
    DarmosharkDescriptor::new(0x073c, "Darmoshark M3S Pro"),
    DarmosharkDescriptor::new(0x0723, "Darmoshark M3S Pro (2.4 GHz)"),
];

/// Finds a Darmoshark descriptor by its complete raw-HID identity.
#[must_use]
pub fn find_darmoshark(
    vendor_id: u16,
    product_id: u16,
    usage_page: u16,
    usage_id: u16,
) -> Option<&'static DarmosharkDescriptor> {
    DARMOSHARK_DEVICES.iter().find(|device| {
        device.vendor_id == vendor_id
            && device.product_id == product_id
            && device.usage_page == usage_page
            && device.usage_id == usage_id
    })
}

/// Whether an HID node is a Darmoshark configuration channel.
#[must_use]
pub fn matches_darmoshark(vendor_id: u16, product_id: u16, usage_page: u16, usage_id: u16) -> bool {
    find_darmoshark(vendor_id, product_id, usage_page, usage_id).is_some()
}

#[cfg(test)]
mod tests {
    use super::{
        DARMOSHARK_CONFIG_USAGE_ID, DARMOSHARK_CONFIG_USAGE_PAGE, DARMOSHARK_DEVICES,
        DARMOSHARK_VENDOR_ID, find_darmoshark, matches_darmoshark,
    };

    #[test]
    fn both_transports_match_the_configuration_collection() {
        for (product_id, name) in [
            (0x073c, "Darmoshark M3S Pro"),
            (0x0723, "Darmoshark M3S Pro (2.4 GHz)"),
        ] {
            let descriptor = find_darmoshark(
                DARMOSHARK_VENDOR_ID,
                product_id,
                DARMOSHARK_CONFIG_USAGE_PAGE,
                DARMOSHARK_CONFIG_USAGE_ID,
            )
            .expect("registered Darmoshark identity");
            assert_eq!(descriptor.name, name);
        }
    }

    #[test]
    fn a_plain_hid_collection_is_not_the_configuration_channel() {
        assert!(!matches_darmoshark(
            DARMOSHARK_VENDOR_ID,
            0x0723,
            0x0001,
            0x0002
        ));
        assert!(!matches_darmoshark(
            DARMOSHARK_VENDOR_ID,
            0x0723,
            0x000c,
            0x0001
        ));
    }

    #[test]
    fn another_vendor_is_never_claimed() {
        assert!(!matches_darmoshark(
            0x046d,
            0x0723,
            DARMOSHARK_CONFIG_USAGE_PAGE,
            DARMOSHARK_CONFIG_USAGE_ID
        ));
    }

    #[test]
    fn identities_are_unique() {
        for (index, device) in DARMOSHARK_DEVICES.iter().enumerate() {
            assert!(
                DARMOSHARK_DEVICES[..index].iter().all(|other| {
                    (
                        other.vendor_id,
                        other.product_id,
                        other.usage_page,
                        other.usage_id,
                    ) != (
                        device.vendor_id,
                        device.product_id,
                        device.usage_page,
                        device.usage_id,
                    )
                }),
                "duplicate Darmoshark identity {:04x}:{:04x}/{:04x}:{:04x}",
                device.vendor_id,
                device.product_id,
                device.usage_page,
                device.usage_id
            );
        }
    }
}
