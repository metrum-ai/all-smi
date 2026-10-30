<!-- Copyright (c) 2026 Metrum AI. All rights reserved. -->

# Windows NPU discovery and performance counters

## Implementation

DXCore discovers NPU hardware adapters independently of DXGI, using the NPU hardware-type attribute; on systems without that attribute, the generic machine-learning attribute is used while excluding graphics adapters. Each adapter's LUID joins it to the same persistent PDH query used by the Windows GPU readers. NPU device utilization is the maximum engine utilization after summing each engine's process shares. Shared and dedicated adapter memory usage are summed; process memory and engine samples are attributed by LUID and pid. The first utilization sample is unavailable because PDH rate counters require two collections. NPU LUIDs are excluded from DXGI GPU augmentation and identified WMI NPU rows from the GPU lists.

On AMD hardware, an installed Windows `xrt-smi` can provide best-effort firmware, XRT and platform details, and a driver-version fallback when DXCore does not provide one. The tool is located under the Windows system root; each command has a five-second timeout, and results or failures are cached for at least a minute. Failure or absence does not prevent discovery. The initial hardware build showed `0.0.0.0` for an unavailable DXCore driver version; the subsequent build omits that placeholder and uses XRT's version if available. The `windows.npu` doctor check reports DXCore discovery, PDH availability and whether the tool is present.

The fork's existing convention exports common utilization and memory for *all* NPU devices using `all_smi_gpu_*`, with `type="NPU"` on the device info series. Linux XDNA uses that same convention, plus an `all_smi_amdxdna_firmware_info` gauge when firmware is known. Windows XRT-enriched AMD NPUs now use that same vendor-specific firmware gauge; no existing common metric family is renamed. The separately exported `all_smi_gpu_windows_pdh_utilization` gauge preserves the underlying PDH engine reading when a vendor reader overwrites the headline utilization.

## Hardware verification

A Ryzen AI 9 HX PRO 475 test laptop (Radeon 890M and XDNA 2) ran build `721c1d8` on 2026-10-30. `windows.npu` passed. Four API scrapes per phase are paired below with the nearest reference PDH sample (at most 0.62 s away). Each cell is **all-smi / reference**, utilization in percent and used memory in MiB. The reference rounds to whole MiB. The iGPU reference is the maximum of its 3D and compute engine samples and shared memory, consistent with the GPU reader's selected pool.

| Phase, scrape | NPU utilization | iGPU utilization | NPU memory | iGPU memory |
|---|---:|---:|---:|---:|
| Idle 1 | 0 / 0 | 0 / 0 | 3010.2 / 3010 | 81.6 / 82 |
| Idle 2 | 0 / 0 | 0 / 0 | 3010.2 / 3010 | 81.6 / 82 |
| Idle 3 | 0 / 0 | 0 / 0 | 3010.2 / 3010 | 81.6 / 82 |
| Idle 4 | 0 / 0 | 0 / 0 | 3010.2 / 3010 | 81.6 / 82 |
| NPU decisions 1 | 59.5 / 60 | 0 / 0 | 4136.2 / 4136 | 81.6 / 82 |
| NPU decisions 2 | 59.8 / 60 | 0 / 0 | 4136.2 / 4136 | 81.6 / 82 |
| NPU decisions 3 | 61.0 / 61 | 0 / 0 | 4136.2 / 4136 | 81.6 / 82 |
| NPU decisions 4 | 60.8 / 61 | 0 / 0 | 4136.2 / 4136 | 81.6 / 82 |
| NPU chat 1 | 99.0 / 100 | 0 / 0 | 28421.7 / 28422 | 81.6 / 82 |
| NPU chat 2 | 100 / 99 | 0 / 0 | 28421.7 / 28422 | 81.6 / 82 |
| NPU chat 3 | 100 / 99 | 0 / 0 | 28421.7 / 28422 | 81.6 / 82 |
| NPU chat 4 | 97.9 / 99 | 0 / 0 | 28421.7 / 28422 | 81.6 / 82 |
| iGPU chat 1 | 0 / 0 | 96 / 89 | 3010.2 / 3010 | 21958.7 / 21959 |
| iGPU chat 2 | 0 / 0 | 97 / 90 | 3010.2 / 3010 | 21958.7 / 21959 |
| iGPU chat 3 | 0 / 0 | 97 / 89 | 3010.2 / 3010 | 21958.7 / 21959 |
| iGPU chat 4 | 0 / 0 | 96 / 89 | 3010.2 / 3010 | 21958.7 / 21959 |
| CPU chat 1 | 0 / 0 | 0 / 0 | 3010.2 / 3010 | 82.2 / 82 |
| CPU chat 2 | 0 / 0 | 0 / 0 | 3010.2 / 3010 | 82.2 / 82 |
| CPU chat 3 | 0 / 0 | 0 / 0 | 3010.2 / 3010 | 82.2 / 82 |
| CPU chat 4 | 0 / 0 | 0 / 0 | 3010.2 / 3010 | 82.2 / 82 |

