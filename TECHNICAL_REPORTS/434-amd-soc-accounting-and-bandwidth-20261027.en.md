<!-- Copyright (c) 2026 Metrum AI, Inc. All rights reserved. -->

# AMD SoC power accounting and system memory-bandwidth availability

## Scope

This initial change keeps GPU/device power separate from chassis-level power. It
removes fallbacks that presented aggregate GPU power as chassis power, and leaves
`ChassisInfo.total_power_watts` unavailable when no chassis/SoC-level source is
reported. GPU power remains available through its GPU-scoped metric. No RAPL or
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
and power side-channel risk. all-smi does not change permissions or install
rules. The `amd.umc.bandwidth` doctor check reports system memory bandwidth as
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

The snapshot regression test verifies that an available GPU power reading does
not synthesize a chassis-level reading. Workspace tests, formatting, Clippy,
and release-build results are recorded with the PR validation.
