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

//! `amd.*` checks: ROCm, the runtime-loaded Linux AMD plugin, DRI access,
//! the musl target gate, and Windows ADL diagnostics.

#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use crate::doctor::exec::try_exec;
use crate::doctor::types::{Check, CheckCtx, CheckResult, Severity};

static CHECKS: &[&Check] = &[
    &ROCM_VERSION,
    &RAPL_ACCESS,
    &UMC_BANDWIDTH,
    &LIBAMDGPU_TOP_ABI,
    &DRI_PERMS,
    &BUILD_GATE,
    &ADL_LIBRARY,
    &ADL_SENSORS,
    &ADL_ADAPTERS,
    &ADL_PER_ADAPTER,
];

pub fn checks() -> &'static [&'static Check] {
    CHECKS
}

static ROCM_VERSION: Check = Check {
    id: "amd.rocm.version",
    title: "ROCm version",
    severity_on_fail: Severity::Warn,
    run: check_rocm,
};

static RAPL_ACCESS: Check = Check {
    id: "amd.rapl.access",
    title: "RAPL energy-counter access",
    severity_on_fail: Severity::Info,
    run: check_rapl_access,
};

static UMC_BANDWIDTH: Check = Check {
    id: "amd.umc.bandwidth",
    title: "System memory bandwidth access",
    severity_on_fail: Severity::Info,
    run: check_umc_bandwidth,
};

static LIBAMDGPU_TOP_ABI: Check = Check {
    id: "amd.libamdgpu_top.abi",
    title: "AMD runtime plugin",
    severity_on_fail: Severity::Warn,
    run: check_libamdgpu_top,
};

static DRI_PERMS: Check = Check {
    id: "amd.dri.perms",
    title: "/dev/dri permissions",
    severity_on_fail: Severity::Warn,
    run: check_dri_perms,
};

// The check id stays `amd.build.target_env` for compatibility with existing
// bundles even though the cargo feature is now an accepted no-op.
static BUILD_GATE: Check = Check {
    id: "amd.build.target_env",
    title: "AMD build-time availability",
    severity_on_fail: Severity::Warn,
    run: check_build_gate,
};

static ADL_LIBRARY: Check = Check {
    id: "amd.adl.library",
    title: "AMD ADL library (atiadlxx.dll)",
    severity_on_fail: Severity::Info,
    run: check_adl_library,
};

static ADL_SENSORS: Check = Check {
    id: "amd.adl.sensors",
    title: "AMD ADL PMLog sensors",
    severity_on_fail: Severity::Info,
    run: check_adl_sensors,
};

static ADL_ADAPTERS: Check = Check {
    id: "amd.adl.adapters",
    title: "AMD ADL adapter inventory",
    severity_on_fail: Severity::Info,
    run: check_adl_adapters,
};

static ADL_PER_ADAPTER: Check = Check {
    id: "amd.adl.per_adapter",
    title: "AMD ADL per-adapter PMLog sampling",
    severity_on_fail: Severity::Info,
    run: check_adl_per_adapter,
};

