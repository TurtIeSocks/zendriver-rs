//! Generator for the GPU device catalogue.
//!
//! Emits `crates/zendriver-stealth/src/gpu/catalogue.rs` from two pinned
//! sources: driver-reported model names out of the fingerprint corpus, and PCI
//! device IDs out of `pci.ids`. Nothing here invents a value.
//!
//! The catalogue supplies *identity* only — a renderer string, a device ID, an
//! architecture token. Every capability number still comes from the measured
//! tier tables that `gpu-tier-gen` emits, which is what keeps this generator
//! from being able to fabricate a capability at all.

pub mod sources;

/// One `<vendor id>:<device id>` pair and the name `pci.ids` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PciDevice {
    pub vendor_id: u32,
    pub device_id: u32,
    pub name: String,
}

/// Parse the `pci.ids` format.
///
/// The shape is three levels deep, distinguished only by leading tabs:
///
/// ```text
/// 10de  NVIDIA Corporation            <- vendor
/// \t2684  AD102 [GeForce RTX 4090]    <- device
/// \t\t1043 87b3  TUF Gaming           <- subsystem
/// ```
///
/// Only the middle level carries a device ID. Subsystem lines are a
/// `(subvendor, subdevice)` pair, so reading one as a device invents a card
/// that does not exist.
///
/// The file then ends with device-*class* sections, whose headings are
/// unindented like vendors and whose subclass lines are indented exactly like
/// devices:
///
/// ```text
/// C 03  Display controller
/// \t00  VGA compatible controller
/// ```
///
/// `C 03` is not hex, so the current vendor is **cleared** rather than left
/// alone. Leaving it alone would file every subclass in the file under
/// whichever vendor happened to come last.
#[must_use]
pub fn parse_pci_ids(raw: &str) -> Vec<PciDevice> {
    let mut out = Vec::new();
    let mut vendor_id: Option<u32> = None;

    for line in raw.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }

        if let Some(rest) = line.strip_prefix('\t') {
            // Two tabs: a subsystem pair, which has no device ID of its own.
            if rest.starts_with('\t') {
                continue;
            }
            let (Some(vendor), Some((id, name))) = (vendor_id, rest.split_once("  ")) else {
                continue;
            };
            if let Ok(device_id) = u32::from_str_radix(id.trim(), 16) {
                out.push(PciDevice {
                    vendor_id: vendor,
                    device_id,
                    name: name.trim().to_string(),
                });
            }
            continue;
        }

        // Unindented: a vendor heading, or a class heading that is not one.
        vendor_id = line
            .split_once("  ")
            .and_then(|(id, _)| u32::from_str_radix(id.trim(), 16).ok());
    }
    out
}

#[cfg(test)]
mod pci_tests {
    use super::*;

    const MINI: &str = include_str!("../tests/fixtures/pci-mini.ids");

    #[test]
    fn parses_vendor_and_device_lines() {
        let devices = parse_pci_ids(MINI);
        let rtx4090 = devices
            .iter()
            .find(|d| d.device_id == 0x2684)
            .expect("RTX 4090 device line");
        assert_eq!(rtx4090.vendor_id, 0x10de);
        assert_eq!(rtx4090.name, "AD102 [GeForce RTX 4090]");
    }

    #[test]
    fn keeps_each_device_under_its_own_vendor() {
        let devices = parse_pci_ids(MINI);
        // The bug this guards: carrying the previous vendor across a new
        // unindented line would file Raphael under NVIDIA.
        let raphael = devices.iter().find(|d| d.device_id == 0x164e).unwrap();
        assert_eq!(raphael.vendor_id, 0x1002);
        let uhd = devices.iter().find(|d| d.device_id == 0x3e92).unwrap();
        assert_eq!(uhd.vendor_id, 0x8086);
    }

    #[test]
    fn skips_subsystem_lines() {
        let devices = parse_pci_ids(MINI);
        // `\t\t1043 87b3  TUF Gaming` is a (subvendor, subdevice) pair, not a
        // device ID. Read as one it would invent a device 0x1043.
        assert!(
            !devices.iter().any(|d| d.name.contains("TUF Gaming")),
            "subsystem lines must not become devices"
        );
        assert!(!devices.iter().any(|d| d.device_id == 0x1043));
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let devices = parse_pci_ids(MINI);
        assert!(!devices.iter().any(|d| d.name.starts_with('#')));
        assert!(!devices.iter().any(|d| d.name.is_empty()));
    }

    #[test]
    fn device_class_sections_contribute_nothing() {
        // The real file ends with `C xx  <class>` headings whose subclass lines
        // are indented exactly like device lines. A parser that keeps the last
        // good vendor when a heading fails to parse files every one of them
        // under Intel, inventing devices 0x00, 0x01 and 0x03 that no card has.
        let devices = parse_pci_ids(MINI);
        for name in [
            "Non-VGA unclassified device",
            "VGA compatible unclassified device",
            "VGA compatible controller",
        ] {
            assert!(
                !devices.iter().any(|d| d.name == name),
                "class subclass {name:?} leaked into the device list"
            );
        }
        assert_eq!(
            devices.len(),
            5,
            "expected exactly the five real device lines, got {:?}",
            devices.iter().map(|d| &d.name).collect::<Vec<_>>()
        );
    }
}
