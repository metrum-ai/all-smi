<!-- Copyright (c) 2026 Metrum AI. All rights reserved. -->

# AMD APU clock and throttle telemetry

## Implemented readings

On AMD APU `gpu_metrics` v3 tables, the reader exports the reported time-filtered average GFX, SOC, UCLK, FCLK, VCLK, VPECLK, IPUCLK, and MPIPU clock fields as `all_smi_gpu_clock_domain_average_mhz`. Zero and `u16::MAX` values are omitted as inactive/unavailable placeholders. VCN activity is accepted only in the 0–100 range; an absent or invalid v3 value falls back to `vcn_busy_percent`, and an actual 0% sample is retained. Unsupported `u16::MAX` and `u32::MAX` clock and throttle-residency sentinels are omitted.

The reader also exports the `pp_dpm_sclk`, `pp_dpm_mclk`, `pp_dpm_fclk`, and `pp_dpm_socclk` frequency ranges when those sysfs files are readable. Indexed DPM formats include the current level; the SMU v14 min/current/max form is treated as named rows, so its ordinal is not exported as a DPM table level. Performance level, active power profile, and DPM details are cached for 10 seconds to avoid repeatedly issuing slow sysfs/SMU reads. `pp_power_profile_mode` formats with or without a colon are parsed when present; missing files remain absent.

`gpu_metrics` v3 throttle-residency fields are exposed as raw Prometheus gauges, with a `reason` label. The ABI does not document a portable unit or establish monotonicity, so consumers must not interpret them as seconds or percentages, and the reader does not convert them.

## Idle evidence and limits

An idle sample on a Strix Halo (Radeon 8060S) test system reported average GFX/SOC/UCLK/FCLK/VCLK/VPECLK frequencies of 614/603/1000/1561/800/800 MHz, VCN activity of 0%, and zero-valued IPU clocks. The observed `pp_dpm_sclk` file (committed as a fixture) has three unlabeled rows: 600, 1100, and 2900 MHz, with the first marked active. The SMU v14 implementation uses these rows for min/current/max rather than DPM levels, so the SCLK level is omitted for this ambiguous shape. MCLK reported level 2 at 1000 MHz in a 400–1000 MHz range. The forced performance level was `auto`; `pp_power_profile_mode` was not present on this host, and `vcn_busy_percent` read 0. No workload was started, so this is availability and idle-format evidence, not a dynamic accuracy or load-response measurement.

The same idle sample reported raw SPL, FPPT, SPPT, and thermal-core residency values of 94,236,815, 6,910,837, 12,236,502, and 1,430 respectively; other exposed reasons were zero. These are recorded without a unit interpretation. A one-shot Prometheus snapshot from the application confirmed the clock-domain, DPM-range, VCN, performance-level, and raw throttle-residency series. Device, kernel, and firmware support determine which metric and sysfs fields are available.

Per-process attribution remains a separate follow-up in issue #26. The existing AMD process reader preserves its compatibility behavior of using nonzero fdinfo VRAM usage, or GTT usage when VRAM is absent, for the single `used_memory` field. It does not report simultaneous VRAM and GTT amounts separately, and the process GPU-utilization field is not an AMD fdinfo measurement.