/// Dump the raw `AdapterInfo` rows: index, PCI bus/device/function,
/// `strAdapterName`, and `strPNPString` per adapter.
///
/// This is the field-verification path for the `AdapterInfo` layout in
/// `device/readers/amd_adl/ffi.rs`, the same role `amd.adl.sensors`
/// plays for the PMLog sensor index mapping: the layout is transcribed
/// from AMD's public headers and nothing in CI can check it (no job
/// compiles all-smi for Windows, no test can call the real library),
/// so an operator on real hardware is the verifier. Legible device
/// paths in the dump confirm the layout; garbage refutes it, which is
/// why the rows are printed even, and especially, when runtime
/// verification fails, and why each row carries its `RowState` tag:
/// BLANK (driver memset, healthy filtering) versus UNTOUCHED (poison
/// intact, short write) is exactly the distinction a real-hardware
/// report needs to settle. The dump also shows the grouping and the
/// PNP strings that multi-GPU attribution matches against the GPU
/// uuids, so a wrong match can be diagnosed from the same output.
fn check_adl_adapters(_ctx: &CheckCtx) -> CheckResult {
    #[cfg(target_os = "windows")]
    {
        use crate::device::readers::amd_adl::loader::AdapterProbe;
        use crate::device::readers::amd_adl::{adapters, loader};

        let Some(probe) = loader::adapter_info_probe() else {
            return CheckResult::Skip("ADL unavailable (see amd.adl.library)".to_string());
        };
        match probe {
            AdapterProbe::NoEntryPoint => CheckResult::Skip(
                "atiadlxx.dll does not export ADL2_Adapter_AdapterInfo_Get; multi-GPU \
                 attribution is unavailable on this driver"
                    .to_string(),
            ),
            AdapterProbe::CallFailed => CheckResult::Warn(
                "ADL2_Adapter_AdapterInfo_Get failed or reported an implausible adapter count"
                    .to_string(),
                Some("single-GPU sensor augmentation is unaffected".to_string()),
            ),
            AdapterProbe::Rows { rows, accepted } => {
                // Every row renders with its RowState tag (POPULATED
                // rows untagged, BLANK / UNTOUCHED / GARBLED named), so
                // a real-hardware dump says decisively whether a
                // non-populated row was memset by the driver or never
                // written; that distinction is what the poison pre-fill
                // exists for.
                let dump = rows
                    .iter()
                    .enumerate()
                    .map(|(slot, row)| adapters::describe_raw_entry(slot, row))
                    .collect::<Vec<_>>()
                    .join("; ");
                let Some(populated) = accepted else {
                    // The failure shapes point at different
                    // corrections, so name the one seen. See
                    // `adapters::describe_layout_failure`, which is
                    // where this is tested: this whole function is
                    // Windows-gated and cannot run on the Linux runner.
                    let shape = adapters::describe_layout_failure(&rows);
                    return CheckResult::Warn(
                        format!(
                            "AdapterInfo layout verification FAILED; multi-GPU attribution is \
                             disabled. {shape}. raw rows: {dump}"
                        ),
                        Some(
                            "please report the raw rows so the transcribed layout in \
                             device/readers/amd_adl/ffi.rs can be corrected"
                                .to_string(),
                        ),
                    );
                };
                let parsed = adapters::parse_adapters(&populated);
                let groups = adapters::group_by_card(&parsed);
                CheckResult::Pass(format!(
                    "{} adapter row(s), {} populated, across {} physical card(s); {dump}",
                    rows.len(),
                    populated.len(),
                    groups.len(),
                ))
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        CheckResult::Skip("ADL is Windows-only".to_string())
    }
}

/// Render an `AdlReadout` the one way both ADL sensor checks print it.
///
/// `amd.adl.sensors` reports the scanned adapter and `amd.adl.per_adapter`
/// reports every adapter index. A field-verification dump is only
/// comparable across the two if they render the same struct identically,
/// so the formatting lives here rather than at each call site.
#[cfg(target_os = "windows")]
fn describe_readout(readout: &crate::device::readers::amd_adl::sensors::AdlReadout) -> String {
    // `temperature_gfx_c` is printed because it is the second link in
    // `primary_temperature_c`'s chain (edge, then gfx, then hotspot), and
    // omitting it made this dump contradict the reading all-smi actually
    // publishes. A Strix Halo APU reports none of edge, mem or hotspot and
    // does report gfx, so the summary said `edge=None hotspot=None mem=None`
    // while the TUI correctly showed 41 C from the gfx sensor. The raw table
    // below carried the answer at index 28 all along, which is the opposite
    // of what a field-verification dump is for: it should not take reading
    // the source to reconcile the interpreted line with the product.
    format!(
        "edge={:?}C gfx={:?}C hotspot={:?}C mem={:?}C power={:?}W fan={:?}rpm \
         gfxclk={:?}MHz mclk={:?}MHz activity={:?}%",
        readout.temperature_edge_c,
        readout.temperature_gfx_c,
        readout.temperature_hotspot_c,
        readout.temperature_mem_c,
        readout.power_w,
        readout.fan_rpm,
        readout.clock_gfx_mhz,
        readout.clock_mem_mhz,
        readout.activity_gfx_pct,
    )
}

/// Sample PMLog against every ADL adapter index, grouped by physical card.
///
/// This is the field-verification path for the multi-GPU attribution
/// machinery, the same role `amd.adl.sensors` plays for the sensor index
/// mapping and `amd.adl.adapters` plays for the `AdapterInfo` layout.
/// Three pieces of that machinery are both `cfg(target_os = "windows")`
/// and unreachable on a single-GPU host, because `plan_attribution` takes
/// its `SoleGpu` arm before any of them execute: `loader::adapter_inventory`,
/// `loader::sample_adapter`, and the `PerCard` arm of `augment`. Nothing in
/// CI reaches them either, since no job compiles all-smi for Windows. This
/// check puts the first two on a reachable code path and prints what the
/// third would consume.
///
/// The question it exists to answer is whether two physically distinct
/// cards report distinct telemetry, the outstanding acceptance criterion
/// tracked in issue #370. With two or more cards the summary states the
/// verdict outright. With one card it says so instead of implying a
/// result, because several indices of a single card agreeing is not
/// evidence either way about two cards.
fn check_adl_per_adapter(_ctx: &CheckCtx) -> CheckResult {
    #[cfg(target_os = "windows")]
    {
        use crate::device::readers::amd_adl::{adapters, loader, sensors};

        let Some(inventory) = loader::adapter_inventory() else {
            // `adapter_inventory` collapses no-library, no-entry-point,
            // failed-call, and failed-layout-verification into one
            // `None`. `amd.adl.adapters` is the check that separates
            // them, so point there rather than guessing here.
            return CheckResult::Skip(
                "no validated adapter inventory; see amd.adl.adapters for which stage declined"
                    .to_string(),
            );
        };

        let groups = adapters::group_by_card(&inventory);
        if groups.is_empty() {
            return CheckResult::Warn(
                format!(
                    "{} adapter row(s) grouped into 0 physical card(s), so multi-GPU attribution \
                     would decline",
                    inventory.len()
                ),
                Some(
                    "please report the amd.adl.adapters dump; the grouping rules in \
                     device/readers/amd_adl/adapters.rs disagree with this driver"
                        .to_string(),
                ),
            );
        }

        // One readout per card, in card order, for the distinctness
        // verdict below. `None` for a card no index answered for.
        let mut per_card_first: Vec<Option<String>> = Vec::new();
        let mut answered = 0usize;
        let mut attempted = 0usize;
        let mut blocks = Vec::new();

        for (slot, group) in groups.iter().enumerate() {
            let mut first_seen: Option<String> = None;
            let mut rows = Vec::new();
            for &index in &group.indices {
                attempted += 1;
                let Some(output) = loader::sample_adapter(index) else {
                    // `augment` takes the first index of a card that
                    // answers, so a silent index is survivable but is
                    // exactly what makes that choice order-dependent.
                    // Name it rather than omitting the row.
                    rows.push(format!("[{index}] no PMLog answer"));
                    continue;
                };
                answered += 1;
                let raw = sensors::supported_raw(&output);
                let readout = sensors::extract(&output);
                let interpreted = describe_readout(&readout);
                if first_seen.is_none() {
                    first_seen = Some(interpreted.clone());
                }
                let dump = raw
                    .iter()
                    .map(|(sensor, value)| format!("{sensor}={value}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                rows.push(format!(
                    "[{index}] {} sensor(s); {interpreted}; raw: {dump}",
                    raw.len()
                ));
            }
            per_card_first.push(first_seen);
            blocks.push(format!(
                "card {slot} bus={} device={} function={} name={:?} pnp={:?} -> {}",
                group.bus,
                group.device,
                group.function,
                group.adapter_name,
                group.pnp_string,
                rows.join(", ")
            ));
        }

        let dump = blocks.join(" | ");
        if answered == 0 {
            return CheckResult::Warn(
                format!(
                    "no PMLog answer from any of {attempted} adapter index(es) across {} card(s), \
                     so multi-GPU attribution would find no card to sample. {dump}",
                    groups.len()
                ),
                Some(
                    "expected on pre-Vega cards, whose sensors live behind the legacy Overdrive \
                     5/6/7 entry points that all-smi does not implement; otherwise please report \
                     this dump"
                        .to_string(),
                ),
            );
        }

        let verdict = per_card_verdict(&per_card_first, groups.len());

        CheckResult::Pass(format!(
            "{answered}/{attempted} adapter index(es) answered across {} card(s); {verdict}. \
             {dump}",
            groups.len()
        ))
    }
    #[cfg(not(target_os = "windows"))]
    {
        CheckResult::Skip("ADL is Windows-only".to_string())
    }
}

/// The one sentence `amd.adl.per_adapter` exists to produce: do distinct
/// physical cards report distinct telemetry?
///
/// `per_card_first` holds the first answering readout per card, in card
/// order, with `None` for a card that stayed silent. Comparison is on the
/// rendered string rather than field by field, so "identical" means
/// exactly what an operator reads off the dump.
///
/// Split out from the check body so it is testable on every platform: the
/// check itself is Windows-gated and cannot run on the Linux runner, which
/// is the same reason `adapters::describe_layout_failure` is a free
/// function.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn per_card_verdict(per_card_first: &[Option<String>], card_count: usize) -> String {
    if card_count < 2 {
        return "1 physical card, so per-card differentiation is unobservable on this host; \
                at least 2 physical cards must answer to verify distinct per-card telemetry"
            .to_string();
    }
    let seen: Vec<&String> = per_card_first.iter().flatten().collect();
    if seen.len() < 2 {
        return "fewer than 2 cards answered, so per-card differentiation is undetermined"
            .to_string();
    }
    if seen.windows(2).all(|pair| pair[0] == pair[1]) {
        return "every card reported an IDENTICAL readout, which on distinct cards would mean the \
                driver is not separating per-card telemetry; please report this"
            .to_string();
    }
    "cards reported DISTINCT readouts, which is what per-card attribution needs".to_string()
}

/// Report whether `atiadlxx.dll` loaded and an ADL2 context was created.
fn check_adl_library(_ctx: &CheckCtx) -> CheckResult {
    #[cfg(target_os = "windows")]
    {
        use crate::device::readers::amd_adl::loader;

        let path = loader::dll_path();
        if !std::path::Path::new(path).exists() {
            return CheckResult::Skip(format!(
                "{path} not present; install AMD's graphics driver for temperature, power, fan, \
                 and clock readings"
            ));
        }
        if !loader::library_available() {
            return CheckResult::Warn(
                format!("{path} exists but ADL2_Main_Control_Create failed"),
                Some(
                    "the driver install may be partial; reinstalling AMD Software usually \
                     restores it"
                        .to_string(),
                ),
            );
        }
        match loader::selected_adapter_index() {
            Some(index) => CheckResult::Pass(format!(
                "{path} loaded, PMLog-capable adapter index {index}"
            )),
            None => CheckResult::Warn(
                format!("{path} loaded but no adapter exposes the PMLog sensor table"),
                Some(
                    "expected on pre-Vega cards, whose sensors live behind the legacy Overdrive \
                     5/6/7 entry points that all-smi does not implement"
                        .to_string(),
                ),
            ),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        CheckResult::Skip("ADL is Windows-only".to_string())
    }
}

/// Dump the PMLog sensor table as raw `index=value` pairs.
///
/// This is the field-verification path for the sensor index mapping in
/// `device/readers/amd_adl/sensors.rs`. Those indices are transcribed
/// from AMD's public headers and cannot be checked by anything in CI:
/// no job compiles all-smi for Windows and no test can call the real
/// library. Printing the raw table lets an operator on real hardware
/// confirm or correct the mapping without a code change shipping first.
///
/// The dump is deliberately unfiltered and shows indices rather than
/// names, because an unexpected index is exactly the evidence needed.
fn check_adl_sensors(_ctx: &CheckCtx) -> CheckResult {
    #[cfg(target_os = "windows")]
    {
        use crate::device::readers::amd_adl::{loader, sensors};

        let Some(output) = loader::sample() else {
            return CheckResult::Skip(
                "no PMLog sample available (see amd.adl.library)".to_string(),
            );
        };

        let raw = sensors::supported_raw(&output);
        if raw.is_empty() {
            return CheckResult::Warn(
                "PMLog returned a table with no supported sensors".to_string(),
                None,
            );
        }

        let readout = sensors::extract(&output);
        let dump = raw
            .iter()
            .map(|(index, value)| format!("{index}={value}"))
            .collect::<Vec<_>>()
            .join(" ");

        let interpreted = describe_readout(&readout);

        let count = raw.len();
        if readout.is_empty() {
            return CheckResult::Warn(
                format!(
                    "{count} sensor(s) reported supported but none passed the range guard. \
                     raw: {dump}"
                ),
                Some(
                    "this is what a shifted ADLSensorType enum looks like; please report the raw \
                     dump so the index mapping can be corrected"
                        .to_string(),
                ),
            );
        }

        CheckResult::Pass(format!(
            "{count} supported sensor(s); interpreted: {interpreted}; raw: {dump}"
        ))
    }
    #[cfg(not(target_os = "windows"))]
    {
        CheckResult::Skip("ADL is Windows-only".to_string())
    }
}

#[cfg(target_os = "linux")]
fn rocm_version_from_root(root: &std::path::Path) -> Option<(String, std::path::PathBuf)> {
    let direct_version_files = [
        root.join(".info/version"),
        root.join(".info/version-dev"),
        root.join("lib/rocm-release-info/version"),
    ];
    for path in direct_version_files {
        if let Some(result) = read_rocm_version_file(&path) {
            return Some(result);
        }
    }

    let root_header = root.join("include/rocm-core/rocm_version.h");
    if let Some(result) = read_rocm_version_header(&root_header) {
        return Some(result);
    }

    let mut cores = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (entry.file_name().to_string_lossy().starts_with("core") && path.is_dir())
                .then_some(path)
        })
        .collect::<Vec<_>>();
    sort_versioned_paths(&mut cores);
    for core in cores {
        let header = core.join("include/rocm-core/rocm_version.h");
        if let Some(result) = read_rocm_version_header(&header) {
            return Some(result);
        }
        if let Some(result) = read_rocm_version_file(&core.join(".info/version")) {
            return Some(result);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn read_rocm_version_file(path: &std::path::Path) -> Option<(String, std::path::PathBuf)> {
    let version = std::fs::read_to_string(path).ok()?;
    let version = version.trim();
    (!version.is_empty()).then(|| (version.to_string(), path.to_path_buf()))
}

#[cfg(target_os = "linux")]
fn read_rocm_version_header(path: &std::path::Path) -> Option<(String, std::path::PathBuf)> {
    let contents = std::fs::read_to_string(path).ok()?;
    parse_rocm_version_header(&contents).map(|version| (version, path.to_path_buf()))
}

#[cfg(target_os = "linux")]
fn version_sort_key(name: &str) -> Vec<u32> {
    name.split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

#[cfg(target_os = "linux")]
fn sort_versioned_paths(paths: &mut [std::path::PathBuf]) {
    paths.sort_by(|left, right| {
        let left_name = left.file_name().unwrap_or_default().to_string_lossy();
        let right_name = right.file_name().unwrap_or_default().to_string_lossy();
        version_sort_key(&right_name)
            .cmp(&version_sort_key(&left_name))
            .then_with(|| left.cmp(right))
    });
}

#[cfg(target_os = "linux")]
fn parse_rocm_version_header(contents: &str) -> Option<String> {
    let value = |key: &str| {
        contents.lines().find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()? == "#define" && words.next()? == key)
                .then(|| words.next()?.parse::<u32>().ok())
                .flatten()
        })
    };
    Some(format!(
        "{}.{}.{}",
        value("ROCM_VERSION_MAJOR")?,
        value("ROCM_VERSION_MINOR")?,
        value("ROCM_VERSION_PATCH")?
    ))
}

#[cfg(target_os = "linux")]
fn rocm_roots() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(path) = std::env::var_os("ROCM_PATH") {
        roots.push(path.into());
    }
    if let Ok(entries) = std::fs::read_dir("/opt") {
        let mut versioned = entries
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("rocm-")
                    .then(|| entry.path())
            })
            .collect::<Vec<_>>();
        sort_versioned_paths(&mut versioned);
        roots.extend(versioned);
    }
    roots.push("/opt/rocm".into());
    let mut seen = std::collections::HashSet::new();
    roots.retain(|root| seen.insert(root.clone()));
    roots
}

