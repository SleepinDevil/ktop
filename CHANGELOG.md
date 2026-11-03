# Changelog

## Unreleased
- Memory panel: new `{} committed / {} limit` line (`Committed_AS` / `CommitLimit` from `/proc/meminfo`, hidden when unparseable) inserted under the `avail / cached` line, and the blank separator between the RAM and Swap groups (below the PSI line) removed so the panel is exactly 8 content lines; Top Processes lists now keep up to 20 rows instead of 10 (`MAX_PROCS`); main layout now fixes the mid row to exactly 10 rows (the three mid panels' 8-line maximum) and gives the procs row `Constraint::Min(5)` — all vertical slack and all of the first stage of shrinking go to the process panels (down to two process rows + header + borders) while GPU/mid/temps/disks/status keep their exact sizes
  - Tested: `cargo check`, `cargo test` (new `parses_committed_and_limit_kib_as_bytes`, `missing_or_malformed_keys_are_unavailable`), `cargo build --release`, 120x60/40/36/34/30 PTY captures verifying the 8-line Memory panel, 20 process rows, and the shrink priority

- Memory panel: new line under the RAM `avail / cached` line showing memory pressure — the PSI `some` stall percentages as `PSI some avg10/avg60/avg300%`, parsed from `/proc/pressure/memory` each refresh — and hidden entirely when PSI is unavailable (kernel < 4.20 or pressure disabled at boot)
  - Tested: `cargo check`, `cargo test` (new `parses_some_stall_averages_from_psi_content`, `parses_some_line_regardless_of_key_order`, `missing_or_malformed_some_line_is_unavailable`), `cargo build --release`, 120x60 PTY capture showing the PSI line

- Network panel: the single `Peak:` line (max of up/down) under the Down readout is now split into two per-direction lines — `Peak Down:` under the Down line and a new `Peak Up:` line directly under the Up line — backed by independent `max_up`/`max_down` running maxima in `NetTracker`; the combined `max_speed` is kept untouched as the sparkline/bar scale bound
  - Tested: `cargo check`, `cargo test`, `cargo build --release`, 120x60 PTY capture showing both peak lines

- Memory panel: moved the `avail`/`Cache` detail line up to sit directly under the RAM `used / total` line (the blank separator between the RAM and Swap groups removed) and added a new line under the Swap `used / total` showing swap I/O throughput as `R <rate>  W <rate>` — pages read into RAM from swap (`pswpin`) and written out (`pswpout`) from `/proc/vmstat` × page size, delta-sampled per refresh like the network tracker, colored with the same R=net_down/W=net_up pair the Disks panel uses; reads `0 B/s` on systems whose swap layer does not bump the counters (e.g. zswap-only)
  - Tested: `cargo check`, `cargo test` (new `reads_pswpin_pswpout_from_vmstat`), `cargo build --release`, 120x60 PTY capture showing the reordered Memory panel with the swap I/O row

- CPU panel: sub-threshold idle cores (usage below the 6.25% single-dot cut, so bucket 0) now flicker in the grid — each render frame a 20% chance to show exactly one raised dot at any of the 16 positions across the core's two glyphs (uniform, both glyphs), otherwise fully blank, so the grid always has some activity to look at; deterministically derived from the frame counter (`render_base.elapsed()/refresh`) so redraws inside one frame are identical, colored at the `bar_low` end of the gradient; compat/steady ASCII mode stays blank for idle cores
  - Tested: `cargo test` (new `idle_core_flickers_one_dot_across_the_two_glyphs`, `idle_flicker_is_deterministic_per_frame`), `cargo build --release`, live-terminal check
- CPU panel: the per-core grid now shows each core as two random braille glyphs (U+2800..U+28FF) drawn from the dot-count bucket matching the core's usage (bucket k = every glyph with round(pct/12.5) raised dots: 0/8 dots at 0% up to the solid `⣿⣿` at 100%), replacing the `░▒▓█` shade ramp; pairs re-roll once per wall-clock second and stay stable across frames inside the same second, and each pair is colored by the smooth `bar_low`..`bar_high` gradient at its own usage (same as the GPU Util bars) instead of the stepped low/mid/high bands; compat/steady mode keeps ASCII pairs bucketed by the same dot counts (e.g. `#=` for a pegged core, `..` at 25%)
  - Tested: `cargo check`, `cargo test` (new `braille_tables_cover_every_braille_glyph_exactly_once`, `braille_bucket_rounds_usage_to_nearest_eighth`, `core_glyph_pair_is_two_chars_from_the_usage_bucket`, `glyph_pair_is_stable_within_a_second_and_rerolls_across_seconds`, `ascii_fallback_pairs_use_the_bucket_and_the_one_below`), `cargo build --release`, manual live-terminal confirmation of the grid
- CPU panel power row now shows `⌁: SU req` when the RAPL package zone exists but its energy counters are root-only (PLATYPUS lockdown, i.e. ktop run without privileges), keeping `n/a` for systems that expose no CPU package power at all
  - Tested: `cargo test` (new `missing_energy_counter_is_unavailable_not_denied`), `cargo build --release`, 120x60 PTY capture on the root-locked 5950X box showing `⌁: SU req`
- CPU panel: added a blank line between the two-row history sparkline and the per-core grid
  - Tested: `cargo build --release`, 120x60 PTY capture showing sparkline rows, blank separator, then the grid
- CPU panel: core-grid symbol changed to `⊞` (U+229E) and power symbol to `⌁` (U+2301); the grid + readout block is now centered in the panel when the inner width fits it, and on narrow panels the grid is clipped column-by-column (whole cells only) so the `Σ:`/`ƒ:`/`⌁:` readouts stay visible
  - Tested: `cargo build --release`, 120x60 PTY capture showing the block centered with 4-column padding and 90x40 capture showing the grid clipped to 7 columns with all readouts intact
- CPU panel: dropped the `Cores:`/`Total:`/`Freq:` row and the gaps between grid columns; the per-core grid now carries those readouts down its right side as `▦: <cores>`, `Σ: <total%>`, `ƒ: <freq>` and a new `⚡: <watts>` row showing CPU package power (RAPL; `n/a` when the energy counters are not readable, e.g. unprivileged access on AMD; ASCII `C:`/`T:`/`F:`/`P:` fallbacks in compat/steady mode)
  - Tested: `cargo test` (9 passed), `cargo build --release`, 120x60 PTY capture under load showing gapless `██░░…` grid rows with `▦: 32`, `Σ: 243%`, `ƒ: 4772 MHz` and `⚡: n/a` right of the grid
- Added a per-core usage grid to the CPU panel below the `Cores:` row: up to 8 columns x 4 rows of two-character shade cells (`░░▒▒▓▓██`, ASCII `. : = #` in compat/steady mode), each colored by the bar ramp (low/mid/high); per-core values are EMA-smoothed (0.65/0.35, same blend as the process table); machines with more than 32 cores group cores so each cell shows the hottest core of its group
  - Tested: `cargo check`, `cargo test` (new `lays_out_thirty_two_cores_as_four_rows_of_eight`, `groups_beyond_thirty_two_cores_by_max`, `short_core_counts_use_partial_rows`), `cargo build --release`, 120x60 PTY capture under 3-core load showing `██` hot cells in `bar_high` and `░░` idle cells in `bar_low`, and a `--compat` capture showing the ASCII ramp

- Panel appearance tweaks: Network panel dropped the blank line between `Down` and the peak row and renamed it `Peak Down` in white; CPU panel dropped the `History` label, moved the sparkline above the `Cores:` row (now white), and the bottom CPU name matches the panel border theme color; Memory panel colors the Swap percentage by usage (was static grey) and shows the two lines below Swap in white; Disks panel border/title now use the same theme color as Temps (`bar_mid`); GPU panel card names at the bottom match the panel border color
  - Tested: `cargo check`, `cargo test`, `cargo build --release`, 120x60 truecolor PTY capture confirming `Peak Down`/`Cores:`/swap detail lines emit classic white SGR (`97`), Swap % follows the bar color ramp, CPU/GPU bottom titles share their border color, and Disks/Temps borders are both `bar_mid`

- Intel GPU PCIe link stats now read from the top-most upstream switch port in the card's sysfs topology (first non-root-bus bridge, e.g. `0000:0b:00.0` for a Battlemage card behind a PCIe switch) instead of the endpoint itself, because the xe driver pins the endpoint's own `current_link_speed`/`current_link_width` at the idle floor (2.5 GT/s x1); GPUs on a root port directly are unchanged
  - Tested: `cargo check`, `cargo test` (new `picks_topmost_non_root_bridge_from_sysfs_path`), `cargo build --release`, PTY capture showing `PCIe G4 x8 (max G5 x16)` for the Intel Arc Pro B70 behind the switch and unchanged NVIDIA `PCIe G4 x8 (max G4 x16)`

- Added `AGENTS.md` repository guidelines for AI coding assistants (architecture, data flow, commands, conventions, QA workflow)
- Slimmed `CLAUDE.md` to mandatory rules only, pointing to `AGENTS.md` (guidelines) and `CHANGELOG.md` (canonical history); removed its duplicated condensed changelog
- Refreshed the `README.md` feature list for everything added since v1.0.17: Intel Arc GPU support, per-GPU power & PCIe rows, the Disks panel, the per-core CPU grid, the memory panel's PSI pressure / committed-limit / swap-I/O lines, per-direction network peak lines, and the 20-row process tables; updated `AGENTS.md` module/data-flow/testing descriptions to match the current code
- Tested: docs-only change; no code touched, no build required



## 1.0.23 — 2026-10-18

- Added a Disks panel below Temps: live per-device Read/Write throughput from `/sys/block/<dev>/stat` deltas, physical disks only (nvme/sd/vd/mmcblk; loop/ram/zram/dm/partitions skipped), NVMe drives sorted first, laid out 3 disks per row
- Fixed the Pwr history sparkline: bar heights are now watts ÷ card power limit (was raw watts clamped to 100, so any card above ~100 W pinned the graph to full height)
- CPU panel name drops the core-count suffix: `AMD Ryzen 9 5950X` (was `AMD Ryzen 9 5950X 16-Core Processor`)
- GPU panel names keep the vendor: `NVIDIA GeForce RTX 3080 Ti`, `Intel Arc Pro B70`
- Fixed a blank trailing row in the Disks panel: panel height is now `ceil(n/3) + 2` (border + rows) instead of `+ 3`
- Tested: `cargo test` (5 passed), `cargo build --release` (0 warnings), PTY frame capture: Disks panel `nvme0n1/nvme1n1/sda` row 1 + `sdb/sdc/sdd` row 2 with live `R 38.2 KB/s` and `W 167.9 KB/s` under `dd oflag=direct` load, Pwr sparkline low bars at 37 W/370 W, CPU border `AMD Ryzen 9 5950X`, GPU borders with vendor names

## 1.0.22 — 2026-10-18

- PCIe row now shows the live trained link only (`PCIe G4 x8`), with `(max G4 x16)` appended only when running below the device max; removed the confusing `current → max` arrow notation and the yellow degraded coloring — the row is plain white like the rest of the panel
- Added a blank line above the PCIe row to match the Util→Mem and Mem→Pwr gaps
- CPU panel bottom border now shows the processor name from `/proc/cpuinfo` (e.g. `AMD Ryzen 9 5950X 16-Core Processor`), matching the GPU panels' card-name style
- Intel PCIe link is sampled live every tick from the same sysfs file nvtop reads (`current_link_speed`/`current_link_width` of the GPU endpoint); the xe driver parks the B70 at G1 x1 when idle and retrains under load — ktop reflects the file as it changes
- Tested: `cargo test` (5 passed), `cargo build --release` (0 warnings), PTY frame capture: `PCIe G1 x8 (max G4 x16) ↓ 0.6 ↑ 0.7 MB/s` (NVIDIA), `PCIe G1 x1` (B70, no max suffix), gap above PCIe, CPU border `AMD Ryzen 9 5950X 16-Core Processor`; strace confirmed nvtop reads the same endpoint `current_link_speed`

## 1.0.21 — 2026-10-18

- Pwr bars now scale to the card's power limit (max power) from the driver — NVML `power_management_limit` (NVIDIA), `power1_cap` (AMD), `power1_cap`/`power1_max` (Intel) — and the readout shows `current/limit W` (e.g. `42.0/ 370 W`); falls back to observed peak when no limit is exposed
- Added live PCIe transfer speed to the GPU panel PCIe row (NVIDIA via NVML `nvmlDeviceGetPcieThroughput` RX/TX KB/s → `↓ x.x ↑ x.x MB/s`); Intel/AMD expose no PCIe byte-throughput counter (XeStrata reports `None` for Intel too), so the row shows link speed/width only there
- Added a blank line above the Pwr row to match the Util→Mem gap
- Fixed the large white space below the PCIe row: GPU panel is now a fixed `Length(13)` sized to its content; the freed rows go to the process panel
- Tested: `cargo test` (5 passed), `cargo build --release` (0 warnings), PTY frame capture: `42.0/ 370 W` (RTX 3080 Ti) and `45.2/ 275 W` (Arc Pro B70, `power1_cap` 275 W), `PCIe G1 x8 → G4 x16 ↓ 1.7 ↑ 8.1 MB/s` (NVIDIA), B70 row without throughput, gap above Pwr, no trailing blank rows in GPU panel

## 1.0.20 — 2026-10-18

- Moved the per-GPU `Pwr` bar + sparkline into the GPU panel columns (under Util/Mem) and removed the standalone Pwr row; GPU panel now gets more vertical space (`Fill(3)`)
- Added a live `PCIe` row per GPU: current trained link speed/width vs device max from sysfs (`current_link_speed`/`current_link_width`, `max_link_speed`/`max_link_width`), yellow when running below max (e.g. `PCIe G4 x8 → G4 x16`)
- PCIe link state works for all vendors via the PCI device sysfs (NVML `nvmlDeviceGetPciInfo` link fields return 0 and Level Zero `zesDevicePciGetState`/`GetStats` are unsupported on current NVIDIA/Intel stacks; no per-device PCIe byte-throughput counter exists in NVML, L0 sysman, or the kernel)
- Tested: `cargo test` (5 passed), `cargo build --release` (0 warnings), PTY smoke on RTX 3080 Ti + Arc Pro B70: Pwr bar/sparkline under Mem in each GPU column, `PCIe G4 x8 → G4 x16` (yellow, NVIDIA) and `PCIe G1 x1 → G1 x1` (xe power-managed link, B70)

## 1.0.19 — 2026-10-18

- Added a per-GPU `Pwr` panel between the net/CPU/Mem row and the Temps panel: live watts with a bar (scaled to the observed peak, 100 W floor) and a history sparkline per GPU
- Sources: NVIDIA NVML `nvmlDeviceGetPowerUsage` (instantaneous mW), AMD `power1_average` (µW), Intel xe `energy1_input` µJ deltas / i915 `power1_max`/`power1_average`; cards without a live sensor show `n/a`
- Per-GPU power is now sampled once per refresh tick into `AppState.gpu_power`/`gpu_power_hist` and reused for the footer power estimate (previously the footer summed three separate per-vendor calls)
- Tested: `cargo test` (5 passed), `cargo build --release` (0 warnings), PTY smoke on RTX 3080 Ti + Arc Pro B70: Pwr panel shows live watts and sparklines for both GPUs, footer `PWR` estimate unchanged

## 1.0.18 — 2026-10-18

- Added Intel Arc GPU detection and monitoring (Alchemist/Battlemage via `xe`/`i915` sysfs)
- Detection matches PCI vendor `0x8086` on DRM cards; integrated Intel GPUs are included (consistent with AMD APUs) and render with the sensors they expose
- Temperature selects the hwmon input labeled `pkg` (xe/Battlemage) with fallback to `temp1_input` (i915); max from `tempN_crit`/`tempN_max`
- Live power computed from `energy1_input` (µJ) deltas on xe, falling back to `power1_max`/`power1_average` (i915); summed into the footer power estimate
- Name Intel cards from a PCI device-id table (Arc Pro B70, Arc A770/A750/A380/A310, Arc B580) with `Intel GPU [0x…]` fallback
- Add Intel VRAM used/total from Level Zero sysman (`libze_loader.so.1` dlopened at runtime, `ZES_ENABLE_SYSMAN=1` set automatically, device matched by PCI BDF) — card-wide, no root, no perf; falls back to summing `drm-active-vram0` from `/proc/<pid>/fdinfo` (the user's own GPU processes), then to `mem_info_vram_used` (i915)
- Add Intel utilization from the xe PMU (engine-active/total-ticks, render class) via raw `perf_event_open` when permitted, falling back to DRM fdinfo engine cycles (`drm-cycles-rcs/ccs` ÷ `drm-engine-capacity-*` over `drm-total-cycles-*` deltas) which works without root; shows `n/a` when neither source is available
- Add Intel VRAM total from the PCI BAR2 aperture when `mem_info_vram_total` is absent (32 GiB on Arc Pro B70); Level Zero sysman total (31.9 GiB) takes precedence when present
- Gate the Mem bar, GB line, and sparkline on VRAM availability so unavailable memory reads `n/a` instead of a fake 0%
- Tested: `cargo test` (5 passed, incl. new fdinfo_scan fake-proc test), `cargo build --release`, PTY smoke on Intel Arc Pro B70 (xe) + RTX 3080 Ti (NVML): panel titled "Arc Pro B70" with `Util 32.0%` (fdinfo, XeStrata engine active), `Mem 99.2%` `31.6/31.9 GB` (Level Zero sysman), pkg temp 64/105°C, PWR ~165 W includes B70; NVIDIA GPU unchanged; note nvtop shows no Intel GPU on this host (`perf_event_paranoid=4` blocks its PMU source)

## 1.0.17 — 2026-05-06

- Restored color in Basic mode without using extended foreground color escapes
- Rewrites crossterm standard-palette foreground/background SGR from `38;5;n` / `48;5;n` into classic ANSI `30-37` / `90-97` and `40-47` / `100-107`
- Basic mode maps theme RGB colors to visible classic ANSI colors, keeping text, borders, bars, and status accents colored while avoiding the flash-triggering foreground sequence class
- Tested: `cargo check`, `cargo test`, `cargo build --release`, PTY capture with Basic mode showing zero RGB foreground, zero indexed foreground, zero blink SGR, and classic ANSI color output

## 1.0.16 — 2026-05-06

- Made Basic color mode avoid all extended foreground color SGR output, including indexed `38;5;n` sequences as well as RGB `38;2;r;g;b`
- Clear/reset the terminal when toggling color mode so stale truecolor-styled cells are repainted immediately
- Kept truecolor as the default; Basic is now the safest monochrome fallback for terminals that flash on extended foreground color escapes
- Tested: `cargo check`, debug build, PTY capture with Basic mode showing zero RGB foreground and zero indexed foreground SGR sequences

## 1.0.15 — 2026-05-06

- Added a main-screen `c Color` toggle that switches between truecolor and basic ANSI color output
- Persisted color mode in the existing user config alongside the selected theme, defaulting existing installs to truecolor
- Mapped RGB and indexed theme colors to ANSI/basic colors when Basic color mode is active
- Tested: `cargo check`, `cargo build --release`, PTY smoke for persisted `c` toggle and RGB-SGR removal in Basic mode

## 1.0.14 — 2026-05-06

- Buffered terminal output before handing stdout to ratatui so frame flushes are less fragmented over remote transports
- Simplified `--compat` progress bars to one filled span plus one empty span instead of per-cell RGB gradient spans, reducing paint-stream volume during each poll

## 1.0.13 — 2026-05-06

- Made `--compat` avoid dark-gray ANSI styling and shaded filler glyphs, which can animate or flash in some remote terminals even after ktop exits
- Compatibility mode now clears the current screen on entry and exit so stale styled cells do not remain behind the shell prompt
- Reset terminal attributes and colors during cleanup before returning control to the shell

## 1.0.12 — 2026-05-06

- Reduced lower-screen repaint churn by smoothing process CPU percentages before sorting/display
- Added deterministic process-table tie breakers so similar samples do not reshuffle rows unnecessarily
- Made `--compat` use low-churn rendering: moving sparklines are suppressed and process tables refresh every 10s
- Intended for terminals that visibly flash only the cells/regions rewritten by live metric updates

## 1.0.11 — 2026-05-06

- Added `ktop diagnose-render` to isolate visual flashing by terminal escape category
- Render diagnostics cover plain output, cursor repaint, ANSI color, 24-bit RGB color, Unicode blocks, full-frame painting, repeated clear, alternate screen, and synchronized update
- Intended follow-up for terminals where size diagnostics are stable but the TUI still visibly flashes

## 1.0.10 — 2026-05-06

- Avoid repeated terminal clears from unstable resize reporting by using a fixed viewport and debounced resize handling
- Added `ktop diagnose-terminal` to report terminal size jitter and resize event storms
- Added `--compat` / `KTOP_COMPAT=1` for terminals that still flash during normal full-screen rendering
- Compatibility mode disables alternate screen and synchronized redraws, skips startup clears, and defaults to a 2s refresh unless `-r` is set
- Unknown commands/options now exit with a usage error instead of falling through into the TUI
- Terminal diagnostics still print size samples when stdin event polling is unavailable

## 1.0.9 — 2026-05-06

- Added `ktop update` to run the official README one-line update installer
- Added `ktop uninstall` to remove only the resolved `ktop` executable while leaving user config and unrelated system files untouched
- Added command help for update/uninstall
- Tested: `cargo check`, `cargo build --release`, command help smoke tests, temp-copy uninstall smoke test

## 1.0.8 — 2026-05-06

- Reduced repaint flashing on remote/web terminals by wrapping TUI draws in synchronized terminal updates
- Hide the cursor while ktop is running and restore it on exit
- Added `--no-alt-screen` / `KTOP_NO_ALT_SCREEN=1` for terminals whose alternate-screen support flashes or tears
- Added `--no-sync` / `KTOP_NO_SYNC=1` as a fallback for terminals that mishandle synchronized update escape sequences
- Clamp refresh intervals below 0.25s to avoid accidental tight redraw loops
- Cap footer right-side status width to the visible terminal width on narrow terminals
- Tested: `cargo check`, `cargo build --release`

## 1.0.7 — 2026-04-12

- Keep the footer power slot visible on narrower terminals by right-anchoring power and OOM status
- Let the left-side help text truncate first instead of clipping PWR off the screen
- Tested: `cargo build --release`

## 1.0.6 — 2026-04-12

- Always show the footer power slot
- Display `PWR n/a` when the host exposes no usable power telemetry instead of hiding the field entirely
- Use saturating footer padding so the power segment is not squeezed out on narrower terminals
- Tested: `cargo build --release`

## 1.0.5 — 2026-04-12

- Add footer power estimate segment before OOM status when live sensors are available
- Estimate uses CPU package power from Linux powercap/hwmon plus NVIDIA NVML and AMD hwmon GPU power
- Hide the power field when the host exposes no usable power sensors instead of showing a fake value
- Tested: `cargo build --release`

## 1.0.4 — 2026-04-12

- Switch from musl static linking to glibc dynamic linking for NVIDIA GPU compatibility
- musl binaries cannot `dlopen` glibc-linked shared libraries such as `libnvidia-ml.so`, which caused NVIDIA GPU detection to fail
- CI now uses `cross` to build GNU-targeted binaries against an older glibc for portability
- Install script updated to fetch GNU-targeted binaries
- Tested: local build, version check, `ldd` confirms dynamic linking

## 1.0.3 — 2026-03-22

- Static linking via musl — binary now runs on any Linux distro regardless of GLIBC version
- No more "GLIBC_2.xx not found" errors on older systems
- Install script updated for new musl-linked binaries

## 1.0.2 — 2026-03-18

- Show total CPU percentage across all cores (e.g. 400% = 4 cores maxed)
- Show available RAM and disk cache size under memory section

## 1.0.0 — 2026-03-10

- Complete rewrite from Python to Rust — single static binary (~1.2 MB)
- Near-zero CPU and memory overhead (2-5 MB RAM vs 30-50 MB for Python version)
- Instant startup, no runtime dependencies
- One-line install script with automatic upgrades
- GitHub Actions release workflow for x86_64 and aarch64 binaries
- All features from 0.9.0 preserved: 50 themes, NVIDIA + AMD GPU monitoring, sparklines, OOM tracking, process tables, temperature strip

## 0.9.0 — 2026-02-11

- **AMD GPU support** via Linux sysfs — no new dependencies required
- AMD GPUs detected automatically from `/sys/class/drm/card*/device/vendor` (vendor `0x1002`)
- GPU utilization from `gpu_busy_percent`, VRAM from `mem_info_vram_total`/`mem_info_vram_used`
- AMD GPU temperatures from hwmon `temp1_input`/`temp1_crit`
- Mixed NVIDIA+AMD systems show all GPUs together with unified numbering
- Gracefully handles missing sysfs files (older cards, APUs): util→0%, VRAM→0/0 GB, temp→N/A
- GPU name from `product_name` with fallback to PCI device ID
- Strips "AMD " and "Advanced Micro Devices, Inc. " prefixes in GPU panel subtitles
- NVIDIA-only path unchanged — refactored `gpu_ok` → `nvidia_ok` for vendor-specific guards
- Tested: verified no regression on NVIDIA-only system, ran `ktop --sim`, reinstalled via setup.sh

## 0.8.1 — 2026-02-11

- OOM tracker now detects `systemd-oomd` kills in addition to kernel OOM kills
- Fixed `capture_output=True` + `stderr=DEVNULL` conflict that silently broke OOM detection entirely
- Uses `short-unix` journal output for reliable timestamp comparison between OOM sources
- Scope names cleaned up: strips `.scope` suffix and UUIDs for readable display (e.g. `tmux-spawn`)
- Added `__version__` and `ktop --version` flag
- Tested: verified both kernel OOM and systemd-oomd kills detected from real journal entries, reinstalled via setup.sh

## 0.8.0 — 2026-02-10

- CPU process table now shows both Core % (per-core, matches `top`) and CPU % (system-wide)
- Fixed per-process CPU calculation: no longer divides by num_cpus
- Raw binary I/O for `/proc` reads: `os.open`/`os.read`/`os.close` instead of Python file objects
- Binary mode (`rb`) and `split(None, 22)` to avoid decoding overhead and unnecessary allocations
- CPU frequency read from sysfs (`scaling_cur_freq`) instead of `psutil.cpu_freq()` (9ms → 0.02ms), with psutil fallback
- Deferred `/proc/pid/statm` reads: only read for the top 20 displayed processes instead of all ~1690
- `cpu_count` cached at init; `cpu_freq` polled every 5s
- Process CPU baselines seeded at startup for accurate first-frame deltas
- Process tables populate after 1s, refresh every 3s
- Total frame time: 228ms → 20ms (11x improvement)
- Tested: profiled with `--sim`, reinstalled via setup.sh

## 0.7.0 — 2026-02-10

- Process scanning optimized: replaced `psutil.process_iter` with direct `/proc/pid/stat` + `/proc/pid/statm` reads (214ms → 23ms per scan, ~9x faster)
- Process list cached for 5 seconds instead of rescanning every frame
- Added `--sim` flag for simulation mode (fake OOM kills, profiling output to `/tmp/ktop_profile.log`)
- Profiler logs avg/max/calls per section every 5s in sim mode
- OOM kill tracker uses `journalctl` for persistent 8-hour lookback instead of `dmesg` kernel ring buffer
- OOM status shows solid block `█` when OOM detected, hollow `░` when clear
- Tested: profiled with `--sim`, reinstalled via setup.sh

## 0.6.0 — 2026-02-10

- Network panel: sparklines now centered between bar charts — upload sparkline extends upward, download sparkline extends downward using upper-block Unicode characters
- Added `SPARK_DOWN` character set and `_sparkline_down()` for top-down sparklines
- Network upload and download now have separate theme colors (`net_up` defaults to GPU color, `net_down` defaults to net color)
- Theme picker swatches updated to show net_up/net_down colors
- Status bar now shows most recent OOM kill (process name + timestamp) on the right side
- Temperature strip between charts and process tables with hardware-accurate thresholds (GPU slowdown from NVML, CPU critical from psutil, 85°C JEDEC for memory)
- Temperature strip border uses theme `bar_mid` color; entries evenly spaced
- GPU bar charts now use dynamic width matching CPU/memory panel sizing
- Tested: reinstalled via setup.sh

## 0.5.0 — 2026-02-10

- Added Network panel to the second row (layout is now Network, CPU, Memory)
- Shows upload and download bar charts with auto-scaling and sparkline history
- Displays current speed (B/s, KB/s, MB/s, GB/s) and peak observed speed
- Added `net` color to theme system (defaults to CPU color for all existing themes)
- Theme picker preview and swatches updated to include network color
- Added spacing between GPU utilization sparkline and memory bar chart
- Tested: reinstalled via setup.sh

## 0.4.3 — 2026-02-10

- Memory values shown in GB or MB with max 1 decimal place
- Tested: reinstalled via setup.sh

## 0.4.2 — 2026-02-10

- Bar charts now render a smooth per-block gradient from bar_low to bar_high across the full 0-100% width
- Each filled block gets its own interpolated hex color via linear RGB lerp
- RGB conversion cached to avoid re-parsing color names every frame
- Tested: reinstalled via setup.sh

## 0.4.1 — 2026-02-10

- Memory process table now shows Used (RSS−shared) + Shared columns instead of just RSS
- Uses `memory_info().shared` from `/proc/statm` (instant) instead of `memory_full_info()` which reads `/proc/smaps` and was extremely slow on systems with large memory maps, causing ktop to hang on launch
- Tested: reinstalled via setup.sh, launches instantly

## 0.4.0 — 2026-02-10

- Fixed arrow key input: rewrote `_read_key` to use `os.read()` on raw fd instead of buffered `sys.stdin.read()`, so escape sequences are captured atomically
- Responsive input loop: keys polled at 50ms with immediate redraw on keypress
- Theme picker: color swatches (background-colored chips for gpu/cpu/mem/bar) right-aligned next to each theme name with gaps between colors
- Sparklines aligned with bar chart left edge in GPU and CPU panels
- Bar charts now render as gradients: green→yellow→red across the filled region (thresholds at 50% and 80%)
- Tested: reinstalled via setup.sh

## 0.3.1 — 2026-02-10

- Sparklines now match the color of their metric (GPU util, GPU mem, CPU) instead of default white
- Sparklines dynamically fill the width of their enclosing panel (with margin) instead of fixed 20 chars
- Increased history buffer from 60 to 300 samples to support wide terminals
- Tested: reinstalled via setup.sh

## 0.3.0 — 2026-02-10

- Added theme system with 50 color themes (press `t` to open theme picker)
- Arrow keys + Enter to select theme, ESC to cancel
- Theme preference saved to `~/.config/ktop/config.json` and persists across sessions
- `--theme` CLI flag to set theme from command line
- Bottom status bar showing keybindings (q/ESC quit, t themes)
- Proper arrow key handling for theme picker navigation
- Tested: both main view and theme picker render correctly; config persistence works

## 0.2.0 — 2026-02-10

- GPU panels now laid out horizontally — all GPUs visible side by side
- Added q and ESC keys to quit the app (in addition to Ctrl+C)
- Tested: horizontal layout renders correctly with 3 GPUs at 140-col width

## 0.1.1 — 2026-02-10

- Added `setup.sh` installer script — installs `ktop` as a command to `~/.local/bin` (or `/usr/local/bin` with `--system`)
- Suppressed pynvml FutureWarning deprecation noise
- Updated README with quick-install instructions
- Tested: `ktop` command works system-wide after `./setup.sh`

## 0.1.0 — 2026-02-10

- Initial release of ktop
- GPU utilization and memory monitoring with per-GPU sparkline history (NVIDIA)
- CPU usage monitoring with overall bar chart and sparkline history
- RAM and swap usage bar charts
- Top 10 processes by memory usage table
- Top 10 processes by CPU usage table
- Color-coded thresholds (green/yellow/red)
- Configurable refresh rate via `-r` flag
- Tested: renders correctly with 3x NVIDIA RTX 2000 Ada GPUs, 128-core CPU, ~1 TB RAM