NPU utilization tracks the reference within 1.1 percentage points in these samples; its idle memory is nonzero because Windows processes retain allocations. Snapshot process rows included six NPU processes at idle and seven during each NPU workload; the largest NPU process allocation was about 24.8 GiB during NPU chat. The iGPU chat snapshot showed 17 processes with nonzero GPU memory, including one at about 21.4 GiB. Reference data did not include per-process counters, so process values are availability evidence, not an independent per-process accuracy check. Capacity is from DXCore/DXGI and was not independently compared in this capture.

The iGPU headline utilization under load differs by 7–8 points: the snapshot identifies ADL, not PDH, as its source, because the existing vendor layer overrides PDH. Thus this capture demonstrates load response but **does not independently validate the application's iGPU PDH utilization**. A subsequent build retains the underlying PDH value in a separate gauge for direct verification. The measurement windows are not synchronized, so short-term differences are also possible.

A second run of `0856974` confirmed `type="NPU"`, XRT firmware `1.1.2.64`, and driver version `32.0.20102.3930`. The table pairs all four scrapes with their nearest reference samples (maximum timestamp difference 0.54 s). Each comma-separated list is in scrape order; `/` separates all-smi from reference. Memory is in MiB. ADL is the application's separate headline iGPU utilization.

| Phase | NPU utilization | iGPU PDH utilization | iGPU ADL | NPU memory | iGPU memory |
|---|---|---|---|---|---|
| Idle | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 | 3074.0 / 3074 (all four) | 89.6 / 90 (all four) |
| NPU decisions | 60.0, 60.3, 60.5, 60.2 / 60, 60, 60, 60 | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 | 4200.0 / 4200 (all four) | 89.6 / 90 (all four) |
| NPU chat | 100, 98.7, 100, 98.3 / 99, 100, 98, 98 | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 1 | 28421.7 / 28422 (all four) | 89.6 / 90 (all four) |
| iGPU chat | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 / 94, 90, 87, 87 | 97, 97, 96, 95 | 3010.2 / 3010 (all four) | 21966.7 / 21967 (all four) |
| CPU chat | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 | 3010.2 / 3010 (all four) | 90.2 / 90 (all four) |

The second run exposed a real defect: the reference names the active engine `compute 0`, while the GPU filter accepted only the exact engine type `Compute`. The fix accepts numbered Compute and 3D instances without counting unrelated engine types; a concurrent-consumer test verifies GPU and NPU readers share one coalesced collection. One-shot `doctor` reports utilization unavailable before the second rate sample; that is not by itself evidence of a LUID mismatch.

A third run of `b81624d` verified the fix. The same nearest-sample method has a maximum timestamp difference of 0.54 s. The table retains all four scrape pairs in order:

| Phase | NPU utilization | iGPU PDH utilization | iGPU ADL | NPU memory | iGPU memory |
|---|---|---|---|---|---|
| Idle | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 | 3010.2 / 3010 (all four) | 89.6 / 90 (all four) |
| NPU decisions | 59.0, 60.1, 59.6, 59.1 / 60, 59, 61, 60 | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 | 4136.2 / 4136 (all four) | 89.6 / 90 (all four) |
| NPU chat | 100, 98.8, 99.2, 99.6 / 99, 100, 97, 100 | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 | 28421.7 / 28422 (all four) | 89.6 / 90 (all four) |
| iGPU chat | 0, 0, 0, 0 / 0, 0, 0, 0 | 93.1, 88.1, 91.7, 76.5 / 87, 95, 92, 88 | 97, 97, 97, 93 | 3010.2 / 3010 (all four) | 21966.7 / 21967 (all four) |
| CPU chat | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 / 0, 0, 0, 0 | 0, 0, 0, 0 | 3010.2 / 3010 (all four) | 90.2 / 90 (all four) |

The corrected PDH gauge is nonzero under iGPU load and zero in the other phases, while the NPU remains idle under iGPU and CPU workloads. Individual iGPU PDH pairs differ by up to 11.5 percentage points; sampling windows are not synchronized, so the capture demonstrates load response, not exact instantaneous parity. ADL uses a different source and remains the headline value.

## Build and test evidence

On Linux, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` passed with Rust 1.99.1. `cargo-zigbuild` 0.24.0 with Zig 0.17.0 built the aarch64 GNU, x86-64 musl, and x86-64 GNU glibc 2.17 targets. `cargo-xwin` 0.23.1 passed an MSVC target release build and clippy at `b81624d`, using Clang 20.1.2 and rustup LLVM tooling. The target has not yet been built or tested natively on Windows. The laptop counter-name examples and synthetic per-engine aggregation fixtures are covered by platform-independent unit tests. Hardware verification remains pending; unit tests do not establish driver or device compatibility.
