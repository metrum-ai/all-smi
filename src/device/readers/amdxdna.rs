// Copyright 2025 Lablup Inc. and Jeongkyu Shin
// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
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

//! Linux AMD XDNA NPU discovery from the kernel accel class.
//!
//! Discovery and firmware identity are read from sysfs. This reader does not
//! invoke `xrt-smi`: its examine command requires elevated access on supported
//! systems, and the exporter must remain useful without privilege.

use crate::device::GpuReader;
use crate::device::platform_detection::{ACCEL_SYSFS_CLASS_DIR, is_amdxdna_accel_node};
use crate::device::types::{GPU_METRIC_UNAVAILABLE, GpuInfo, ProcessInfo};
use crate::utils::get_hostname;
use chrono::Local;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_ACCEL_CLASS: &str = ACCEL_SYSFS_CLASS_DIR;

pub struct AmdXdnaNpuReader {
    accel_class: PathBuf,
    proc_root: PathBuf,
}

impl Default for AmdXdnaNpuReader {
    fn default() -> Self {
        Self::new()
    }
}

impl AmdXdnaNpuReader {
    pub fn new() -> Self {
        Self {
            accel_class: PathBuf::from(DEFAULT_ACCEL_CLASS),
            proc_root: PathBuf::from("/proc"),
        }
    }

    #[cfg(test)]
    fn with_paths(accel_class: PathBuf, proc_root: PathBuf) -> Self {
        Self {
            accel_class,
            proc_root,
        }
    }

    fn devices(&self) -> Vec<GpuInfo> {
        read_xdna_devices(&self.accel_class, &get_hostname())
    }
}

impl GpuReader for AmdXdnaNpuReader {
    fn get_gpu_info(&self) -> Vec<GpuInfo> {
        self.devices()
    }

    fn get_process_info(&self) -> Vec<ProcessInfo> {
        self.devices()
            .iter()
            .enumerate()
            .flat_map(|(npu_index, gpu)| {
                let Some(device_node) = gpu.detail.get("device_node") else {
                    return Vec::new();
                };
                read_fdinfo_clients(
                    &self.proc_root,
                    Path::new(device_node),
                    npu_index,
                    &gpu.uuid,
                )
            })
            .collect()
    }
}

fn read_xdna_devices(accel_class: &Path, hostname: &str) -> Vec<GpuInfo> {
    let Ok(entries) = fs::read_dir(accel_class) else {
        return Vec::new();
    };
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    entries
        .flatten()
        .filter(|entry| is_amdxdna_accel_node(&entry.path()))
        .filter_map(|entry| {
            let node_name = entry.file_name().to_string_lossy().into_owned();
            gpu_info_from_node(&entry.path(), &node_name, hostname, &timestamp)
        })
        .collect()
}

fn read_fdinfo_clients(
    proc_root: &Path,
    device_node: &Path,
    device_id: usize,
    device_uuid: &str,
) -> Vec<ProcessInfo> {
    let Ok(processes) = fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut clients = Vec::new();
    for process in processes.flatten() {
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let fd_dir = process.path().join("fd");
        // `/proc/<pid>/fd` is commonly unreadable under ptrace/Yama policy;
        // inaccessible processes are an unavailable client list, not an error.
        let Ok(fds) = fs::read_dir(fd_dir) else {
            continue;
        };
        let mut memory_by_client: HashMap<String, u64> = HashMap::new();
        for fd in fds.flatten() {
            let Ok(target) = fs::read_link(fd.path()) else {
                continue;
            };
            if !same_accel_node(&target, device_node) {
                continue;
            }
            let Ok(fdinfo) = fs::read_to_string(process.path().join("fdinfo").join(fd.file_name()))
            else {
                continue;
            };
            let Some(parsed) = parse_amdxdna_fdinfo(&fdinfo) else {
                continue;
            };
            let Some(memory_bytes) = parsed.memory_bytes else {
                continue;
            };
            let client_key = parsed.client_id.map_or_else(
                || format!("fd:{}", fd.file_name().to_string_lossy()),
                |client_id| format!("client:{client_id}"),
            );
            // Multiple fd descriptors for the same DRM client carry the
            // same per-client allocation totals. Count the client once.
            memory_by_client
                .entry(client_key)
                .and_modify(|existing| *existing = (*existing).max(memory_bytes))
                .or_insert(memory_bytes);
        }
        if !memory_by_client.is_empty() {
            let used_memory = memory_by_client
                .values()
                .fold(0_u64, |total, bytes| total.saturating_add(*bytes));
            clients.push(process_info_from_proc(
                &process.path(),
                pid,
                device_id,
                device_uuid,
                used_memory,
            ));
        }
    }
    clients
}

