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

//! Windows compute-only accelerators, using the shared DXCore/PDH snapshot.

use super::{windows_gpu_perf, windows_npu_xrt};
use crate::device::GpuReader;
use crate::device::types::{GPU_METRIC_UNAVAILABLE, GpuInfo, ProcessInfo};
use std::collections::{HashMap, HashSet};

pub struct WindowsNpuReader;

fn uuid(luid: windows_gpu_perf::ids::AdapterLuid) -> String {
    format!("DXCore-{:08x}-{:08x}", luid.high as u32, luid.low)
}

impl GpuReader for WindowsNpuReader {
    fn get_gpu_info(&self) -> Vec<GpuInfo> {
        let snapshot = windows_gpu_perf::snapshot();
        let hostname = crate::utils::get_hostname();
        let time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        snapshot
            .npus
            .iter()
            .map(|npu| {
                let adapter = &npu.adapter;
                let mut detail = HashMap::new();
                detail.insert("driver".to_string(), "windows_dxcore".to_string());
                detail.insert(
                    "pci_vendor_id".to_string(),
                    format!("{:04x}", adapter.vendor),
                );
                detail.insert(
                    "pci_device_id".to_string(),
                    format!("{:04x}", adapter.device),
                );
                let v = adapter.driver_version;
                if v != 0 {
                    detail.insert(
                        "driver_version".to_string(),
                        format!(
                            "{}.{}.{}.{}",
                            v >> 48,
                            (v >> 32) & 0xffff,
                            (v >> 16) & 0xffff,
                            v & 0xffff
                        ),
                    );
                }
                if adapter.vendor == 0x1022 || adapter.vendor == 0x1002 {
                    let mut xrt = windows_npu_xrt::cached_details();
                    if let Some(version) = xrt.remove("npu_driver_version") {
                        detail
                            .entry("driver_version".to_string())
                            .or_insert(version);
                    }
                    detail.extend(xrt);
                }
                if npu.used_memory.is_none() {
                    detail.insert(
                        super::detail_keys::MEMORY_STATUS_DETAIL_KEY.to_string(),
                        "unavailable".to_string(),
                    );
                }
                GpuInfo {
                    uuid: uuid(adapter.luid),
                    time: time.clone(),
                    name: adapter.name.clone(),
                    device_type: "NPU".to_string(),
                    host_id: hostname.clone(),
                    hostname: hostname.clone(),
                    instance: hostname.clone(),
                    utilization: npu.utilization.unwrap_or(GPU_METRIC_UNAVAILABLE),
                    ane_utilization: GPU_METRIC_UNAVAILABLE,
                    dla_utilization: None,
                    tensorcore_utilization: None,
                    temperature: 0,
                    used_memory: npu.used_memory.unwrap_or(0),
                    total_memory: adapter
                        .dedicated_memory
                        .saturating_add(adapter.shared_memory),
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
                }
            })
            .collect()
    }

    fn get_process_info(&self) -> Vec<ProcessInfo> {
        let mut snapshot = windows_gpu_perf::latest();
        if snapshot.npus.is_empty() {
            snapshot = windows_gpu_perf::snapshot();
        }
        let mut rows = Vec::new();
        for (index, npu) in snapshot.npus.iter().enumerate() {
            for &(pid, memory, utilization) in &npu.processes {
                let mut row =
                    windows_gpu_perf::gpu_process_row(index, &uuid(npu.adapter.luid), pid, memory);
                row.gpu_utilization = utilization.unwrap_or(0.0);
                rows.push(row);
            }
        }
        if rows.is_empty() {
            return rows;
        }
        let pids: HashSet<u32> = rows.iter().map(|row| row.pid).collect();
        let system_rows = crate::utils::system::with_global_system(|system| {
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::All,
                true,
                sysinfo::ProcessRefreshKind::everything().with_user(sysinfo::UpdateKind::Always),
            );
            system.refresh_memory();
            crate::device::process_list::get_all_processes(system, &pids)
        });
        crate::device::process_list::merge_gpu_processes(system_rows, rows)
    }
}
