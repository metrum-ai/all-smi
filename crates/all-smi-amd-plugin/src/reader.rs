// Copyright 2025 Lablup Inc. and Jeongkyu Shin
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use all_smi::device::GpuReader;
use all_smi::device::readers::common_cache::{DetailBuilder, DeviceStaticInfo};
use all_smi::device::types::{GpuInfo, MAX_GPU_FAN_RPM, ProcessInfo};
use all_smi::utils::get_hostname;
use chrono::Local;
use libamdgpu_top::AMDGPU::{DeviceHandle, GPU_INFO, GpuMetrics, MetricsInfo};
use libamdgpu_top::stat::{self, FdInfoStat, ProcInfo};
use libamdgpu_top::{AppDeviceInfo, DevicePath, VramUsage};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// GPU metric validation constants
const MAX_GPU_UTILIZATION: f64 = 100.0; // Maximum utilization percentage
const MAX_GPU_POWER_WATTS: f64 = 1000.0; // Maximum power consumption in watts
const MAX_GPU_TEMP_CELSIUS: u32 = 125; // Maximum temperature in Celsius
const MAX_GPU_FREQ_MHZ: u32 = 5000; // Maximum frequency in MHz
const MAX_GPU_MEMORY_BYTES: u64 = 512 * 1024 * 1024 * 1024; // 512GB max memory

// Driver version validation constant
// Linux kernel versions typically don't exceed 999 for any component
const MAX_VERSION_COMPONENT: i32 = 999;

/// Cap a raw `sensors.fan_rpm` reading so a garbled `libamdgpu_top` sample
/// can never propagate `u32::MAX` into `GpuInfo::fan_speed_rpm` or the `Fan
/// Speed` detail string. Mirrors the same defence applied to temperature,
/// frequency, power, and memory a few lines below, and shares its bound
/// with the Windows ADL, Intel sysfs, and Intel Level Zero fan readings via
/// [`all_smi::device::types::MAX_GPU_FAN_RPM`].
fn clamp_fan_rpm(rpm: Option<u32>) -> Option<u32> {
    rpm.map(|value| value.min(MAX_GPU_FAN_RPM))
}

/// Convert the `gpu_metrics` DRAM bandwidth field (decimal MB/s) to bytes/s.
fn dram_bandwidth_mb_s_to_bytes_per_second(value: u16) -> u64 {
    u64::from(value) * 1_000_000
}

fn valid_metrics_percent(value: u16) -> Option<f64> {
    (value <= 100).then(|| f64::from(value))
}

fn valid_metrics_u16(value: u16) -> Option<u16> {
    (value != u16::MAX).then_some(value)
}

/// Convert a `gpu_metrics` socket power reading to watts.
///
/// The unit depends on the table's format revision, so one divisor cannot
/// serve every card. The amdgpu driver fills the dGPU tables (format 1:
/// `average_socket_power` up to v1.3, `curr_socket_power` on the v1.4 / v1.5
/// tables the Instinct MI300 series uses) in watts, and the APU tables
/// (formats 2 and 3) from SMU fields that are milliwatts on every SMU except
/// Renoir firmware new enough to report watts, which the table cannot tell
/// apart and which keeps its previous scaling here.
///
/// Returns `None` for the driver's unsupported sentinel and for a format
/// revision this reader does not know, so the caller falls back to hwmon
/// instead of publishing a guess.
fn metrics_socket_power_watts(format_revision: u8, raw: u32) -> Option<f64> {
    if raw == u32::from(u16::MAX) || raw == u32::MAX {
        return None;
    }
    let watts = match format_revision {
        1 => f64::from(raw),
        2 | 3 => f64::from(raw) / 1000.0,
        _ => return None,
    };
    Some(watts.clamp(0.0, MAX_GPU_POWER_WATTS))
}

fn apu_gfx_power_watts(format_revision: u8, raw_mw: u32) -> Option<f64> {
    matches!(format_revision, 2 | 3)
        .then(|| metrics_socket_power_watts(format_revision, raw_mw))
        .flatten()
}

fn read_gtt_memory_bytes(sysfs_path: &std::path::Path) -> Option<(u64, u64)> {
    let used = std::fs::read_to_string(sysfs_path.join("mem_info_gtt_used"))
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    let total = std::fs::read_to_string(sysfs_path.join("mem_info_gtt_total"))
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    (used <= total).then_some((used, total))
}

