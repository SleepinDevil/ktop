# Changelog

## 1.0.27 — 2026-11-04
- Startup update check: ktop now asks the fork's GitHub `releases/latest` endpoint once, on a detached background thread (`curl` subprocess — no HTTP crate), whether a release newer than the compiled-in version exists. The event loop picks the result up with a non-blocking read on the next tick, so startup and the sampling loop are untouched (idle cost unchanged); when a newer release is confirmed the status bar grows a right-hand segment `v<current> | Update available │ ` before the PWR readout — muted version, bold accented flag, vertical-bar separator — and nothing is ever shown while offline, on any check failure, or with the new `--no-update-check` / `KTOP_NO_UPDATE_CHECK=1`
- Memory panel: `committed` label shortened to `comitd` to keep the Committed_AS/CommitLimit line inside narrow terminals
- README: documented the previously undocumented `--no-alt-screen` / `--no-sync` flags and the `KTOP_*` environment flags, replaced the stale `--sim` profiling note (no profile log anymore), and documented `--no-update-check`; app `--help` gained the same flag
  - Tested: `cargo test` (35 tests, new `tag_versions_order_numerically_not_lexically`, `tag_name_is_extracted_from_api_json`), `cargo build --release`, PTY captures of the live status bar (160×50: no segment at the current version, `v0.0.1 | Update available │ PWR …` rendered from a deliberately-downgraded test build of the same code, segment absent with `--no-update-check`), `strace`-free idle check (no curl on the main thread)

## 1.0.26 — 2026-11-04
- The fork's own release channel: `install.sh` now downloads from `SleepinDevil/ktop` releases, the compiled-in `INSTALL_SCRIPT_URL` used by `ktop update` points at `SleepinDevil/ktop/master/install.sh`, and README/AGENTS install and clone URLs match (the shipped v1.0.25 assets still carry the upstream update URL, hence this release); the README gains an "About this fork" preamble describing the fork's focus — idle cost ~0.8 % of a core vs upstream's ~6 %, Intel GPU support, revamped Network/Memory panels, the per-core grid animation (auto-sized to 32 cells, pairing cores up to 64-thread machines), the disks panel, the `s` performance stats page, and the process lists as the layout's vertical adjuster
  - Tested: `cargo test` (33 tests), `cargo build --release`; installer and CI assets verified against the live fork after release

## 1.0.25 — 2026-11-04
- Performance: the Intel GPU fallback no longer resamples the whole machine on every refresh tick — a denied xe PMU counter open is remembered permanently for the session (previously the `perf_event_paranoid` rejection was rediscovered by probing two `perf_event_open` syscalls every tick, which also kept the "utilization needs fdinfo" gate permanently open), the fdinfo walk now caches the `/proc/<pid>/fdinfo` entries that matched the card and re-reads only those — an fd that vanishes (client exited) is dropped, and a full client rediscovery runs only every 15 s or whenever nothing is known — and while fdinfo is the sole utilization source its result is held for 5 s (when only VRAM is missing, per-tick walks continue, incrementally, so the VRAM figure stays as fresh as before). Steady state on a desktop with 4 DRM clients: fdinfo opens ≈ 3,300 per refresh tick → 0 (4 opens per 5 s plus one ~3.5k-open full-walk burst per 15 s), `perf_event_open` 2 per tick → 1 per session; idle at refresh 1 in a 160×50 pty: 5.9 % → **0.86 % of one core** (58 → 8.6 ms/tick). New clients (a game started while ktop runs) are picked up by the next full walk within 15 s
  - Tested: `cargo check`, `cargo test` (31 tests: `fdinfo_scan_computes_util_and_vram_from_fake_proc` migrated to the `FdWalk` API asserting the known-set registration and that an incremental walk reproduces the same delta-based util/VRAM, new `client_fd_churn_drops_known_fds_and_a_full_walk_rediscovers_them` pinning stale-fd drop and re-discovery), `cargo build --release`, timestamped `strace` over 20 s (incremental 4-open bursts every 5 s, full walk exactly at +15 s, exactly one `perf_event_open`), `/proc/<pid>/stat` CPU-time measurement of the same scenario (0.86 % of one core over 22 s), PTY capture with the GPU panel unchanged (Intel util via the held fdinfo source, VRAM via Level Zero)
