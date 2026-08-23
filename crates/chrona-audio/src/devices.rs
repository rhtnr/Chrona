//! Input device enumeration and the pure negotiation-preference logic used
//! to pick a capture config once a device's supported ranges are known.

use cpal::traits::{DeviceTrait, HostTrait};

/// One input device as reported by the host, with a stable id suitable for
/// round-tripping through [`find_by_id`].
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// All input devices currently visible to the default host.
///
/// Never panics: any per-device or host-level query failure (disconnects
/// mid-enumeration, no host available, headless CI) is treated as "skip this
/// device" / "no devices", never propagated as a panic.
pub fn list_input_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default_id = host
        .default_input_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string());

    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };

    devices
        .filter_map(|device| {
            // `Device`'s `Display` impl (and thus `to_string()`) can fail if the
            // backend query errors, and `to_string()` panics on a `Display` error
            // internally — so we go through `description()` directly instead and
            // treat a failure as "skip this device", never a panic.
            let id = device.id().ok()?.to_string();
            let name = device.description().ok()?.name().to_string();
            Some(DeviceInfo {
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
                name,
            })
        })
        .collect()
}

/// Look up an input device by the stable id string [`DeviceInfo::id`] reports.
/// Returns `None` if no currently visible input device matches.
pub(crate) fn find_by_id(host: &cpal::Host, id: &str) -> Option<cpal::Device> {
    host.input_devices().ok()?.find(|device| {
        device
            .id()
            .is_ok_and(|device_id| device_id.to_string() == id)
    })
}

/// Given the supported configs a device reports, pick the negotiation target:
/// (sample_rate_hz, channels). Preference: 48k -> 44.1k -> device default; mono
/// if available at that rate, else the fewest channels offered.
pub fn choose_config(supported: &[(f64, u16)], default: (f64, u16)) -> (f64, u16) {
    const PREFERRED_RATES_HZ: [f64; 2] = [48_000.0, 44_100.0];
    for rate in PREFERRED_RATES_HZ {
        let min_channels = supported
            .iter()
            .filter(|&&(r, _)| r == rate)
            .map(|&(_, channels)| channels)
            .min();
        if let Some(channels) = min_channels {
            return (rate, channels);
        }
    }
    default
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_48k_mono_then_44k1_then_default() {
        assert_eq!(
            choose_config(&[(48_000.0, 2), (48_000.0, 1)], (96_000.0, 2)),
            (48_000.0, 1)
        );
        assert_eq!(
            choose_config(&[(44_100.0, 2)], (96_000.0, 2)),
            (44_100.0, 2)
        );
        assert_eq!(choose_config(&[], (96_000.0, 2)), (96_000.0, 2));
        assert_eq!(
            choose_config(&[(48_000.0, 4), (48_000.0, 2)], (48_000.0, 4)),
            (48_000.0, 2)
        );
    }
}