#[derive(Debug, PartialEq, Eq)]
struct DpmClockInfo {
    min_mhz: u32,
    max_mhz: u32,
    current_level: Option<u32>,
    current_mhz: Option<u32>,
}

fn parse_pp_dpm_clock_levels(contents: &str, sclk: bool) -> Option<DpmClockInfo> {
    let mut levels = Vec::new();
    for line in contents.lines() {
        let Some((index, rest)) = line.split_once(':') else {
            continue;
        };
        let Ok(index) = index.trim().parse::<u32>() else {
            continue;
        };
        let mut mhz = None;
        for token in rest.split_whitespace() {
            let token = token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
            let Some((digits, unit)) = token.split_at_checked(token.len().saturating_sub(3)) else {
                continue;
            };
            if !unit.eq_ignore_ascii_case("mhz") {
                continue;
            }
            if let Ok(value) = digits.parse::<u32>() {
                mhz = Some(value);
                break;
            }
        }
        let Some(mhz) = mhz else {
            continue;
        };
        let current = rest.contains('*');
        let range_marker = rest
            .split_whitespace()
            .any(|word| matches!(word.to_ascii_lowercase().as_str(), "min" | "max"));
        levels.push((index, mhz, current, range_marker));
    }
    // SMU v14 SCLK reports unlabeled 0=min, 1=current (or a fallback
    // value), 2=max. The same shape can also be a genuine three-level
    // table, so omit its ambiguous level rather than report a false index.
    let sclk_three_rows = sclk && levels.len() == 3 && levels.iter().map(|row| row.0).eq([0, 1, 2]);
    let min_mhz = if sclk_three_rows {
        levels[0].1
    } else {
        levels.iter().map(|(_, mhz, _, _)| *mhz).min()?
    };
    let max_mhz = if sclk_three_rows {
        levels[2].1
    } else {
        levels.iter().map(|(_, mhz, _, _)| *mhz).max()?
    };
    let current = levels.iter().find(|(_, _, current, _)| *current);
    let has_min_max_rows = sclk_three_rows || levels.iter().any(|(_, _, _, marked)| *marked);
    Some(DpmClockInfo {
        min_mhz,
        max_mhz,
        // On SMU v14, min/current/max are synthetic row labels, not table
        // indices. Keep the current frequency but do not expose that row's
        // ordinal as a DPM level.
        current_level: (!has_min_max_rows)
            .then(|| current.map(|(index, _, _, _)| *index))
            .flatten(),
        current_mhz: current.map(|(_, mhz, _, _)| *mhz),
    })
}

fn active_power_profile(contents: &str) -> Option<String> {
    for line in contents.lines().filter(|line| line.contains('*')) {
        // Kernel variants use either `1 NAME*`, `1 NAME*:`, or `1: NAME *:`.
        // The non-numeric `S:` status line is intentionally ignored.
        let line = line.trim();
        let parsed = match line.split_once(':') {
            Some((prefix, rest)) if prefix.trim().parse::<u32>().is_ok() => {
                Some((prefix.trim(), rest.trim()))
            }
            _ => line
                .split_once(char::is_whitespace)
                .map(|(index, rest)| (index.trim(), rest.trim())),
        };
        let Some((index_token, rest)) = parsed else {
            continue;
        };
        let Ok(index) = index_token.parse::<u32>() else {
            continue;
        };
        let name = rest
            .split('*')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_end_matches(':')
            .trim();
        if !name.is_empty() {
            return Some(format!("{index}:{name}"));
        }
    }
    None
}

fn read_vcn_busy_percent(sysfs_path: &std::path::Path) -> Option<u16> {
    let busy = std::fs::read_to_string(sysfs_path.join("vcn_busy_percent"))
        .ok()?
        .trim()
        .parse::<u16>()
        .ok()?;
    (busy <= 100).then_some(busy)
}

fn vcn_activity_percent(metrics_value: Option<u16>, sysfs_path: &std::path::Path) -> Option<u16> {
    metrics_value
        .filter(|activity| *activity <= 100)
        .or_else(|| read_vcn_busy_percent(sysfs_path))
}

