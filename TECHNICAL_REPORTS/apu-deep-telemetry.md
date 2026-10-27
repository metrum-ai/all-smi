# AMD APU telemetry

## Shared memory and power readings

The AMD reader now records GTT memory used and capacity separately from its
existing GPU-memory counters. Prometheus exposes these values as
`all_smi_gpu_memory_gtt_used_bytes` and
`all_smi_gpu_memory_gtt_total_bytes`. On AMD APU metrics tables that report
GFX power, `all_smi_gpu_power_consumption_watts` uses that GFX-only reading;
the separate `all_smi_gpu_socket_power_watts` gauge preserves whole-socket
power. When the driver does not provide GFX power, the existing power metric
retains its socket-power fallback. Discrete AMD power handling is unchanged.

An idle Strix Halo API sample reported 31,657,984 bytes of GTT usage out of
121,367,355,392 bytes, 0.109 W GFX power, and 15.086 W socket power. An
independent AMD-SMI sample taken separately reported 0.23 W GFX and 16.02 W
socket power; the readings are time-varying and were not simultaneous. The
existing GPU-memory counters remain distinct from shared GTT memory.

The implementation and verification are part of the focused AMD APU telemetry
work; additional telemetry fields are tracked separately.