#[cfg(target_os = "linux")]
fn has_rocm_userspace_marker(root: &std::path::Path) -> bool {
    if root.join(".info").is_dir() || root.join("bin/rocminfo").is_file() {
        return true;
    }
    std::fs::read_dir(root.join("lib"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("librocm-core.so")
        })
}

#[cfg(target_os = "linux")]
fn is_rocm_root(root: &std::path::Path) -> bool {
    has_rocm_userspace_marker(root) || std::path::Path::new("/sys/module/amdgpu").exists()
}

#[cfg(target_os = "linux")]
fn parse_amd_smi_rocm_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (label, version) = line.split_once(':')?;
        (label.to_ascii_lowercase().contains("rocm") && !version.trim().is_empty())
            .then(|| format!("ROCm {}", version.trim()))
    })
}

#[cfg(target_os = "linux")]
fn parse_rocminfo_hsa_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (label, version) = line.split_once(':')?;
        (label.trim() == "Runtime Version" && !version.trim().is_empty())
            .then(|| format!("HSA runtime version {}", version.trim()))
    })
}

fn check_rapl_access(_ctx: &CheckCtx) -> CheckResult {
    #[cfg(target_os = "linux")]
    {
        use std::fs;

        let domains = fs::read_dir("/sys/class/powercap")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = fs::read_to_string(entry.path().join("name")).ok()?;
                name.trim().starts_with("package").then_some(entry.path())
            })
            .collect::<Vec<_>>();
        let readable = domains
            .iter()
            .filter(|domain| fs::File::open(domain.join("energy_uj")).is_ok())
            .count();
        describe_rapl_access(domains.len(), readable)
    }
    #[cfg(not(target_os = "linux"))]
    {
        CheckResult::Skip("RAPL powercap access checks are Linux-only".to_string())
    }
}

