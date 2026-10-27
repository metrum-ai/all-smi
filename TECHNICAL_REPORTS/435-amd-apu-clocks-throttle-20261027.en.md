<!-- Copyright (c) 2026 Metrum AI. All rights reserved. -->

# AMD APU clock and throttle telemetry

## Implemented readings

On AMD APU `gpu_metrics` v3 tables, the reader exports the reported average GFX, SOC, UCLK, FCLK, VCLK, VPECLK, IPUCLK, and MPIPU clock fields as clock-domain gauges. A zero frequency is omitted because it represents an unavailable or inactive clock, not a measured operating frequency. VCN activity uses the v3 multimedia activity field, with `vcn_busy_percent` as a sysfs fallback; an actual zero-percent activity reading is retained.

The reader also exports the `pp_dpm_sclk`, `pp_dpm_mclk`, `pp_dpm_fclk`, and `pp_dpm_socclk` current level and min/max frequency range when those sysfs files are readable. The forced performance-level setting and active power profile are emitted only when their sysfs controls provide valid values. Missing files or unreadable controls remain absent.

`gpu_metrics` v3 throttle-residency fields are exposed as raw counters, with a `reason` label. The ABI does not document a portable unit for these values, so the API deliberately does not call them seconds, time, or percentages, and does not convert them.

## Idle evidence and limits

An idle sample on a Strix Halo (Radeon 8060S) test system reported average GFX/SOC/UCLK/FCLK/VCLK/VPECLK frequencies of 614/603/1000/1561/800/800 MHz, VCN activity of 0%, and zero-valued IPU clocks. The sysfs DPM files showed SCLK level 0 at 600 MHz with a 600–2900 MHz range and MCLK level 2 at 1000 MHz with a 400–1000 MHz range. The forced performance level was `auto`; `pp_power_profile_mode` was unavailable, and `vcn_busy_percent` read 0. No workload was started, so this is availability and idle-format evidence, not a dynamic accuracy or load-response measurement.

The same idle sample reported raw SPL, FPPT, SPPT, and thermal-core residency values of 94,236,815, 6,910,837, 12,236,502, and 1,430 respectively; other exposed reasons were zero. These are recorded without a unit interpretation. A one-shot Prometheus snapshot from the application confirmed the clock-domain, DPM-range, VCN, performance-level, and raw throttle-residency series. Device, kernel, and firmware support determine which metric and sysfs fields are available.

Per-process attribution remains a separate follow-up in issue #26. The existing AMD process reader preserves its compatibility behavior of using nonzero fdinfo VRAM usage, or GTT usage when VRAM is absent, for the single `used_memory` field. It does not report simultaneous VRAM and GTT amounts separately, and the process GPU-utilization field is not an AMD fdinfo measurement.
