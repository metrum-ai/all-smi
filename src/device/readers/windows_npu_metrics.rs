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

//! Pure NPU PDH memory and process aggregation.

use super::windows_gpu_perf::ids::{AdapterLuid, GpuProcessMemoryInstance};
use std::collections::HashMap;

pub fn adapter_memory(dedicated: Option<u64>, shared: Option<u64>) -> Option<u64> {
    match (dedicated, shared) {
        (None, None) => None,
        (dedicated, shared) => Some(dedicated.unwrap_or(0).saturating_add(shared.unwrap_or(0))),
    }
}

pub fn process_rows<'a>(
    luid: AdapterLuid,
    memory: impl IntoIterator<Item = &'a (GpuProcessMemoryInstance, u64)>,
    utilization: &HashMap<(AdapterLuid, u32), f64>,
) -> Vec<(u32, u64, Option<f64>)> {
    let mut rows: HashMap<u32, (u64, Option<f64>)> = HashMap::new();
    for (instance, bytes) in memory {
        if instance.luid == luid {
            let entry = rows.entry(instance.pid).or_default();
            entry.0 = entry.0.saturating_add(*bytes);
        }
    }
    for (&(row_luid, pid), &value) in utilization {
        if row_luid == luid {
            rows.entry(pid).or_default().1 = Some(value);
        }
    }
    let mut rows: Vec<_> = rows
        .into_iter()
        .map(|(pid, (bytes, util))| (pid, bytes, util))
        .collect();
    rows.sort_by_key(|row| row.0);
    rows
}

#[cfg(test)]
mod tests {
    use super::super::windows_gpu_perf::ids::parse_gpu_process_memory_instance;
    use super::*;

    #[test]
    fn npu_laptop_memory_and_process_isolation() {
        let npu = AdapterLuid::new(0, 0x11a8f);
        let gpu = AdapterLuid::new(0, 0xfc61);
        let instance =
            parse_gpu_process_memory_instance("pid_7740_luid_0x00000000_0x00011a8f_phys_0")
                .unwrap();
        let memory = vec![
            (instance.clone(), 2399 * 1024 * 1024),
            (instance, 4),
            (
                GpuProcessMemoryInstance {
                    pid: 7740,
                    luid: gpu,
                    phys: 0,
                },
                17,
            ),
        ];
        let util = HashMap::from([((npu, 7740), 19.0), ((npu, 123), 4.0), ((gpu, 999), 90.0)]);
        assert_eq!(
            process_rows(npu, &memory, &util),
            vec![
                (123, 0, Some(4.0)),
                (7740, 2399 * 1024 * 1024 + 4, Some(19.0))
            ]
        );
        assert_eq!(adapter_memory(None, None), None);
        assert_eq!(adapter_memory(Some(7), Some(9)), Some(16));
        assert_eq!(adapter_memory(Some(u64::MAX), Some(9)), Some(u64::MAX));
    }
}