#[cfg(any(target_os = "linux", test))]
fn describe_rapl_access(domains: usize, readable: usize) -> CheckResult {
    if domains == 0 {
        return CheckResult::Skip(
            "RAPL energy counters unavailable: no package powercap domains were detected"
                .to_string(),
        );
    }
    if readable == domains {
        return CheckResult::Pass(format!(
            "RAPL energy counters are readable for all {domains} detected package domain(s)"
        ));
    }

    CheckResult::Skip(format!(
        "RAPL energy counters unavailable to this user: {readable}/{domains} package domain(s) are readable. On stock Linux systems energy counters may be root-only; an administrator may document and opt in to read access for a dedicated group, with the power side-channel risk noted. all-smi does not change host permissions or install access rules"
    ))
}

fn check_umc_bandwidth(_ctx: &CheckCtx) -> CheckResult {
    #[cfg(target_os = "linux")]
    {
        use std::fs;

        let umc_count = fs::read_dir("/sys/bus/event_source/devices")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.strip_prefix("amd_umc_").is_some_and(|suffix| {
                    !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
                })
            })
            .count();
        let paranoid = fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
            .ok()
            .and_then(|value| value.trim().parse::<i32>().ok());
        describe_umc_bandwidth_availability(umc_count, paranoid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        CheckResult::Skip("AMD UMC system-memory bandwidth checks are Linux-only".to_string())
    }
}

