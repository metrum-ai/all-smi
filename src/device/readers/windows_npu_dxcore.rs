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

//! DXCore compute-only adapter discovery. No COM initialization is needed.

use super::windows_gpu_perf::ids::AdapterLuid;
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::DXCore::*;
use windows::core::GUID;

// Published in the DirectX-Headers dxcore_interface.h; these newer GUIDs
// are not present in the current windows-rs Win32 metadata.
const HARDWARE_NPU: GUID = GUID::from_u128(0xd46140c4_add7_451b_9e56_06fe8c3b58ed);
const GENERIC_ML: GUID = GUID::from_u128(0xb71b0d41_1088_422f_a27c_0250b7d3a988);

#[derive(Clone, Debug)]
pub struct NpuAdapter {
    pub luid: AdapterLuid,
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub driver_version: u64,
    pub dedicated_memory: u64,
    pub shared_memory: u64,
}

fn property<T: Default>(adapter: &IDXCoreAdapter, key: DXCoreAdapterProperty) -> Option<T> {
    let mut value = T::default();
    // The caller supplies the exact type of the requested DXCore property.
    unsafe {
        adapter
            .GetProperty(key, std::mem::size_of::<T>(), (&mut value as *mut T).cast())
            .ok()?;
    }
    Some(value)
}

fn description(adapter: &IDXCoreAdapter) -> Option<String> {
    unsafe {
        let size = adapter.GetPropertySize(DriverDescription).ok()?;
        if size == 0 || size > 65536 {
            return None;
        }
        let mut data = vec![0u8; size];
        adapter
            .GetProperty(DriverDescription, size, data.as_mut_ptr().cast())
            .ok()?;
        let end = data
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(data.len());
        String::from_utf8(data[..end].to_vec()).ok()
    }
}

fn enumerate_for(
    factory: &IDXCoreAdapterFactory,
    attribute: GUID,
    fallback: bool,
) -> Vec<NpuAdapter> {
    unsafe {
        let Ok(list) = factory.CreateAdapterList::<IDXCoreAdapterList>(&[attribute]) else {
            return Vec::new();
        };
        (0..list.GetAdapterCount())
            .filter_map(|index| {
                let adapter = list.GetAdapter::<IDXCoreAdapter>(index).ok()?;
                if fallback
                    && (adapter.IsAttributeSupported(&DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS)
                        || adapter.IsAttributeSupported(&DXCORE_ADAPTER_ATTRIBUTE_D3D11_GRAPHICS))
                {
                    return None;
                }
                let luid: LUID = property(&adapter, InstanceLuid)?;
                let ids: DXCoreHardwareID = property(&adapter, HardwareID)?;
                Some(NpuAdapter {
                    luid: AdapterLuid {
                        high: luid.HighPart,
                        low: luid.LowPart,
                    },
                    name: description(&adapter)?,
                    vendor: ids.vendorID,
                    device: ids.deviceID,
                    driver_version: property(&adapter, DriverVersion).unwrap_or_default(),
                    dedicated_memory: property(&adapter, DedicatedAdapterMemory)
                        .unwrap_or_default(),
                    shared_memory: property(&adapter, SharedSystemMemory).unwrap_or_default(),
                })
            })
            .collect()
    }
}

pub fn enumerate() -> Vec<NpuAdapter> {
    let Ok(factory) = (unsafe { DXCoreCreateAdapterFactory::<IDXCoreAdapterFactory>() }) else {
        return Vec::new();
    };
    let adapters = enumerate_for(&factory, HARDWARE_NPU, false);
    if adapters.is_empty() {
        enumerate_for(&factory, GENERIC_ML, true)
    } else {
        adapters
    }
}