- Performance: NVIDIA GPU sampling no longer round-trips every NVML call on every refresh tick — the stats page's new per-GPU rows showed the NVIDIA card costing ~43 ms wall/tick vs ~2.7 ms for the Intel one (94 % of the GPU block; NVML blocks per driver call, milliseconds each when the GPU is asleep), so device statics (name, power limit, PCIe BDF, slowdown threshold) are now fetched once and temperature, instant power and the PCIe throughput counters are held for 3 s (`NVML_SLOW_SECS`), while utilization, VRAM and the sysfs PCIe link state stay per-tick — per-tick wall for the card drops from ~43 ms to ~70 µs on hot ticks (~10 ms avg with the 3 s refresh), and the whole tick's wall median drops accordingly
  - Tested: `cargo check`, `cargo test`, `cargo build --release`, PTY captures at refresh 1 before/after on a two-GPU machine (NVIDIA + Intel Arc): stats rows `gpu0` 42.77 → 10.53 ms avg (last-tick 71 µs), `gpu1` unchanged, per-tick wall median 1.92 ms; visible row-set diff before/after identical with live values (VRAM, power limits 275 W / 370 W, PCIe gen/width and NVML PCIe throughput counters updating, utilization bars), `q` quits with exit code 0
- Performance stats page: `s` opens an overlay (status bar shows the ` s Stats` hint) with a per-block timing table of the refresh loop — last/avg/max **wall** duration per tick for cpu, cpu freq, memory, one row per GPU (`gpu0`, `gpu1`, …; the NVML device loop is one call per vendor, so on a multi-NVIDIA card machine its time splits evenly across the NVIDIA rows), temps, network, disks, processes, oom poll and render — sorted hottest-first, and a header line giving the process's own **CPU time** per tick as µs and as % of one core (a ring of `CLOCK_PROCESS_CPUTIME_ID` samples kept alongside the wall rings), plus the wall median/avg/worst on a second line. The two disagree on purpose: the tick spends most of its wall time waiting inside GPU driver calls (here ~47 ms wall vs ~9 ms CPU per tick), and the CPU figure is the one a `%CPU` column shows — an earlier revision of this page derived the percentage from wall time and read ~5 % while the kernel charged the process 1 %. The timing runs on every tick whether or not the page is open (2 extra clock reads per tick beyond the block marks, tens of ns each; 60-tick in-memory rings; recording allocates nothing and writes no files), so opening the page shows what the loop cost before the keypress. The first tick is excluded from the window (it carries one-time startup work — first full walk, `journalctl` launch, initial interface enumeration, first full paint — and would hold the window's mean high for a full minute), and interactive redraws (keys, resizes) are timed into the `render` row too, so every draw is accounted for. `s`/`Esc` close it, `q` quits
  - Tested: `cargo test` (33 tests: new `profiler_reports_only_the_last_window_after_wrapping`, `median_ignores_an_isolated_spike_and_discard_rolls_the_ring_back` and `gpu_rows_share_the_ring_and_get_zeroed_on_reuse` pinning the 60-tick ring, the median, the first-tick rollback, and that per-GPU rows share the ring and get zeroed when its slot is reused), `cargo build --release`, PTY captures at refresh 1: `s` opens the page and `s`/`Esc` close it while `p` no longer does anything, header read `cpu 9.49 ms/tick = 0.95% of one core` in exact agreement with `/proc/<pid>/stat` CPU time over the same 45 s window (95/100 % of a core), wall line showed the driver-wait gap the CPU line no longer hides, per-card `gpu0`/`gpu1` rows sorted the two GPUs by cost, `q` quit with exit code 0, normal UI and status bar otherwise unchanged

