<!-- Copyright (c) 2026 Metrum AI. All rights reserved. -->

# Windows NPU discovery and performance counters

## Implementation

DXCore discovers NPU hardware adapters independently of DXGI, using the NPU hardware-type attribute; on systems without that attribute, the generic machine-learning attribute is used while excluding graphics adapters. Each adapter's LUID joins it to the same persistent PDH query used by the Windows GPU readers. NPU device utilization is the maximum engine utilization after summing each engine's process shares. Shared and dedicated adapter memory usage are summed; process memory and engine samples are attributed by LUID and pid. The first utilization sample is unavailable because PDH rate counters require two collections. NPU LUIDs are excluded from DXGI GPU augmentation and identified WMI NPU rows from the GPU lists.

On AMD hardware, an installed Windows `xrt-smi` can provide best-effort firmware, XRT and platform details. Both commands are cached for at least a minute; failure or absence does not prevent discovery. The `windows.npu` doctor check reports DXCore discovery, PDH availability and whether the tool is present.

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

## Build and test evidence

On Linux, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` passed with Rust 1.99.1. `cargo-zigbuild` 0.24.0 with Zig 0.17.0 built the aarch64 GNU, x86-64 musl, and x86-64 GNU glibc 2.17 targets. `cargo-xwin` 0.23.1 passed an MSVC target release build and clippy at `328a56e`, using Clang 20.1.2 and rustup LLVM tooling. The target has not yet been built or tested natively on Windows. The laptop counter-name examples and synthetic per-engine aggregation fixtures are covered by platform-independent unit tests. Hardware verification remains pending; unit tests do not establish driver or device compatibility.
