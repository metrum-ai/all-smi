<!-- Copyright (c) 2026 Metrum AI, Inc. All rights reserved. -->

# AMD SoC power accounting and system memory-bandwidth availability

## Scope

When no platform chassis-power source is available, the existing estimate from
available discrete GPU device-power readings is retained. Unavailable GPU rows
are skipped. Any APU GPU row carrying the socket-power detail marker is
excluded, even when its primary power field contains GFX-only power, so it is
not treated as discrete chassis power or added to another device.
Platform-reported chassis power takes precedence. The same shared fallback
helper is used by the API, snapshot, and local TUI collection paths. No RAPL or
UMC perf counters are read by this change.

## Read-only system memory-bandwidth discovery

On a Strix Halo (Radeon 8060S) test system, read-only inspection found 16
`amd_umc` PMU devices and an `amd_df` PMU. The current
`kernel.perf_event_paranoid` value was `3`. No perf events were opened and no
kernel policy or host permissions were changed. No UMC/DF PMU bandwidth values
were measured in this discovery step. GPU engine-activity metrics remain GPU-
scoped and are not presented as system DRAM bandwidth.

The `amd.rapl.access` doctor check reports package-energy access as unavailable
when the current user cannot open `energy_uj`. Read-only inspection found the
package counter owned by root with mode `0400`; the normal-user path did not
open it. The check explains the administrator-managed dedicated-group opt-in
and power side-channel risk. A one-off root doctor invocation reported the
single package counter readable; the normal-user invocation reported it
unavailable. This verifies access only, not power measurement. all-smi does not
change permissions or install rules. The `amd.umc.bandwidth` doctor check
specifically reports bandwidth from system-wide UMC PMU counters as unavailable,
explains that those counters require `CAP_PERFMON` or an administrator-managed
perf policy, and states that all-smi does not change kernel policy or request
capabilities. AMD APU `gpu_metrics` bandwidth readings are a separate
unprivileged source and are exported separately when the driver supplies them;
they are not UMC measurements. A privileged UMC implementation is out of scope
here; there is no helper binary in this epic. No root STREAM-style or perf
counter measurement was performed.

## APU gpu_metrics unit and range check

On a Strix Halo (Radeon 8060S) test system, `amd-smi metric --json` reported the
APU fields `apu_average_dram_reads` and `apu_average_dram_writes` in `MB/s`;
the AMD GPU-metrics schema describes these as time-filtered DRAM read/write
bandwidth in MB/sec. Samples were read through a fresh `sg render` context.
Before the load, the shared-device preflight found no users of `/dev/kfd` or
`/dev/accel/accel0`, no `vllm`/`llama`/`ollama`/benchmark process, and low host
load. The load ran with nice priority 19.

Five idle samples (read/write MB/s) were 730/132, 551/106, 500/79, 483/112,
and 475/114; their means were 548/109 MB/s (0.656 GB/s combined). A short
16-thread STREAM-style triad used 2,097,152 doubles per thread (768 MiB across
three arrays), ran for 8.016 s, and reported 114.505 GB/s using the conventional
24 logical bytes per element (two reads plus one write). During its active
window, the APU fields rose into tens of GB/s; six samples were 42,377/14,727,
33,753/33,228, 45,529/37,058, 48,762/38,115, 49,610/38,383, and
49,860/38,466 MB/s. The peak simultaneous sum was 88.326 GB/s, about 77.1% of
the triad's reported logical rate (22.9% lower). Thus the source responds to
memory load and its MB/s scale is consistent, but this sample does not meet the
issue's ±10% comparison target and must not be presented as complete aggregate
system bandwidth.

The fields are `u16`; the largest encodable value is 65,535 MB/s per direction.
Neither field reached that ceiling in this run (peaks: 49,860 MB/s read and
38,466 MB/s write), so saturation was not observed. Values at the ceiling are
ambiguous and cannot represent higher traffic; the API documentation calls out
that bound. These observations validate unit scale and non-saturation for this
short workload only, not accuracy under every workload or above the field
ceiling.

An administrator who explicitly accepts the power side-channel risk may grant
a dedicated group read-only access to the relevant package `energy_uj` file,
for example with a systemd-tmpfiles rule restricted to that exact domain and
mode `0440 root:<dedicated-group>`. Group membership exposes workload-dependent
power information. This is an operator-managed opt-in only: all-smi never
changes permissions, installs a rule, or recommends applying it to a host
without an administrator's approval.

## Validation

Tests verify that discrete GPU readings (including multi-device rows with
unavailable dies) still provide the fallback, that APU rows carrying the socket
power marker are excluded even when primary power is GFX-only, and that
platform-reported chassis power takes precedence. Workspace tests,
formatting, Clippy, and release-build results are recorded with the PR
validation.