fn read_apu_sysfs_details(sysfs_path: &std::path::Path, detail: &mut HashMap<String, String>) {
    use all_smi::device::readers::detail_keys as keys;
    for (file, domain) in [
        ("pp_dpm_sclk", "SCLK"),
        ("pp_dpm_mclk", "MCLK"),
        ("pp_dpm_fclk", "FCLK"),
        ("pp_dpm_socclk", "SOCCLK"),
    ] {
        let Ok(contents) = std::fs::read_to_string(sysfs_path.join(file)) else {
            continue;
        };
        let Some(clock) = parse_pp_dpm_clock_levels(&contents, file == "pp_dpm_sclk") else {
            continue;
        };
        detail.insert(
            format!("{}{}", keys::DPM_CLOCK_MIN_MHZ_DETAIL_KEY_PREFIX, domain),
            clock.min_mhz.to_string(),
        );
        detail.insert(
            format!("{}{}", keys::DPM_CLOCK_MAX_MHZ_DETAIL_KEY_PREFIX, domain),
            clock.max_mhz.to_string(),
        );
        if let Some(level) = clock.current_level {
            detail.insert(
                format!("{}{}", keys::DPM_CLOCK_LEVEL_DETAIL_KEY_PREFIX, domain),
                level.to_string(),
            );
        }
        if let Some(mhz) = clock.current_mhz {
            detail.insert(format!("Frequency: {domain} (DPM)"), mhz.to_string());
        }
    }
    if let Ok(level) = std::fs::read_to_string(sysfs_path.join("power_dpm_force_performance_level"))
    {
        let level = level.trim();
        if !level.is_empty() {
            detail.insert(
                keys::AMD_PERFORMANCE_LEVEL_DETAIL_KEY.to_string(),
                level.to_string(),
            );
        }
    }
    if let Ok(profile) = std::fs::read_to_string(sysfs_path.join("pp_power_profile_mode"))
        && let Some(profile) = active_power_profile(&profile)
    {
        detail.insert(keys::AMD_POWER_PROFILE_DETAIL_KEY.to_string(), profile);
    }
}

const APU_SYSFS_DETAILS_CACHE_TTL: Duration = Duration::from_secs(10);
type ApuSysfsDetailsCache = Mutex<Option<(Instant, HashMap<String, String>)>>;

fn read_cached_apu_sysfs_details(
    sysfs_path: &std::path::Path,
    cache: &ApuSysfsDetailsCache,
    detail: &mut HashMap<String, String>,
) {
    read_cached_apu_sysfs_details_at(sysfs_path, cache, detail, Instant::now());
}

fn read_cached_apu_sysfs_details_at(
    sysfs_path: &std::path::Path,
    cache: &ApuSysfsDetailsCache,
    detail: &mut HashMap<String, String>,
    now: Instant,
) {
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.as_ref().is_none_or(|(fetched_at, _)| {
        now.saturating_duration_since(*fetched_at) >= APU_SYSFS_DETAILS_CACHE_TTL
    }) {
        let mut fresh = HashMap::new();
        read_apu_sysfs_details(sysfs_path, &mut fresh);
        *cache = Some((now, fresh));
    }
    if let Some((_, cached)) = cache.as_ref() {
        detail.extend(cached.clone());
    }
}

fn insert_apu_metric_details(
    detail: &mut HashMap<String, String>,
    clocks_mhz: &[(&str, Option<u16>)],
    vcn_activity: Option<u16>,
    throttle_residency: &[(&str, Option<u32>)],
) {
    for (domain, frequency_mhz) in clocks_mhz {
        if let Some(frequency_mhz) =
            frequency_mhz.filter(|frequency| *frequency > 0 && *frequency != u16::MAX)
        {
            detail.insert(
                format!(
                "{}{domain}",
                all_smi::device::readers::detail_keys::AVERAGE_FREQUENCY_DOMAIN_DETAIL_KEY_PREFIX
            ),
                frequency_mhz.to_string(),
            );
        }
    }
    if let Some(activity) = vcn_activity.filter(|activity| *activity <= 100) {
        detail.insert("Engine: VCN".to_string(), activity.to_string());
    }
    for (reason, raw_residency) in throttle_residency {
        if let Some(raw_residency) = raw_residency.filter(|value| *value != u32::MAX) {
            detail.insert(
                format!("Throttle residency raw: {reason}"),
                raw_residency.to_string(),
            );
        }
    }
}