#[cfg(any(target_os = "linux", test))]
fn describe_umc_bandwidth_availability(umc_count: usize, paranoid: Option<i32>) -> CheckResult {
    if umc_count == 0 {
        return CheckResult::Skip(
            "System memory bandwidth unavailable: no amd_umc memory-controller PMUs were detected"
                .to_string(),
        );
    }

    let policy = paranoid.map_or_else(
        || "kernel.perf_event_paranoid could not be read".to_string(),
        |value| format!("kernel.perf_event_paranoid is {value}"),
    );
    CheckResult::Skip(format!(
        "System memory bandwidth unavailable: detected {umc_count} amd_umc PMUs, but this build does not sample their system-wide perf counters; access to these counters requires CAP_PERFMON or an administrator-managed perf policy ({policy}). all-smi does not change kernel policy or request capabilities"
    ))
}

fn check_rocm(_ctx: &CheckCtx) -> CheckResult {
    #[cfg(target_os = "linux")]
    {
        let roots = rocm_roots();
        for root in &roots {
            if let Some((version, source)) = rocm_version_from_root(root) {
                return CheckResult::Pass(format!("ROCm {version} ({})", source.display()));
            }
        }
        if let Some(out) = try_exec("amd-smi", &["version"], Duration::from_millis(2_500))
            && out.success()
            && let Some(version) = parse_amd_smi_rocm_version(&out.stdout)
        {
            return CheckResult::Pass(version);
        }
        if let Some(out) = try_exec("rocminfo", &[], Duration::from_millis(2_500))
            && out.success()
            && let Some(version) = parse_rocminfo_hsa_version(&out.stdout)
        {
            return CheckResult::Pass(version);
        }
        for root in roots {
            if root.exists() && is_rocm_root(&root) {
                return CheckResult::Pass(format!(
                    "ROCm present, version unknown ({})",
                    root.display()
                ));
            }
        }
        CheckResult::Skip("ROCm not detected (no installation path or runtime tool)".to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        CheckResult::Skip("ROCm is Linux-only".to_string())
    }
}

fn check_libamdgpu_top(_ctx: &CheckCtx) -> CheckResult {
    #[cfg(all(target_os = "linux", not(target_env = "musl")))]
    {
        use crate::device::readers::amd::{self, AmdBackendStatus};

        match amd::backend_status() {
            AmdBackendStatus::Loaded {
                path,
                abi_version,
                plugin_version,
                libamdgpu_top_version,
            } => CheckResult::Pass(format!(
                "loaded {} (plugin {plugin_version}, ABI v{abi_version}, libamdgpu_top {libamdgpu_top_version})",
                path.display()
            )),
            AmdBackendStatus::Unavailable { reason } => CheckResult::Warn(
                reason,
                Some(
                    "install the packaged AMD companion library, then rerun doctor; missing libdrm dependencies are reported by dlopen in this check"
                        .to_string(),
                ),
            ),
        }
    }
    #[cfg(all(target_os = "linux", target_env = "musl"))]
    {
        CheckResult::Skip(
            "AMD runtime plugin is unavailable in musl builds (see amd.build.target_env)"
                .to_string(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        CheckResult::Skip("libamdgpu_top is Linux-only".to_string())
    }
}

fn check_dri_perms(_ctx: &CheckCtx) -> CheckResult {
    #[cfg(target_os = "linux")]
    {
        let p = std::path::Path::new("/dev/dri");
        if !p.exists() {
            return CheckResult::Skip("/dev/dri missing".to_string());
        }
        match std::fs::read_dir(p) {
            Ok(iter) => {
                let nodes: Vec<String> = iter
                    .filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.starts_with("render") || n.starts_with("card"))
                    .collect();
                if nodes.is_empty() {
                    CheckResult::Skip("no card* or render* nodes".to_string())
                } else {
                    CheckResult::Pass(format!("{} node(s): {}", nodes.len(), nodes.join(", ")))
                }
            }
            Err(e) => CheckResult::Fail(
                format!("/dev/dri unreadable: {e}"),
                Some("ensure the caller is in the render group".to_string()),
            ),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        CheckResult::Skip("/dev/dri is Linux-only".to_string())
    }
}

/// Report whether this target can host the runtime AMD plugin.
///
/// The legacy `amd` cargo feature is intentionally irrelevant here: it is a
/// compatibility no-op, so `default-features = false` consumers get the same
/// loader as default builds without inheriting libdrm linkage.
fn check_build_gate(_ctx: &CheckCtx) -> CheckResult {
    #[cfg(target_env = "musl")]
    {
        CheckResult::Warn(
            "musl build — AMD runtime plugin loader is unavailable".to_string(),
            Some("use a glibc build (x86_64-unknown-linux-gnu) for AMD GPU monitoring".to_string()),
        )
    }
    #[cfg(all(target_os = "linux", not(target_env = "musl")))]
    {
        CheckResult::Pass(
            "glibc Linux build — AMD runtime loader compiled in; the `amd` cargo feature does not gate it"
                .to_string(),
        )
    }
    #[cfg(all(not(target_env = "musl"), not(target_os = "linux")))]
    {
        CheckResult::Pass("non-Linux target — Linux AMD runtime plugin not applicable".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{describe_rapl_access, describe_umc_bandwidth_availability, per_card_verdict};
    #[cfg(target_os = "linux")]
    use super::{
        has_rocm_userspace_marker, parse_amd_smi_rocm_version, parse_rocm_version_header,
        parse_rocminfo_hsa_version, rocm_version_from_root, sort_versioned_paths,
    };

    #[cfg(target_os = "linux")]
    #[test]
    fn rocm_6_release_info_layout_is_detected() {
        let root = tempfile::tempdir().expect("temporary ROCm root");
        let version = root.path().join(".info/version");
        std::fs::create_dir_all(version.parent().unwrap()).unwrap();
        std::fs::write(&version, "6.3.2\n").unwrap();
        assert_eq!(
            rocm_version_from_root(root.path()),
            Some(("6.3.2".to_string(), version))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rocm_10_core_header_layout_is_detected() {
        let root = tempfile::tempdir().expect("temporary ROCm root");
        let header = root
            .path()
            .join("core-10.0/include/rocm-core/rocm_version.h");
        std::fs::create_dir_all(header.parent().unwrap()).unwrap();
        std::fs::write(
            &header,
            "#define ROCM_VERSION_MAJOR 10\n#define ROCM_VERSION_MINOR 0\n#define ROCM_VERSION_PATCH 0\n",
        )
        .unwrap();
        assert_eq!(
            rocm_version_from_root(root.path()),
            Some(("10.0.0".to_string(), header))
        );
        assert_eq!(
            parse_rocm_version_header("#define ROCM_VERSION_MAJOR 10"),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn core_root_selection_uses_highest_version_deterministically() {
        let root = tempfile::tempdir().expect("temporary ROCm root");
        for (core, major, minor, patch) in [("core-6.3", 6, 3, 2), ("core-10.0", 10, 0, 0)] {
            let header = root
                .path()
                .join(core)
                .join("include/rocm-core/rocm_version.h");
            std::fs::create_dir_all(header.parent().unwrap()).unwrap();
            std::fs::write(
                header,
                format!(
                    "#define ROCM_VERSION_MAJOR {major}\n#define ROCM_VERSION_MINOR {minor}\n#define ROCM_VERSION_PATCH {patch}\n"
                ),
            )
            .unwrap();
        }
        assert_eq!(rocm_version_from_root(root.path()).unwrap().0, "10.0.0");

        let mut roots = vec![
            std::path::PathBuf::from("/opt/rocm-6.3"),
            std::path::PathBuf::from("/opt/rocm-10.0"),
        ];
        sort_versioned_paths(&mut roots);
        assert!(roots[0].ends_with("rocm-10.0"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn empty_lib_directory_is_not_a_rocm_marker() {
        let root = tempfile::tempdir().expect("temporary ROCm root");
        std::fs::create_dir(root.path().join("lib")).unwrap();
        assert!(!has_rocm_userspace_marker(root.path()));

        std::fs::write(root.path().join("lib/librocm-core.so.1"), "").unwrap();
        assert!(has_rocm_userspace_marker(root.path()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn amd_smi_and_rocminfo_versions_are_labeled_correctly() {
        assert_eq!(
            parse_amd_smi_rocm_version("ROCm version: 10.0.0\n"),
            Some("ROCm 10.0.0".to_string())
        );
        assert_eq!(parse_amd_smi_rocm_version("No version data"), None);
        assert_eq!(
            parse_rocminfo_hsa_version("Runtime Version: 1.16\n"),
            Some("HSA runtime version 1.16".to_string())
        );
        assert_eq!(parse_rocminfo_hsa_version("Name: AMD GPU"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn absent_rocm_root_has_no_detected_version() {
        let root = tempfile::tempdir().expect("temporary ROCm root");
        assert_eq!(rocm_version_from_root(root.path()), None);
    }

    #[test]
    fn rapl_access_is_cleanly_unavailable_without_package_domains() {
        let result = describe_rapl_access(0, 0);
        assert_eq!(result.tag(), "SKIP");
        assert!(result.message().contains("no package powercap domains"));
    }

    #[test]
    fn rapl_access_explains_admin_opt_in_without_changing_host_permissions() {
        let result = describe_rapl_access(1, 0);
        assert_eq!(result.tag(), "SKIP");
        assert!(result.message().contains("dedicated group"));
        assert!(result.message().contains("side-channel"));
        assert!(
            result
                .message()
                .contains("does not change host permissions")
        );
    }

    #[test]
    fn rapl_access_passes_when_all_package_energy_files_are_readable() {
        let result = describe_rapl_access(2, 2);
        assert_eq!(result.tag(), "PASS");
        assert!(result.message().contains("all 2"));
    }

    #[test]
    fn umc_bandwidth_is_explicitly_unavailable_without_a_controller_pmu() {
        let result = describe_umc_bandwidth_availability(0, Some(3));
        assert_eq!(result.tag(), "SKIP");
        assert!(result.message().contains("no amd_umc"));
    }

    #[test]
    fn umc_bandwidth_explains_perf_access_without_changing_policy() {
        let result = describe_umc_bandwidth_availability(16, Some(3));
        assert_eq!(result.tag(), "SKIP");
        assert!(result.message().contains("16 amd_umc PMUs"));
        assert!(result.message().contains("CAP_PERFMON"));
        assert!(result.message().contains("perf_event_paranoid is 3"));
        assert!(result.message().contains("does not change kernel policy"));
    }

    fn readout(power_w: f64) -> Option<String> {
        Some(format!("power=Some({power_w})W"))
    }

    /// A single card cannot answer the question this check exists for, so
    /// the verdict must say that rather than reporting agreement as if it
    /// were evidence. This is the arm every single-GPU host takes, which
    /// makes it the arm most likely to be read and misread.
    #[test]
    fn one_card_reports_the_question_as_unobservable() {
        let verdict = per_card_verdict(&[readout(32.0)], 1);
        assert!(
            verdict.contains("unobservable"),
            "single-card verdict must not imply a result, got: {verdict}"
        );
        assert!(
            !verdict.contains("IDENTICAL") && !verdict.contains("DISTINCT"),
            "single-card verdict must claim neither outcome, got: {verdict}"
        );
        assert!(
            verdict.contains("at least 2 physical cards"),
            "single-card verdict must explain how to verify differentiation, got: {verdict}"
        );
        assert!(
            !verdict.to_ascii_lowercase().contains("issue") && !verdict.contains('#'),
            "single-card verdict must not depend on an internal tracker reference, got: {verdict}"
        );
    }

    /// Two cards reporting the same telemetry is the failure this check
    /// looks for, so it must be named loudly rather than passing quietly.
    #[test]
    fn identical_readouts_across_cards_are_called_out() {
        let verdict = per_card_verdict(&[readout(32.0), readout(32.0)], 2);
        assert!(
            verdict.contains("IDENTICAL"),
            "matching readouts on two cards must be flagged, got: {verdict}"
        );
    }

    /// The healthy case: distinct cards, distinct telemetry.
    #[test]
    fn distinct_readouts_across_cards_are_the_expected_result() {
        let verdict = per_card_verdict(&[readout(32.0), readout(11.5)], 2);
        assert!(
            verdict.contains("DISTINCT"),
            "differing readouts on two cards are what attribution needs, got: {verdict}"
        );
    }

    /// One differing card is enough to prove the driver separates
    /// telemetry, even when two other cards happen to agree. A pairwise
    /// `all` over neighbours would report IDENTICAL here if the differing
    /// card sat at either end, so the ordering is deliberate.
    #[test]
    fn one_differing_card_among_three_still_reads_as_distinct() {
        let verdict = per_card_verdict(&[readout(32.0), readout(32.0), readout(11.5)], 3);
        assert!(
            verdict.contains("DISTINCT"),
            "any disagreement proves separation, got: {verdict}"
        );
    }

    /// Silent cards make the comparison meaningless, so the verdict must
    /// be undetermined rather than borrowing the one card that answered.
    #[test]
    fn a_single_answering_card_leaves_the_verdict_undetermined() {
        let verdict = per_card_verdict(&[readout(32.0), None], 2);
        assert!(
            verdict.contains("undetermined"),
            "one silent card cannot be compared against, got: {verdict}"
        );
        assert!(
            !verdict.contains("IDENTICAL") && !verdict.contains("DISTINCT"),
            "an undetermined verdict must claim neither outcome, got: {verdict}"
        );
    }
}
