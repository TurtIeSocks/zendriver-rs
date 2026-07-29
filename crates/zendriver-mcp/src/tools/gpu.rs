//! `browser_gpu_devices` — browse the measured GPU device catalogue.
//!
//! An agent could already *use* a catalogued identity, because a device's whole
//! contribution is a renderer string and `browser_open.persona` carries that.
//! What it could not do was find one: naming a GPU meant already knowing the
//! exact string a driver reports, which is not knowledge an agent has.

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zendriver::stealth::{GpuDevice, Platform};

/// Which platform's devices to list.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GpuPlatform {
    /// Direct3D 11 devices, feature level 11 and above.
    Win32,
    /// Apple silicon under ANGLE's Metal backend.
    MacIntel,
}

impl From<GpuPlatform> for Platform {
    fn from(p: GpuPlatform) -> Self {
        match p {
            GpuPlatform::Win32 => Platform::Win32,
            GpuPlatform::MacIntel => Platform::MacIntel,
        }
    }
}

/// Input for `browser_gpu_devices`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GpuDevicesInput {
    /// Case-insensitive substring of the model name, e.g. `"rtx 40"` or
    /// `"iris"`. Omit to list everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Restrict to one platform's devices. Omit for all.
    ///
    /// There is no Linux option on purpose: ANGLE's Vulkan backend reads its
    /// limits off the physical device, so there is no shared capability tier
    /// for a Linux identity to layer over and nothing is catalogued for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<GpuPlatform>,
    /// Cap the number of devices returned. Defaults to 25; the catalogue holds
    /// several hundred, and an agent rarely wants all of them at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// How common a device is, and how to claim it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GpuDeviceOut {
    /// Driver-reported model text, e.g. `NVIDIA GeForce RTX 4090`. This is the
    /// exact string `browser_gpu_devices`' own `query` matches on.
    pub model: String,
    /// ANGLE's vendor token, e.g. `NVIDIA`.
    pub vendor: String,
    /// PCI device id as hex, or absent on Apple silicon, which exposes none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Share of the browser population reporting this device, `0.0..=1.0`.
    /// Measured from the fingerprint corpus, not from a hardware survey.
    pub share: f64,
    /// The full `UNMASKED_RENDERER_WEBGL` string a page reads for this device.
    ///
    /// **This is the field to use.** Put it in a persona's
    /// `webgl.unmasked_renderer` and pass that to `browser_open` — it selects
    /// the capability tier, the WebGPU adapter and the vendor on its own.
    pub renderer: String,
}

/// Output for `browser_gpu_devices`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GpuDevicesOutput {
    /// Matching devices, most common first.
    pub devices: Vec<GpuDeviceOut>,
    /// How many matched before `limit` was applied, so a caller can tell a
    /// short list from a truncated one.
    pub total_matched: usize,
}

const DEFAULT_LIMIT: usize = 25;

/// List catalogued GPU devices, most common first.
///
/// Takes no `SessionState`: the catalogue is a compiled-in table, so this
/// answers without a browser.
pub fn devices(input: GpuDevicesInput) -> Result<GpuDevicesOutput, ErrorData> {
    let hits = GpuDevice::search(input.query.as_deref(), input.platform.map(Into::into));
    let total_matched = hits.len();
    let limit = input.limit.unwrap_or(DEFAULT_LIMIT);
    Ok(GpuDevicesOutput {
        devices: hits
            .into_iter()
            .take(limit)
            .map(|d| GpuDeviceOut {
                model: d.model().to_string(),
                vendor: d.vendor().to_string(),
                device_id: d.device_id().map(|id| format!("{id:#06x}")),
                share: d.share(),
                renderer: d.renderer(),
            })
            .collect(),
        total_matched,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        query: Option<&str>,
        platform: Option<GpuPlatform>,
        limit: Option<usize>,
    ) -> GpuDevicesInput {
        GpuDevicesInput {
            query: query.map(str::to_string),
            platform,
            limit,
        }
    }

    #[test]
    fn lists_the_commonest_devices_first() {
        let out = devices(input(None, Some(GpuPlatform::Win32), None)).unwrap();
        assert_eq!(out.devices.len(), DEFAULT_LIMIT);
        assert!(
            out.total_matched > DEFAULT_LIMIT,
            "a truncated list must say so"
        );
        assert!(out.devices[0].share >= out.devices[1].share);
    }

    #[test]
    fn every_device_carries_a_usable_renderer_string() {
        // The renderer is the field an agent actually acts on, so an entry
        // without one would be advice it cannot follow.
        let out = devices(input(Some("rtx"), None, Some(100))).unwrap();
        assert!(!out.devices.is_empty());
        for d in &out.devices {
            assert!(d.renderer.starts_with("ANGLE ("), "{}", d.renderer);
            assert!(d.renderer.contains(&d.model), "{}", d.renderer);
        }
    }

    #[test]
    fn apple_devices_report_no_pci_id() {
        let out = devices(input(
            Some("apple m"),
            Some(GpuPlatform::MacIntel),
            Some(50),
        ))
        .unwrap();
        assert!(!out.devices.is_empty());
        assert!(out.devices.iter().all(|d| d.device_id.is_none()));
    }

    #[test]
    fn a_query_matching_nothing_returns_an_empty_list_rather_than_an_error() {
        let out = devices(input(Some("definitely not a gpu"), None, None)).unwrap();
        assert!(out.devices.is_empty());
        assert_eq!(out.total_matched, 0);
    }
}
