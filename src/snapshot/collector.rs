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

//! Collection pipeline for a single snapshot sample.
//!
//! Defines [`SnapshotCollector`] (the trait that real hardware readers and
//! test doubles both implement) and [`DefaultSnapshotCollector`] (the
//! production implementation that wraps the existing reader factories).
//!
//! The runtime guarantee enforced by this module is that *every* reader
//! call is wrapped in `spawn_blocking` + `tokio::time::timeout`, so a
//! misbehaving reader cannot stall the whole snapshot.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use crate::cli::SnapshotIncludes;
use crate::device::{
    ChassisInfo, ChassisReader, CpuInfo, CpuReader, GpuInfo, GpuReader, MemoryInfo, MemoryReader,
    ProcessInfo, create_chassis_reader, get_cpu_readers, get_gpu_readers, get_memory_readers,
};
use crate::snapshot::options::{Snapshot, SnapshotError};
use crate::storage::info::StorageInfo;
use crate::storage::reader::{LocalStorageReader, StorageReader};

/// Trait that lets the snapshot collector pull from either real hardware
/// readers or test doubles. Defined here (rather than reusing the existing
/// platform-specific traits) so every section has a uniform signature that
/// can be timeout-wrapped the same way.
pub trait SnapshotCollector: Send + Sync {
    fn hostname(&self) -> String;
    fn collect_gpus(&self) -> Vec<GpuInfo>;
    fn collect_cpus(&self) -> Vec<CpuInfo>;
    fn collect_memory(&self) -> Vec<MemoryInfo>;
    fn collect_chassis(&self) -> Vec<ChassisInfo>;
    fn collect_processes(&self) -> Vec<ProcessInfo>;
    fn collect_storage(&self) -> Vec<StorageInfo>;
}

/// Default collector that wraps the existing device-reader factory
/// functions. Panics-in-readers propagate up through the awaiting task and
/// are converted to [`SnapshotError`] by the collection driver.
pub struct DefaultSnapshotCollector {
    gpu_readers: Vec<Box<dyn GpuReader>>,
    cpu_readers: Vec<Box<dyn CpuReader>>,
    memory_readers: Vec<Box<dyn MemoryReader>>,
    chassis_reader: Box<dyn ChassisReader>,
    storage_reader: Box<dyn StorageReader>,
    hostname: String,
}

impl DefaultSnapshotCollector {
    pub fn new() -> Self {
        Self {
            gpu_readers: get_gpu_readers(),
            cpu_readers: get_cpu_readers(),
            memory_readers: get_memory_readers(),
            chassis_reader: create_chassis_reader(),
            storage_reader: Box::new(LocalStorageReader::new()),
            hostname: crate::utils::get_hostname(),
        }
    }
}

impl Default for DefaultSnapshotCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotCollector for DefaultSnapshotCollector {
    fn hostname(&self) -> String {
        self.hostname.clone()
    }

    fn collect_gpus(&self) -> Vec<GpuInfo> {
        self.gpu_readers
            .iter()
            .flat_map(|r| r.get_gpu_info())
            .collect()
    }

    fn collect_cpus(&self) -> Vec<CpuInfo> {
        self.cpu_readers
            .iter()
            .flat_map(|r| r.get_cpu_info())
            .collect()
    }

    fn collect_memory(&self) -> Vec<MemoryInfo> {
        self.memory_readers
            .iter()
            .flat_map(|r| r.get_memory_info())
            .collect()
    }

    fn collect_chassis(&self) -> Vec<ChassisInfo> {
        self.chassis_reader.get_chassis_info().into_iter().collect()
    }

    fn collect_processes(&self) -> Vec<ProcessInfo> {
        self.gpu_readers
            .iter()
            .flat_map(|r| r.get_process_info())
            .collect()
    }

    fn collect_storage(&self) -> Vec<StorageInfo> {
        self.storage_reader.get_storage_info()
    }
}