## 1.0.24 — 2026-11-04
- Performance: the release profile now builds the binary as a single codegen unit with `panic = "abort"` (alongside the existing `opt-level = 3` + `lto = true`) — unwinding tables and landing pads disappear and LTO sees the whole program; the shipped binary shrank 11 % (1,466 KB → 1,300 KB) with no measurable change in steady-state CPU (paired 30 s samples against the pre-profile build: 181/175 then 177/177 jiffies, inside the run-to-run noise band); `cargo test` is unaffected (dev profile) and release builds take ~13 s instead of ~9 s
  - Tested: `cargo test` (29 tests, dev profile unchanged), `cargo build --release`, launched and interacted with the profile binary normally (panels, keys, quit), paired 30 s `/proc/<pid>/stat` CPU-time A/B across two rounds, user visual confirmation
- Performance: the event loop now sleeps until it has work instead of spinning on a fixed 50 ms poll — `event::poll` gets the remaining time to the next refresh tick (bounded by the resize-debounce window while a resize is pending), keys that change no state (unhandled keypresses) no longer trigger a redraw, the process tables are handed from the scanner to the display state by swap instead of being cloned each scan, and the sparkline helpers scale history values in place (`scale` argument) instead of building a renormalized copy per frame (net up/down, GPU power) — the GPU-util/CPU-history paths additionally dropped their internal per-call value `Vec`
  - Tested: `cargo check`, `cargo test` (29 tests), `cargo build --release`, live PTY session verifying unchanged behavior end to end (refresh ticks advance every panel incl. scaled net/power/util sparklines, `c` color-mode toggle with terminal reset renders Basic mode immediately, theme picker opens/selection applies, `q` quits, Top Processes tables populate through the swap), `strace` over 13 s at `--refresh 1`: event-loop wait syscalls per tick ≈12 → ≈2 and `ioctl` calls −33 % (338 → 225); whole-process CPU unchanged within the run-to-run noise band (concurrent paired 30 s samples: base 187/item6 184, base 182/item6 183 jiffies), user visual confirmation
- Performance: the Memory panel's numbers now come from one `/proc/meminfo` read and one parse per refresh tick (`system::read_mem_sample()`), replacing three separate reads — sysinfo's `refresh_memory()` (kept only to recompute `total − available` and `swap_total − swap_free`, which sysinfo itself parsed from the same file), a second hand-rolled parse for `MemAvailable`/`Cached`, and a third for `Committed_AS`/`CommitLimit` — and the sysinfo `System` is now constructed CPU-only (`MemoryRefreshKind` dropped); displayed values are unchanged because the formulas and the pre-3.14 `MemAvailable`-fallback semantics of the removed code are reproduced exactly, and a failed read keeps the previous values like sysinfo's stale fields did
  - Tested: `cargo check`, `cargo test` (29 tests: the two `parse_committed_limit` tests migrated to the unified parser as `parses_committed_and_limit_kib_as_bytes`/`missing_or_malformed_commit_keys_are_unavailable`, plus new `parses_panel_fields_from_kib_to_bytes` and `absent_memavailable_uses_the_pre_314_estimate`), `cargo build --release`, live Memory panel values cross-checked digit-for-digit against arithmetic on the same `/proc/meminfo` (used/total/avail/cached/swap), `strace` count of `/proc/meminfo` opens dropping from ~3 per refresh tick (13 s window: 40 → 13), user visual confirmation