/// Convert a `libamdgpu_top` hwmon power reading to watts.
///
/// `HwmonPower::value` is already whole watts: `libamdgpu_top` divides the
/// kernel's microwatt `power1_average` / `power1_input` by 1,000,000 before
/// handing it over. Dividing again reported an MI300X drawing 148 W as 0.148.
fn hwmon_power_watts(value: u32) -> f64 {
    f64::from(value).clamp(0.0, MAX_GPU_POWER_WATTS)
}

fn finite_process_percentages(cpu_percent: f64, memory_percent: f64) -> Option<(f64, f64)> {
    if !cpu_percent.is_finite() {
        eprintln!("AMD plugin omitted a process sample with non-finite cpu_percent");
        return None;
    }
    if !memory_percent.is_finite() {
        eprintln!("AMD plugin omitted a process sample with non-finite memory_percent");
        return None;
    }
    Some((cpu_percent, memory_percent))
}

/// Write the plugin's three per-poll sensor readings into `detail` in the
/// shared reader conventions, returning the clamped tachometer value for
/// `GpuInfo::fan_speed_rpm`.
///
/// The link goes through [`all_smi::device::readers::detail_keys::insert_pcie_details`]
/// as bare `pcie_gen_current` / `pcie_width_current` numbers (the two
/// maxima are passed `None`, so AMD writes no `pcie_gen_max` or
/// `pcie_width_max`); the tachometer is written as [`FAN_SPEED_DETAIL_KEY`]
/// (which stays in `detail` for snapshots and the cross-reader overwrite
/// guard in `intel_gpu_level_zero::apply_fan`) in the `"{fan} RPM"` format
/// every `Fan Speed` writer shares; and the memory clock is written as the
/// bare-`MHz` `clock_memory_current` the `all_smi_gpu_clock_memory_current_mhz`
/// gauge reads. It writes no `Current Link` and no `Memory Clock` key: both
/// churned the `all_smi_gpu_info` label set on every idle-to-load
/// transition, and their readings now travel as dedicated series instead.
///
/// Device-free on purpose, like `clamp_fan_rpm` above: taking plain
/// `Option` values is what makes it testable without an amdgpu device.
fn insert_sensor_details(
    detail: &mut HashMap<String, String>,
    current_link: Option<(u8, u8)>,
    fan_rpm: Option<u32>,
    mclk: Option<u32>,
) -> Option<u32> {
    all_smi::device::readers::detail_keys::insert_pcie_details(
        detail,
        current_link.map(|(link_gen, _)| u32::from(link_gen)),
        current_link.map(|(_, link_width)| u32::from(link_width)),
        None,
        None,
    );
    let fan_speed_rpm = clamp_fan_rpm(fan_rpm);
    if let Some(fan) = fan_speed_rpm {
        detail.insert(
            all_smi::device::readers::detail_keys::FAN_SPEED_DETAIL_KEY.to_string(),
            format!("{fan} RPM"),
        );
    }
    if let Some(mclk) = mclk {
        detail.insert(
            all_smi::device::readers::detail_keys::CLOCK_MEMORY_CURRENT_DETAIL_KEY.to_string(),
            mclk.to_string(),
        );
    }
    fan_speed_rpm
}

/// Per-device state that needs to be cached
///
/// # Thread Safety
///
/// The `vram_usage` field is protected by a `Mutex` to ensure thread-safe access
/// across multiple concurrent readers. The VramUsage struct from libamdgpu_top
/// maintains internal state that must be updated atomically.
///
/// ## Synchronization Guarantees
/// - All reads and writes to `vram_usage` are serialized through the mutex
/// - The mutex ensures memory ordering: all writes before unlock are visible after lock
/// - No data races can occur as long as all access goes through the mutex
///
/// ## Mutex Poisoning Recovery
/// If a thread panics while holding the mutex lock, the mutex becomes "poisoned"
/// to prevent other threads from observing potentially inconsistent state.
/// We handle this by:
/// 1. Detecting the poisoned state
/// 2. Attempting to recover with fresh data from the driver
/// 3. Using `catch_unwind` to handle potential panics during recovery
/// 4. Skipping the device if recovery fails to maintain system stability
///
/// ## Performance Considerations
/// - Mutex contention is minimal as updates are quick (microseconds)
/// - Each device has its own mutex, preventing global bottlenecks
/// - The lock is held only during the VramUsage update operations
struct AmdGpuDevice {
    device_path: DevicePath,
    device_handle: DeviceHandle,
    vram_usage: Mutex<VramUsage>, // Protected by mutex for thread-safe updates
    static_info: OnceLock<DeviceStaticInfo>, // Cached static device information
    apu_sysfs_details: ApuSysfsDetailsCache,
}

