<!-- Copyright (c) 2026 Metrum AI. All rights reserved. -->
# Upstream sync: lablup/all-smi `fe479ae` (2026-11-16)

Merged `upstream/main` into a sync branch off the fork's `main` (merge commit, as in earlier sync PRs). The fork already contained upstream through `273f756`, so one upstream commit was new.

## Upstream commits merged

- `fe479ae` fix: repair the Windows self-hosted workspace before checkout (#487)
  - CI: `release.yml` signing job checks out the signing script into `.signing-scripts` instead of narrowing the shared workspace to a sparse checkout; `windows-selfhosted.yml` empties the shared workspace before checkout and fails early if `Cargo.toml` is missing.
  - Tests: `intel_gpu_sysfs.rs` ReBAR test uses a 257 MiB file instead of 12 GiB (NTFS allocates the full length).

## Conflict decisions

None. The merge was textual-clean. Fork runner labels (`aws-pub-4c`), `metrum-release.yml`, `shadeform-gpu-validation.yml` and `TECHNICAL_REPORTS/runs/*` were not touched. No dependency or version changes (main, AMD and DCGM crates remain 0.28.1), so `Cargo.lock` is unchanged.

## Validation

- Not run in the authoring environment: it has no Rust toolchain and no NVIDIA GPU, so `cargo fmt/clippy/build/test` and the Shadeform GPU validation required by `AGENTS.md` are still outstanding. The only Rust change is inside a `#[test]` function.