- Performance: the process sampling pass no longer builds a heap-allocated `ProcInfo` (name `String`, `stat` bytes `Vec`, `splitn` field `Vec`) for every process on the machine — it now parses only the three fields needed for ranking (utime/stime/rss) from a single reused 1 KB read buffer per scan into a compact `RawProc` copy, ranks with two in-place sorts on one vector of 40-byte records, and materializes full rows (name from the stat parentheses, shared memory from `statm`) only for the ≤40 processes that appear in the Top Processes tables; the sort comparators, EMA smoothing, and 20-row cap are unchanged, so the tables render identically including comm values containing spaces or parentheses
  - Tested: `cargo check`, `cargo test` (27 tests: 23 existing plus new `parses_utime_stime_and_rss_after_paren`, `comm_with_spaces_and_parentheses_cannot_shift_fields`, `truncated_or_parenless_stat_is_skipped`, `unparseable_usage_fields_read_as_zero_like_before` pinning the byte-level parser against the old `splitn` indexing), `cargo build --release`, live Top Processes tables cross-checked against `/proc/<pid>/status` VmRSS and statm for the displayed pids (names, Used/Shared, Mem %/CPU %, ordering match), 30 s `/proc/<pid>/stat` CPU-time A/B against the previous build unchanged within the run-to-run drift band (means 182.5 vs 178.5 jiffies/30 s over two rounds each — the win is allocation churn, not syscalls; `strace` shows the whole-machine `stat` byte reads and per-process field/name vectors are gone, traded for ~40 small `stat` re-reads per scan for displayed rows), user visual confirmation
- Performance: OOM-kill detection polls journalctl once a minute instead of every 5 s (the status bar reports kills up to 8 h old, so the slower poll is invisible) and both queries (`-k` `Killed process`, `-u systemd-oomd` `Killed`) now ask for the newest match in the window directly (`-r -n 1`) instead of piping every match of the 8-hour window through a pipe and keeping the last line — on journals with matches the per-poll output is one line and on quiet journals the poll is a pure backwards seek; identical results, including the auto-clear when a kill ages out of the window, and the same skip-on-failure handling (journalctl exits non-zero when nothing matches, verified on systemd 259)
  - Tested: `cargo check`, `cargo test` (23 existing OOM parser tests), `cargo build --release`, shell-level proof that `-r -n 1 --grep` output is byte-identical to the old query's last line (116 matches → 1 line) with match/no-match/boot-separator exit statuses unchanged, live-run showing the status bar's `No OOM kills` state stable across the first poll
- Performance: sysfs sampling now reads only volatile files per refresh tick — a shared hwmon table (`system::hwmon_table()`, discovered once and re-discovered at most every 60 s) resolves every `temp{N}_input`/`temp{N}_crit`/`temp{N}_max`/`power1_*` path the CPU/mem temperature sampling and the CPU power estimator probe, replacing the old per-tick re-enumeration of `/sys/class/hwmon` with hundreds of `ENOENT` probes on machines with many hwmon devices; PCIe `max_link_speed`/`max_link_width` (static while a device is bound) are cached per BDF inside `read_pci_link` while `current_link_*` stays live per tick; AMD/Intel GPU `Pwr` limits from hwmon power-cap attributes are resolved on first read and kept (failures re-probe, so a missing cap behaves exactly as before). All displayed values are identical; NVIDIA power limit stays on NVML as before
  - Tested: `cargo check`, `cargo test` (23 existing tests incl. `detects_intel_gpus_across_xe_and_i915_layouts` exercising `IntelCard::sample` on fake sysfs), `cargo build --release`, live-run 30 s `/proc/<pid>/stat` CPU-time A/B against the previous build (≈7 % process-CPU reduction on a 12-hwmon-device box), visual confirmation GPU/Network/CPUMem/Temps/Disks panels byte-identical including temp bars, limits, and PCIe gen/width rows
- Performance: Intel GPU sampling no longer walks every `/proc/<pid>/fdinfo` on every refresh tick when the results cannot be used — the full-pid scan now runs only when utilization would be lost (xe PMU unavailable, where fdinfo engine cycles are the only source) or VRAM would be lost (neither Level Zero sysman nor `mem_info_vram_*` exposes it); with the PMU and an other VRAM source present the same values are produced from the same sources with ~zero per-tick procfs work
  - Tested: `cargo check`, `cargo test` (`fdinfo_scan_computes_util_and_vram_from_fake_proc`, `detects_intel_gpus_across_xe_and_i915_layouts`), `cargo build --release`, live-run comparison of `pidstat -p <ktop>` CPU before/after and visual confirmation the Intel GPU panel is unchanged
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