fn same_accel_node(target: &Path, device_node: &Path) -> bool {
    target.file_name() == device_node.file_name()
        && target
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|parent| parent == "accel")
}

#[derive(Debug, PartialEq, Eq)]
struct AmdXdnaFdInfo {
    client_id: Option<u64>,
    memory_bytes: Option<u64>,
}

fn parse_amdxdna_fdinfo(fdinfo: &str) -> Option<AmdXdnaFdInfo> {
    let lines: Vec<&str> = fdinfo.lines().collect();
    let driver = lines
        .iter()
        .find_map(|line| line.strip_prefix("drm-driver:").map(str::trim))?;
    if !matches!(driver, "amdxdna_accel_driver" | "amdxdna") {
        return None;
    }
    let client_id = lines
        .iter()
        .find_map(|line| line.strip_prefix("drm-client-id:").map(str::trim))
        .and_then(|value| value.parse::<u64>().ok());

    let total_rows: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("drm-total-"))
        .collect();
    let memory_bytes = if total_rows.is_empty() {
        parse_drm_memory_rows(
            lines
                .iter()
                .copied()
                .filter(|line| line.starts_with("drm-memory-")),
        )
    } else {
        parse_drm_memory_rows(total_rows.into_iter())
    };
    Some(AmdXdnaFdInfo {
        client_id,
        memory_bytes,
    })
}

/// Sum DRM total-memory rows, preferring the current `drm-total-*` form at
/// the call site and accepting deprecated `drm-memory-*` only as fallback.
/// Unitless values are bytes; the fdinfo spec also permits KiB and MiB.
fn parse_drm_memory_rows<'a>(rows: impl Iterator<Item = &'a str>) -> Option<u64> {
    let mut found = false;
    let mut total = 0_u64;
    for line in rows {
        let (_, value) = line.split_once(':')?;
        let mut fields = value.split_whitespace();
        let count = fields.next()?.parse::<u64>().ok()?;
        let bytes = match fields.next() {
            None => count,
            Some("KiB") => count.checked_mul(1024)?,
            Some("MiB") => count.checked_mul(1024 * 1024)?,
            Some(_) => return None,
        };
        if fields.next().is_some() {
            return None;
        }
        total = total.checked_add(bytes)?;
        found = true;
    }
    found.then_some(total)
}

fn process_info_from_proc(
    process_dir: &Path,
    pid: u32,
    device_id: usize,
    device_uuid: &str,
    used_memory: u64,
) -> ProcessInfo {
    let comm = fs::read_to_string(process_dir.join("comm"))
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("pid-{pid}"));
    let status = fs::read_to_string(process_dir.join("status")).unwrap_or_default();
    let value = |key: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map(str::trim)
    };
    let number = |key: &str| value(key).and_then(|v| v.split_whitespace().next()?.parse().ok());
    let uid = value("Uid:")
        .and_then(|v| v.split_whitespace().next())
        .unwrap_or("");
    let memory_kib = |key: &str| {
        value(key)
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .and_then(|kib| kib.checked_mul(1024))
            .unwrap_or(0)
    };
    ProcessInfo {
        device_id,
        device_uuid: device_uuid.to_string(),
        pid,
        process_name: comm.clone(),
        used_memory,
        cpu_percent: 0.0,
        memory_percent: 0.0,
        memory_rss: memory_kib("VmRSS:"),
        memory_vms: memory_kib("VmSize:"),
        user: uid.to_string(),
        state: value("State:")
            .and_then(|v| v.split_whitespace().next())
            .unwrap_or("")
            .to_string(),
        start_time: String::new(),
        cpu_time: 0,
        // The generic process merge may replace this with its existing
        // command metadata; this reader itself only has `/proc/<pid>/comm`.
        command: comm,
        ppid: number("PPid:").unwrap_or(0),
        threads: number("Threads:").unwrap_or(0),
        uses_gpu: true,
        priority: 0,
        nice_value: 0,
        gpu_utilization: 0.0,
        gpu_mem_util: None,
        enc_util: None,
        dec_util: None,
    }
}