/// Run a single collection pass using the provided collector.
///
/// Each requested section is wrapped in [`tokio::task::spawn_blocking`] and
/// governed by a [`tokio::time::timeout`] so that a single hung reader
/// cannot block the whole snapshot.
pub async fn collect_once<C: SnapshotCollector + 'static>(
    collector: Arc<C>,
    includes: &SnapshotIncludes,
    per_reader_timeout: Duration,
) -> Snapshot {
    let mut snap = Snapshot::new(collector.hostname());
    let sections: &[(&str, bool)] = &[
        ("gpu", includes.gpu),
        ("cpu", includes.cpu),
        ("memory", includes.memory),
        ("chassis", includes.chassis),
        ("process", includes.process),
        ("storage", includes.storage),
    ];

    for (name, enabled) in sections {
        if !*enabled {
            continue;
        }
        let c = collector.clone();
        let section = name.to_string();
        let join_handle = tokio::task::spawn_blocking(move || match section.as_str() {
            "gpu" => SectionResult::Gpus(c.collect_gpus()),
            "cpu" => SectionResult::Cpus(c.collect_cpus()),
            "memory" => SectionResult::Memory(c.collect_memory()),
            "chassis" => SectionResult::Chassis(c.collect_chassis()),
            "process" => SectionResult::Processes(c.collect_processes()),
            "storage" => SectionResult::Storage(c.collect_storage()),
            _ => SectionResult::Empty,
        });

        match timeout(per_reader_timeout, join_handle).await {
            Ok(Ok(result)) => match result {
                SectionResult::Gpus(v) => snap.gpus = Some(v),
                SectionResult::Cpus(v) => snap.cpus = Some(v),
                SectionResult::Memory(v) => snap.memory = Some(v),
                SectionResult::Chassis(v) => snap.chassis = Some(v),
                SectionResult::Processes(v) => snap.processes = Some(v),
                SectionResult::Storage(v) => snap.storage = Some(v),
                SectionResult::Empty => {}
            },
            Ok(Err(join_err)) => {
                snap.errors.push(SnapshotError {
                    section: (*name).to_string(),
                    kind: if join_err.is_panic() {
                        "panic".to_string()
                    } else {
                        "error".to_string()
                    },
                    message: join_err.to_string(),
                });
            }
            Err(_elapsed) => {
                // The `JoinHandle` is dropped when `timeout` returns
                // `Elapsed`, but `spawn_blocking` does NOT cancel the
                // underlying OS thread — it keeps running until the
                // reader returns. A hung driver call therefore leaks a
                // Tokio blocking-pool worker for the rest of the
                // process's lifetime. Emit a warning so operators have a
                // diagnostic trail. The CLI entrypoint in `src/main.rs`
                // bounds the blast radius by using a dedicated runtime
                // with `max_blocking_threads(32)` for the snapshot
                // invocation.
                tracing::warn!(
                    section = *name,
                    timeout_ms = per_reader_timeout.as_millis() as u64,
                    "snapshot reader timed out; the blocking worker cannot be cancelled \
                     and will continue until the reader returns"
                );
                snap.errors.push(SnapshotError {
                    section: (*name).to_string(),
                    kind: "timeout".to_string(),
                    message: format!(
                        "reader exceeded timeout of {} ms",
                        per_reader_timeout.as_millis()
                    ),
                });
            }
        }
    }

    if let (Some(gpus), Some(processes)) = (snap.gpus.as_ref(), snap.processes.as_mut()) {
        crate::device::process_list::align_gpu_process_device_indices_by_uuid(
            gpus.iter().map(|gpu| gpu.uuid.as_str()),
            processes,
        );
    }

    if let (Some(gpus), Some(chassis)) = (snap.gpus.as_ref(), snap.chassis.take()) {
        snap.chassis = Some(crate::metrics::gpu_readings::inject_chassis_gpu_power(
            chassis, gpus,
        ));
    }

    snap
}

/// Internal tagged union used to shuttle the result of a single
/// `spawn_blocking` reader call back to the driver loop without reserving
/// a separate channel per section. Stays private because the tag it carries
/// is already encoded by the source-section string.
enum SectionResult {
    Gpus(Vec<GpuInfo>),
    Cpus(Vec<CpuInfo>),
    Memory(Vec<MemoryInfo>),
    Chassis(Vec<ChassisInfo>),
    Processes(Vec<ProcessInfo>),
    Storage(Vec<StorageInfo>),
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::SnapshotIncludes;
    use std::collections::HashMap;

    struct XdnaCollector;

    impl SnapshotCollector for XdnaCollector {
        fn hostname(&self) -> String {
            "test-host".to_string()
        }
        fn collect_gpus(&self) -> Vec<GpuInfo> {
            vec![
                gpu("AMD-IGPU-0000:c2:00.0", "GPU"),
                gpu("AMD-XDNA-0000:c3:00.1", "NPU"),
            ]
        }
        fn collect_cpus(&self) -> Vec<CpuInfo> {
            Vec::new()
        }
        fn collect_memory(&self) -> Vec<MemoryInfo> {
            Vec::new()
        }
        fn collect_chassis(&self) -> Vec<ChassisInfo> {
            Vec::new()
        }
        fn collect_processes(&self) -> Vec<ProcessInfo> {
            vec![ProcessInfo {
                device_id: 0,
                device_uuid: "AMD-XDNA-0000:c3:00.1".to_string(),
                pid: 123,
                process_name: "npu-client".to_string(),
                used_memory: 4096,
                cpu_percent: 0.0,
                memory_percent: 0.0,
                memory_rss: 0,
                memory_vms: 0,
                user: "test".to_string(),
                state: "S".to_string(),
                start_time: String::new(),
                cpu_time: 0,
                command: "npu-client".to_string(),
                ppid: 1,
                threads: 1,
                uses_gpu: true,
                priority: 0,
                nice_value: 0,
                gpu_utilization: 0.0,
                gpu_mem_util: None,
                enc_util: None,
                dec_util: None,
            }]
        }
        fn collect_storage(&self) -> Vec<StorageInfo> {
            Vec::new()
        }
    }

    fn gpu(uuid: &str, device_type: &str) -> GpuInfo {
        GpuInfo {
            uuid: uuid.to_string(),
            time: String::new(),
            name: device_type.to_string(),
            device_type: device_type.to_string(),
            host_id: "test-host".to_string(),
            hostname: "test-host".to_string(),
            instance: "test-host".to_string(),
            utilization: 0.0,
            ane_utilization: 0.0,
            dla_utilization: None,
            tensorcore_utilization: None,
            temperature: 0,
            used_memory: 0,
            total_memory: 0,
            frequency: 0,
            power_consumption: 0.0,
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
            detail: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn snapshot_process_index_uses_combined_gpu_npu_order() {
        let snapshot = collect_once(
            Arc::new(XdnaCollector),
            &SnapshotIncludes {
                gpu: true,
                process: true,
                ..Default::default()
            },
            Duration::from_secs(1),
        )
        .await;
        let process = &snapshot.processes.unwrap()[0];
        assert_eq!(process.device_uuid, "AMD-XDNA-0000:c3:00.1");
        assert_eq!(process.device_id, 1);
    }
}
