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

//! Metrics exported for AMD XDNA NPUs.

use super::exporter_trait::NpuExporter;
use crate::api::metrics::MetricBuilder;
use crate::device::GpuInfo;

pub struct AmdXdnaExporter;

impl AmdXdnaExporter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AmdXdnaExporter {
    fn default() -> Self {
        Self::new()
    }
}

impl NpuExporter for AmdXdnaExporter {
    fn can_handle(&self, info: &GpuInfo) -> bool {
        info.device_type == "NPU"
            && (info.detail.get("driver").is_some_and(|d| d == "amdxdna")
                || (info
                    .detail
                    .get("driver")
                    .is_some_and(|d| d == "windows_dxcore")
                    && info
                        .detail
                        .get("pci_vendor_id")
                        .is_some_and(|v| v == "1022" || v == "1002")
                    && info.detail.contains_key("xrt_version")))
    }

    fn export_vendor_metrics(
        &self,
        builder: &mut MetricBuilder,
        info: &GpuInfo,
        _index: usize,
        index_str: &str,
    ) {
        if let Some(firmware) = info.detail.get("firmware_version") {
            let firmware_labels = [
                ("npu", info.name.as_str()),
                ("instance", info.instance.as_str()),
                ("npu_uuid", info.uuid.as_str()),
                ("npu_index", index_str),
                ("version", firmware.as_str()),
            ];
            builder
                .help("all_smi_amdxdna_firmware_info", "AMD XDNA firmware version")
                .type_("all_smi_amdxdna_firmware_info", "gauge")
                .metric("all_smi_amdxdna_firmware_info", &firmware_labels, 1);
        }

        if let Some(slot) = info.detail.get("pci_slot_name") {
            let slot_labels = [
                ("npu", info.name.as_str()),
                ("instance", info.instance.as_str()),
                ("npu_uuid", info.uuid.as_str()),
                ("npu_index", index_str),
                ("pci_slot", slot.as_str()),
            ];
            builder
                .help("all_smi_amdxdna_device_info", "AMD XDNA device identity")
                .type_("all_smi_amdxdna_device_info", "gauge")
                .metric("all_smi_amdxdna_device_info", &slot_labels, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn npu(detail: &[(&str, &str)]) -> GpuInfo {
        GpuInfo {
            uuid: "AMD-XDNA-0000:c3:00.1".to_string(),
            time: "2026-10-27 00:00:00".to_string(),
            name: "NPU Strix Halo".to_string(),
            device_type: "NPU".to_string(),
            host_id: "test".to_string(),
            hostname: "test".to_string(),
            instance: "test".to_string(),
            utilization: -1.0,
            ane_utilization: -1.0,
            dla_utilization: None,
            tensorcore_utilization: None,
            temperature: 0,
            used_memory: 0,
            total_memory: 0,
            frequency: 0,
            power_consumption: -1.0,
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
            detail: detail
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    #[test]
    fn routes_windows_amd_xrt_without_claiming_unknown_adapters() {
        let exporter = AmdXdnaExporter::new();
        let device = npu(&[
            ("driver", "windows_dxcore"),
            ("pci_vendor_id", "1022"),
            ("xrt_version", "2.21.0"),
            ("firmware_version", "1.1.2.64"),
        ]);
        assert!(exporter.can_handle(&device));
        let mut output = MetricBuilder::new();
        exporter.export_vendor_metrics(&mut output, &device, 0, "0");
        assert!(output.build().contains("all_smi_amdxdna_firmware_info"));
        assert!(!exporter.can_handle(&npu(&[
            ("driver", "windows_dxcore"),
            ("pci_vendor_id", "8086"),
            ("xrt_version", "2.21.0")
        ])));
        assert!(!exporter.can_handle(&npu(&[
            ("driver", "windows_dxcore"),
            ("pci_vendor_id", "1022")
        ])));
    }

    #[test]
    fn routes_amdxdna_and_exports_firmware_identity() {
        let npu = npu(&[("driver", "amdxdna"), ("firmware_version", "1.1.2.65")]);
        let exporter = AmdXdnaExporter::new();
        assert!(exporter.can_handle(&npu));
        let mut builder = MetricBuilder::new();
        exporter.export_vendor_metrics(&mut builder, &npu, 0, "0");
        let output = builder.build();
        assert!(output.contains("all_smi_amdxdna_firmware_info"));
        assert!(output.contains("version=\"1.1.2.65\""));
        assert!(!output.contains("utilization"));
    }

    #[test]
    fn does_not_claim_unknown_xdna_metrics() {
        let npu = npu(&[("driver", "amdxdna")]);
        let mut builder = MetricBuilder::new();
        AmdXdnaExporter::new().export_vendor_metrics(&mut builder, &npu, 0, "0");
        assert!(builder.build().is_empty());
    }
}
