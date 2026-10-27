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
`kernel.perf_event_paranoid` value was `3`. No perf events were opened, no
kernel policy or host permissions were changed, and no bandwidth values were
measured. The GPU memory-activity metric remains GPU-scoped and is not presented
as system DRAM bandwidth.

The `amd.rapl.access` doctor check reports package-energy access as unavailable
when the current user cannot open `energy_uj`. Read-only inspection found the
package counter owned by root with mode `0400`; the normal-user path did not
open it. The check explains the administrator-managed dedicated-group opt-in
and power side-channel risk. A one-off root doctor invocation reported the
single package counter readable; the normal-user invocation reported it
unavailable. This verifies access only, not power measurement. all-smi does not
change permissions or install rules. The `amd.umc.bandwidth` doctor check reports system memory bandwidth as
unavailable, explains that system-wide PMU access requires `CAP_PERFMON` or an
administrator-managed perf policy, and states that all-smi does not change
kernel policy or request capabilities. A privileged implementation is out of
scope here; there is no helper binary in this epic. A root STREAM-style
measurement has not been performed because this change does not sample the
counters.

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
