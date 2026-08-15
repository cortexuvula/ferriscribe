//! Audio device enumeration and selection.
//!
//! Discovers system input (microphone) and output (speaker) devices via cpal,
//! reports their supported sample rates and channel counts, and provides
//! lookup by name or system-default.

use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};

use crate::{AudioError, AudioResult};

/// Describes an audio device discovered on the system.
///
/// Produced by [`list_input_devices`] and [`list_output_devices`]. Serializable
/// so the frontend can display device lists via Tauri commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDevice {
    /// Human-readable device name as reported by the OS.
    pub name: String,
    /// `true` if the device can capture audio (microphone / line-in).
    pub is_input: bool,
    /// `true` if this is the system's default device for its direction.
    pub is_default: bool,
    /// Supported sample rates filtered to common values (16k, 22.05k, 44.1k, 48k).
    pub sample_rates: Vec<u32>,
    /// Supported channel counts (e.g., `[1, 2]` for mono + stereo).
    pub channels: Vec<u16>,
}

/// Sample rates we care about.
const PREFERRED_RATES: &[u32] = &[16_000, 22_050, 44_100, 48_000];

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Returns `(supported_rates, supported_channels)` for a device.
///
/// Tries input configs first, falls back to output configs. Rates are filtered
/// to `PREFERRED_RATES` (16k, 22.05k, 44.1k, 48k) and both lists are
/// deduplicated and sorted.
pub fn supported_configs(device: &cpal::Device) -> (Vec<u32>, Vec<u16>) {
    // Try input configs first, then output configs.
    let ranges: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_input_configs()
        .map(|i| i.collect())
        .unwrap_or_default();

    let ranges = if ranges.is_empty() {
        device
            .supported_output_configs()
            .map(|i| i.collect::<Vec<_>>())
            .unwrap_or_default()
    } else {
        ranges
    };

    let mut rates: Vec<u32> = PREFERRED_RATES
        .iter()
        .copied()
        .filter(|&r| {
            ranges
                .iter()
                .any(|range| range.min_sample_rate().0 <= r && r <= range.max_sample_rate().0)
        })
        .collect();
    rates.sort();
    rates.dedup();

    let mut channels: Vec<u16> = ranges.iter().map(|r| r.channels()).collect();
    channels.sort();
    channels.dedup();

    (rates, channels)
}

fn device_to_audio_device(
    device: &cpal::Device,
    is_input: bool,
    is_default: bool,
) -> Option<AudioDevice> {
    let name = device.name().ok()?;
    let (sample_rates, channels) = supported_configs(device);
    Some(AudioDevice {
        name,
        is_input,
        is_default,
        sample_rates,
        channels,
    })
}

// ──────────────────────────────────────────────────────────────────────────────
// Public API
// ──────────────────────────────────────────────────────────────────────────────

/// List all input (microphone) devices.
pub fn list_input_devices() -> AudioResult<Vec<AudioDevice>> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| d.name().ok());

    let devices = host
        .input_devices()
        .map_err(|e| AudioError::Device(e.to_string()))?;

    let mut result = Vec::new();
    for device in devices {
        let is_default = device
            .name()
            .ok()
            .map(|n| Some(n) == default_name)
            .unwrap_or(false);
        if let Some(ad) = device_to_audio_device(&device, true, is_default) {
            result.push(ad);
        }
    }
    Ok(result)
}

/// List all output (speaker/headphone) devices.
pub fn list_output_devices() -> AudioResult<Vec<AudioDevice>> {
    let host = cpal::default_host();
    let default_name = host.default_output_device().and_then(|d| d.name().ok());

    let devices = host
        .output_devices()
        .map_err(|e| AudioError::Device(e.to_string()))?;

    let mut result = Vec::new();
    for device in devices {
        let is_default = device
            .name()
            .ok()
            .map(|n| Some(n) == default_name)
            .unwrap_or(false);
        if let Some(ad) = device_to_audio_device(&device, false, is_default) {
            result.push(ad);
        }
    }
    Ok(result)
}

/// Obtain a `cpal::Device` for input.
/// If `name` is `None`, returns the system default; otherwise searches by name.
pub fn get_input_device(name: Option<&str>) -> AudioResult<cpal::Device> {
    let host = cpal::default_host();
    match name {
        None => host.default_input_device().ok_or(AudioError::NoInputDevice),
        Some(wanted) => {
            let mut devices = host
                .input_devices()
                .map_err(|e| AudioError::Device(e.to_string()))?;
            devices
                .find(|d| d.name().ok().as_deref() == Some(wanted))
                .ok_or(AudioError::NoInputDevice)
        }
    }
}

/// Obtain a `cpal::Device` for output.
/// If `name` is `None`, returns the system default; otherwise searches by name.
pub fn get_output_device(name: Option<&str>) -> AudioResult<cpal::Device> {
    let host = cpal::default_host();
    match name {
        None => host
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice),
        Some(wanted) => {
            let mut devices = host
                .output_devices()
                .map_err(|e| AudioError::Device(e.to_string()))?;
            devices
                .find(|d| d.name().ok().as_deref() == Some(wanted))
                .ok_or(AudioError::NoOutputDevice)
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// cpal device enumeration can block indefinitely on machines with busy
    /// or virtual audio hardware (the same reason Windows CI is excluded).
    /// Gated like the mDNS smoke tests (FERRISCRIBE_MDNS_TEST) so
    /// `cargo test --workspace --lib` stays runnable on dev machines.
    fn audio_gate() -> bool {
        if std::env::var("FERRISCRIBE_AUDIO_TEST").ok().as_deref() != Some("1") {
            eprintln!("skipping: set FERRISCRIBE_AUDIO_TEST=1 to enumerate audio devices");
            return false;
        }
        true
    }

    #[test]
    fn list_input_returns_vec() {
        if !audio_gate() {
            return;
        }
        // May be empty in a headless CI environment — just assert it doesn't panic.
        let result = list_input_devices();
        assert!(
            result.is_ok(),
            "list_input_devices returned Err: {:?}",
            result
        );
    }

    #[test]
    fn list_output_returns_vec() {
        if !audio_gate() {
            return;
        }
        let result = list_output_devices();
        assert!(
            result.is_ok(),
            "list_output_devices returned Err: {:?}",
            result
        );
    }

    #[test]
    fn get_input_device_none_succeeds_or_no_device() {
        if !audio_gate() {
            return;
        }
        match get_input_device(None) {
            Ok(_) => {}
            Err(AudioError::NoInputDevice) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn audio_device_serializes() {
        let dev = AudioDevice {
            name: "Test Mic".to_string(),
            is_input: true,
            is_default: false,
            sample_rates: vec![16_000, 44_100],
            channels: vec![1, 2],
        };
        let json = serde_json::to_string(&dev).expect("serialize");
        let back: AudioDevice = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.name, "Test Mic");
        assert_eq!(back.sample_rates, vec![16_000, 44_100]);
    }
}