fn gpu_info_from_node(
    node: &Path,
    node_name: &str,
    hostname: &str,
    timestamp: &str,
) -> Option<GpuInfo> {
    let device = node.join("device");
    let uevent = fs::read_to_string(device.join("uevent")).unwrap_or_default();
    let pci_slot = uevent
        .lines()
        .find_map(|line| line.strip_prefix("PCI_SLOT_NAME="))
        .map(str::to_string);
    let uuid = pci_slot
        .as_ref()
        .map(|slot| format!("AMD-XDNA-{slot}"))
        .unwrap_or_else(|| format!("AMD-XDNA-{node_name}"));
    let name = fs::read_to_string(device.join("vbnv"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "AMD XDNA NPU".to_string());
    let firmware = fs::read_to_string(device.join("fw_version"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let mut detail = HashMap::new();
    detail.insert("driver".to_string(), "amdxdna".to_string());
    detail.insert("device_node".to_string(), format!("/dev/accel/{node_name}"));
    detail.insert(
        crate::device::readers::detail_keys::MEMORY_STATUS_DETAIL_KEY.to_string(),
        "unavailable".to_string(),
    );
    detail.insert("metrics_available".to_string(), "false".to_string());
    if let Some(slot) = pci_slot {
        detail.insert("pci_slot_name".to_string(), slot);
    }
    if let Some(firmware) = &firmware {
        detail.insert("firmware_version".to_string(), firmware.clone());
    }

    Some(GpuInfo {
        uuid,
        time: timestamp.to_string(),
        name,
        device_type: "NPU".to_string(),
        host_id: hostname.to_string(),
        hostname: hostname.to_string(),
        instance: hostname.to_string(),
        utilization: GPU_METRIC_UNAVAILABLE,
        ane_utilization: GPU_METRIC_UNAVAILABLE,
        dla_utilization: None,
        tensorcore_utilization: None,
        temperature: 0,
        used_memory: 0,
        total_memory: 0,
        frequency: 0,
        power_consumption: GPU_METRIC_UNAVAILABLE,
        gpu_core_count: None,
        temperature_threshold_slowdown: None,
        temperature_threshold_shutdown: None,
        temperature_threshold_max_operating: None,
        temperature_threshold_acoustic: None,
        performance_state: None,
        fan_speed_rpm: None,
        numa_node_id: None,
        gsp_firmware_mode: None,
        gsp_firmware_version: None,
        nvlink_remote_devices: Vec::new(),
        gpm_metrics: None,
        throttle_reasons: None,
        energy_hw_millijoules: None,
        remapped_rows: None,
        nvlink_errors: Vec::new(),
        utilization_samples: None,
        xid_event_counts: HashMap::new(),
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_only_amdxdna_accel_nodes_and_reads_public_firmware_identity() {
        let class = tempfile::tempdir().expect("accel class tempdir");
        let node = class.path().join("accel0");
        let device = node.join("device");
        fs::create_dir_all(&device).unwrap();
        fs::write(
            device.join("uevent"),
            "DRIVER=amdxdna\nPCI_SLOT_NAME=0000:c3:00.1\n",
        )
        .unwrap();
        fs::write(device.join("vbnv"), "NPU Strix Halo\n").unwrap();
        fs::write(device.join("fw_version"), "1.1.2.65\n").unwrap();

        let other = class.path().join("accel1/device");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("uevent"), "DRIVER=google_tpu\n").unwrap();

        let devices = AmdXdnaNpuReader::with_paths(
            class.path().to_path_buf(),
            PathBuf::from("/proc/does-not-exist"),
        )
        .devices();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_type, "NPU");
        assert_eq!(devices[0].name, "NPU Strix Halo");
        assert_eq!(devices[0].uuid, "AMD-XDNA-0000:c3:00.1");
        assert_eq!(
            devices[0].detail.get("firmware_version").unwrap(),
            "1.1.2.65"
        );
        assert_eq!(devices[0].utilization, GPU_METRIC_UNAVAILABLE);
        assert_eq!(devices[0].power_consumption, GPU_METRIC_UNAVAILABLE);
        assert_eq!(devices[0].total_memory, 0);
        assert_eq!(devices[0].detail.get("metrics_available").unwrap(), "false");
    }

    #[test]
    fn real_amdxdna_fdinfo_fixture_parses_driver_client_and_zero_byte_totals() {
        let fdinfo = include_str!("../../../tests/fixtures/amdxdna/fdinfo-accel0.txt");
        assert_eq!(
            parse_amdxdna_fdinfo(fdinfo),
            Some(AmdXdnaFdInfo {
                client_id: Some(2083),
                memory_bytes: Some(0),
            })
        );
    }

    #[test]
    fn total_memory_rows_are_preferred_and_units_follow_fdinfo_spec() {
        let fdinfo = "drm-driver: amdxdna_accel_driver\ndrm-total-memory: 100\ndrm-total-vram: 2 KiB\ndrm-memory-old: 99 MiB\n";
        assert_eq!(
            parse_amdxdna_fdinfo(fdinfo).unwrap().memory_bytes,
            Some(100 + 2 * 1024)
        );

        let legacy = "drm-driver: amdxdna_accel_driver\ndrm-memory-local: 64 KiB\ndrm-memory-shared: 2 MiB\n";
        assert_eq!(
            parse_amdxdna_fdinfo(legacy).unwrap().memory_bytes,
            Some(64 * 1024 + 2 * 1024 * 1024)
        );
        assert_eq!(
            parse_amdxdna_fdinfo("drm-driver: amdxdna_accel_driver\ndrm-total-memory: 100\n")
                .unwrap()
                .memory_bytes,
            Some(100)
        );
        assert_eq!(
            parse_amdxdna_fdinfo("drm-driver: amdxdna_accel_driver\ndrm-total-memory: 100 kB\n")
                .unwrap()
                .memory_bytes,
            None
        );
        assert!(parse_amdxdna_fdinfo("drm-driver: amdgpu\ndrm-total-memory: 100\n").is_none());
    }

    #[test]
    fn fdinfo_clients_dedupe_client_ids_and_reject_other_drivers() {
        let class = tempfile::tempdir().expect("accel class tempdir");
        let device = class.path().join("accel0/device");
        fs::create_dir_all(&device).unwrap();
        fs::write(
            device.join("uevent"),
            "DRIVER=amdxdna\nPCI_SLOT_NAME=0000:c3:00.1\n",
        )
        .unwrap();
        fs::write(device.join("vbnv"), "NPU Strix Halo\n").unwrap();

        let proc_root = tempfile::tempdir().expect("proc tempdir");
        let process = proc_root.path().join("123");
        fs::create_dir_all(process.join("fd")).unwrap();
        fs::create_dir_all(process.join("fdinfo")).unwrap();
        fs::write(process.join("comm"), "python-worker\n").unwrap();
        fs::write(
            process.join("status"),
            "Name:\tpython-worker\nState:\tS (sleeping)\nUid:\t1000\t1000\t1000\t1000\nPPid:\t1\nThreads:\t2\nVmRSS:\t8 kB\nVmSize:\t16 kB\n",
        )
        .unwrap();
        for fd in ["3", "4", "5"] {
            std::os::unix::fs::symlink("/dev/accel/accel0", process.join("fd").join(fd)).unwrap();
        }
        fs::write(
            process.join("fdinfo/3"),
            "drm-driver:\tamdxdna_accel_driver\ndrm-client-id:\t42\ndrm-total-memory:\t100\n",
        )
        .unwrap();
        fs::write(
            process.join("fdinfo/4"),
            "drm-driver:\tamdxdna_accel_driver\ndrm-client-id:\t42\ndrm-total-memory:\t100\n",
        )
        .unwrap();
        fs::write(
            process.join("fdinfo/5"),
            "drm-driver:\tamdxdna_accel_driver\ndrm-client-id:\t43\ndrm-memory-local:\t64 KiB\n",
        )
        .unwrap();

        let no_memory = proc_root.path().join("124");
        fs::create_dir_all(no_memory.join("fd")).unwrap();
        fs::create_dir_all(no_memory.join("fdinfo")).unwrap();
        std::os::unix::fs::symlink("/dev/accel/accel0", no_memory.join("fd/3")).unwrap();
        fs::write(
            no_memory.join("fdinfo/3"),
            "drm-driver:\tamdxdna_accel_driver\ndrm-client-id:\t44\n",
        )
        .unwrap();

        let other_driver = proc_root.path().join("125");
        fs::create_dir_all(other_driver.join("fd")).unwrap();
        fs::create_dir_all(other_driver.join("fdinfo")).unwrap();
        std::os::unix::fs::symlink("/dev/accel/accel0", other_driver.join("fd/3")).unwrap();
        fs::write(
            other_driver.join("fdinfo/3"),
            "drm-driver:\tamdgpu\ndrm-client-id:\t45\ndrm-total-memory:\t999 MiB\n",
        )
        .unwrap();

        let unreadable = proc_root.path().join("126");
        fs::create_dir_all(&unreadable).unwrap();
        fs::write(unreadable.join("fd"), "not-a-directory").unwrap();

        let reader = AmdXdnaNpuReader::with_paths(
            class.path().to_path_buf(),
            proc_root.path().to_path_buf(),
        );
        let clients = reader.get_process_info();
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].pid, 123);
        assert_eq!(clients[0].device_uuid, "AMD-XDNA-0000:c3:00.1");
        assert_eq!(clients[0].used_memory, 100 + 64 * 1024);
        assert_eq!(clients[0].process_name, "python-worker");
        assert_eq!(clients[0].memory_rss, 8 * 1024);
    }

    #[test]
    fn missing_optional_sysfs_files_leave_a_discovered_device_with_absent_metrics() {
        let class = tempfile::tempdir().expect("accel class tempdir");
        let device = class.path().join("accel2/device");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("uevent"), "DRIVER=amdxdna\n").unwrap();

        let devices = read_xdna_devices(class.path(), "test-host");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "AMD XDNA NPU");
        assert!(!devices[0].detail.contains_key("firmware_version"));
        assert_eq!(devices[0].utilization, GPU_METRIC_UNAVAILABLE);
    }
}