pub struct AmdGpuReader {
    devices: Vec<AmdGpuDevice>,
    /// Cached ROCm version (fetched only once, shared across all devices)
    rocm_version: OnceLock<Option<String>>,
}

impl Default for AmdGpuReader {
    fn default() -> Self {
        Self::new()
    }
}

impl AmdGpuReader {
    pub fn new() -> Self {
        // Check if we have permission to access AMD GPU devices
        // This prevents panic from libamdgpu_top when running without sudo
        // If no permission, silently return empty device list
        // The main program will handle showing sudo message before TUI starts
        if !Self::check_amd_gpu_permissions() {
            return Self {
                devices: Vec::new(),
                rocm_version: OnceLock::new(),
            };
        }

        let device_path_list = DevicePath::get_device_path_list();
        let mut devices = Vec::new();

        // Add device count validation to prevent unbounded growth
        const MAX_DEVICES: usize = 256;
        let device_paths_to_process: Vec<_> =
            device_path_list.into_iter().take(MAX_DEVICES).collect();

        for device_path in device_paths_to_process {
            match device_path.init() {
                Ok(amdgpu_dev) => {
                    // Get initial memory_info to create VramUsage
                    match amdgpu_dev.memory_info() {
                        Ok(memory_info) => {
                            let vram_usage = VramUsage::new(&memory_info);
                            devices.push(AmdGpuDevice {
                                device_path: device_path.clone(),
                                device_handle: amdgpu_dev,
                                vram_usage: Mutex::new(vram_usage),
                                static_info: OnceLock::new(),
                                apu_sysfs_details: Mutex::new(None),
                            });
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: Failed to get memory info for AMD GPU {}: {e}",
                                device_path.pci
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to initialize AMD GPU {}: {e}",
                        device_path.pci
                    );
                }
            }
        }

        Self {
            devices,
            rocm_version: OnceLock::new(),
        }
    }

    /// Get cached ROCm version, initializing if needed
    fn get_rocm_version(&self) -> Option<String> {
        self.rocm_version
            .get_or_init(libamdgpu_top::get_rocm_version)
            .clone()
    }

    /// Get cached static device info for a device, initializing if needed
    fn get_device_static_info<'a>(&self, device: &'a AmdGpuDevice) -> &'a DeviceStaticInfo {
        device
            .static_info
            .get_or_init(|| {
                // Fetch static device information once
                let ext_info = device.device_handle.device_info().ok();
                let memory_info = device.device_handle.memory_info().ok();

                let (device_name, mut detail) = if let (Some(ext), Some(mem)) =
                    (ext_info.as_ref(), memory_info.as_ref())
                {
                    let sensors = libamdgpu_top::stat::Sensors::new(
                        &device.device_handle,
                        &device.device_path.pci,
                        ext,
                    );

                    let app_device_info = AppDeviceInfo::new(
                        &device.device_handle,
                        ext,
                        mem,
                        &sensors,
                        &device.device_path,
                    );

                    let mut builder = DetailBuilder::new()
                        .insert("Device Name", &app_device_info.marketing_name)
                        .insert("PCI Bus", app_device_info.pci_bus.to_string());

                    // Add ROCm version
                    if let Some(ref ver) = self.get_rocm_version() {
                        builder = builder
                            .insert("ROCm Version", ver)
                            .insert("lib_name", "ROCm")
                            .insert("lib_version", ver);
                    }

                    let mut detail = builder.build();

                    // Add device details
                    detail.insert(
                        "Device ID".to_string(),
                        format!("{:#06x}", ext.device_id()),
                    );
                    detail.insert(
                        "Revision ID".to_string(),
                        format!("{:#04x}", ext.pci_rev_id()),
                    );
                    detail.insert(
                        "ASIC Name".to_string(),
                        app_device_info.asic_name.to_string(),
                    );

                    if let Some(ref vbios) = app_device_info.vbios {
                        detail.insert("VBIOS Version".to_string(), vbios.ver.clone());
                        detail.insert("VBIOS Date".to_string(), vbios.date.clone());
                    }

                    if let Some(ref cap) = app_device_info.power_cap {
                        detail.insert("Power Cap".to_string(), format!("{} W", cap.current));
                        detail.insert("Power Cap (Min)".to_string(), format!("{} W", cap.min));
                        detail.insert("Power Cap (Max)".to_string(), format!("{} W", cap.max));
                    }

                    if let Some(link) = app_device_info.max_gpu_link {
                        detail.insert(
                            "Max GPU Link".to_string(),
                            format!("Gen{} x{}", link.r#gen, link.width),
                        );
                    }

                    if let Some(link) = app_device_info.max_system_link {
                        detail.insert(
                            "Max System Link".to_string(),
                            format!("Gen{} x{}", link.r#gen, link.width),
                        );
                    }

                    if let Some(min_dpm_link) = app_device_info.min_dpm_link {
                        detail.insert(
                            "Min DPM Link".to_string(),
                            format!("Gen{} x{}", min_dpm_link.r#gen, min_dpm_link.width),
                        );
                    }

                    if let Some(max_dpm_link) = app_device_info.max_dpm_link {
                        detail.insert(
                            "Max DPM Link".to_string(),
                            format!("Gen{} x{}", max_dpm_link.r#gen, max_dpm_link.width),
                        );
                    }

                    (app_device_info.marketing_name, detail)
                } else {
                    (String::from("Unknown GPU"), HashMap::new())
                };

                // Get driver version
                match device.device_handle.get_drm_version_struct() {
                    Ok(drm) => {
                        if drm.version_major >= 0
                            && drm.version_major <= MAX_VERSION_COMPONENT
                            && drm.version_minor >= 0
                            && drm.version_minor <= MAX_VERSION_COMPONENT
                            && drm.version_patchlevel >= 0
                            && drm.version_patchlevel <= MAX_VERSION_COMPONENT
                        {
                            let ver = format!(
                                "{}.{}.{}",
                                drm.version_major, drm.version_minor, drm.version_patchlevel
                            );
                            detail.insert("Driver Version".to_string(), ver);
                        } else {
                            eprintln!(
                                "Warning: Invalid driver version components detected: {}.{}.{} for device {}",
                                drm.version_major, drm.version_minor, drm.version_patchlevel,
                                device.device_path.pci
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to get driver version for device {}: {e}",
                            device.device_path.pci
                        );
                    }
                };

                DeviceStaticInfo::with_details(device_name, None, detail)
            })
    }

    /// Check if we have permission to access AMD GPU devices
    /// Returns false if /dev/dri devices are not accessible
    fn check_amd_gpu_permissions() -> bool {
        use std::fs;

        // Check if /dev/dri directory exists and is accessible
        let dri_path = std::path::Path::new("/dev/dri");
        if !dri_path.exists() {
            return false;
        }

        // Try to read the directory to check permissions
        match fs::read_dir(dri_path) {
            Ok(entries) => {
                // Check if we can read at least one render or card device
                for entry in entries.flatten() {
                    let path = entry.path();
                    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    // Check card or render devices
                    if file_name.starts_with("card") || file_name.starts_with("render") {
                        // Check if we have read/write permissions
                        // For root: always has access
                        // SAFETY: libc::geteuid() is always safe to call - it's a simple
                        // system call that reads the effective user ID from the kernel.
                        // It cannot fail and doesn't access any memory we provide.
                        if unsafe { libc::geteuid() } == 0 {
                            return true; // Root always has access
                        }

                        // For non-root, check if we can actually open the device
                        if let Ok(_file) = fs::OpenOptions::new().read(true).write(true).open(&path)
                        {
                            return true; // We have access
                        }
                    }
                }
                false // No accessible devices found
            }
            Err(_) => false, // Cannot read /dev/dri directory
        }
    }
}

include!("reader/gpu_reader.rs");
include!("reader/tests.rs");
