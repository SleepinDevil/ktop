# ktop

![ktop screenshot](screenshot.png?v=6fadd5e)

A terminal-based system resource monitor built for tracking resource usage when running hybrid LLM workloads.

## About this fork

This is **SleepinDevil's fork** of [brontoguana/ktop](https://github.com/brontoguana/ktop). Everything from upstream is here; on top of it:

- **Significant performance improvements** — the upstream application uses roughly 6% of a single core while idle; this fork uses roughly 0.8% at the same refresh rate, by sampling only what changes: one `/proc` read per panel, cached sysfs/hwmon paths, fdinfo/NVML query caching, and a poll-based event loop
- **Intel GPU support** — detection and full per-GPU panels for Intel Arc cards (`xe`/`i915`): utilization, VRAM, temperature, and power, no root required
- **Revamped Network and Memory panels** — more detail (per-direction peak speeds, PSI memory pressure, committed vs commit-limit, swap I/O) with the three mid-row panel heights balanced so the layout sits evenly
- **Grid activation core animation** — a per-core braille heat grid that automatically sizes itself up to 32 cores; with more cores than that it pairs them up for the UI, supporting up to 64 cores in the graphic
- **Disks panel** — live per-device read/write throughput (great for projects like XeStrata doing their thing with disk reads)
- **Performance stats page** (`s`) — per-block and per-GPU timing of the sampling loop, which is how we found that the NVIDIA software stack is on average far slower to answer info queries than the other vendors' — and made it 4× cheaper
- **Process lists as the vertical height adjuster** — the Top Processes panels absorb or shrink for all vertical slack first, so GPU, CPU/memory/network, temperature, and disk panels keep their exact sizes at any terminal height

![Linux](https://img.shields.io/badge/platform-linux-blue)

## Features

### Latest

- **Intel Arc GPU support** — Alchemist/Battlemage Arc cards via `xe`/`i915` (Arc Pro B70, Arc A770/A750/A380/A310, Arc B580): utilization, VRAM, temperature, and power — no root required
- **Per-GPU power & PCIe rows** — live watts scaled to each card's driver power limit with history sparkline, live PCIe link speed/width vs max, and PCIe transfer speed (NVIDIA)
- **Disks panel** — live per-device Read/Write throughput for physical disks, NVMe first
- **Per-core CPU grid** — braille per-core usage heat-grid with core-count, total %, frequency, and package-power readouts
- **Deeper memory panel** — PSI memory pressure, committed vs commit-limit, and swap I/O throughput
- **Status-bar version stamp** — bottom right always shows the running version; a background release check pops `| Update available` into it when the fork has a newer release (disable with `--no-update-check` or `KTOP_NO_UPDATE_CHECK=1`)
- One-line install and upgrade: `curl -sSfL https://raw.githubusercontent.com/SleepinDevil/ktop/master/install.sh | bash`
- No runtime dependencies — no Python, no pip, no venv

### Core

- **GPU Monitoring** — Per-GPU utilization and memory usage with color-coded sparkline history (NVIDIA + AMD + Intel Arc)
- **GPU Power & PCIe** — Per-GPU live power in watts scaled to the card's driver power limit with bar and history sparkline (NVIDIA NVML + AMD hwmon + Intel hwmon), live PCIe link speed/width vs device max, and real-time PCIe transfer speed (NVIDIA via NVML; Intel/AMD expose no link byte counters)
- **Network Monitoring** — Upload/download speeds with separate colored sparklines (upload extends up, download extends down) and per-direction peak speeds
- **CPU Monitoring** — Overall CPU usage with gradient bar chart and sparkline history, plus a per-core braille usage grid with core-count, total %, frequency, and CPU package power readouts
- **Memory Monitoring** — RAM and swap usage with gradient progress bars, memory pressure (PSI), committed vs commit-limit, and swap I/O throughput
- **Disk Monitoring** — Live per-device Read/Write throughput from `/sys/block` stats (physical disks only, NVMe drives sorted first)
- **Temperature Strip** — CPU, memory, and per-GPU temps with mini bar charts and hardware-accurate thresholds
- **OOM Kill Tracker** — Status bar shows the most recent OOM kill from the last 8 hours (kernel OOM and systemd-oomd)
- **Process Tables** — Top 20 processes by memory (Used/Shared) and CPU usage (Core % + system-wide CPU %)
- **50 Color Themes** — Press `t` to browse and switch themes with live preview; persists across sessions
- **Color Mode Toggle** — Press `c` to switch between truecolor and safe Basic color output; persists across sessions
- **Gradient Bar Charts** — Smooth per-block color gradients from low to high across all bars
- **Responsive UI** — 50ms input polling for snappy keyboard navigation

## Install

```bash
curl -sSfL https://raw.githubusercontent.com/SleepinDevil/ktop/master/install.sh | bash
```

Downloads the latest binary and installs it to `/usr/local/bin` (will prompt for sudo if needed). Run the same command again to upgrade.

Installed versions can also update themselves:

```bash
ktop update
```

To remove only the installed `ktop` executable and leave user config untouched:

```bash
ktop uninstall
```

### Build from source

```bash
git clone https://github.com/SleepinDevil/ktop.git
cd ktop/ktop-rs
cargo build --release
sudo cp target/release/ktop /usr/local/bin/
```

## Usage

```bash
# Run with defaults (1s refresh)
ktop

# Custom refresh rate
ktop -r 2

# Start with a specific theme
ktop --theme "Tokyo Night"

# Simulation mode (fake OOM kills in the status bar)
ktop --sim

# Draw in the current screen instead of the alternate screen
ktop --no-alt-screen

# Disable synchronized (atomic) terminal redraws
ktop --no-sync

# Skip the background GitHub update check at startup
ktop --no-update-check

# Remote terminal fallback if the display flashes
ktop --compat

# Report terminal size and resize behavior
ktop diagnose-terminal

# Isolate which terminal repaint behavior flashes
ktop diagnose-render --case full-paint

# Show version
ktop --version

# Update or remove ktop
ktop update
ktop uninstall
```

Environment variables (all accept `1`/`true`/`yes`/`on`): `KTOP_COMPAT`, `KTOP_NO_ALT_SCREEN`, `KTOP_NO_SYNC`, `KTOP_NO_UPDATE_CHECK` — equivalents of `--compat`, `--no-alt-screen`, `--no-sync`, and `--no-update-check`.

### Keybindings

| Key | Action |
|-----|--------|
| `q` / `ESC` | Quit |
| `t` | Open theme picker |
| `s` | Performance stats page: per-block timing of the refresh loop and ktop's own CPU time; `s`/`Esc` closes |
| `c` | Toggle color mode between truecolor and safe Basic color output |
| Arrow keys | Navigate theme picker |
| `Enter` | Select theme |

### Remote Terminal Compatibility

If a web or remote-support terminal flashes while repainting, try:

```bash
ktop --compat
```

Compatibility mode disables alternate screen and synchronized redraws, slows the default refresh to 2s, and uses lower-churn rendering by suppressing moving sparklines and refreshing process tables less often.
It also uses coarse progress bars instead of per-cell gradients, avoids dark-gray ANSI styling and shaded filler glyphs, and clears the current screen on entry/exit so stale styled cells do not remain behind the shell prompt.

You can also set `KTOP_COMPAT=1`.

If a terminal flashes on extended color output, press `c` in the main view to switch Color from `Truecolor` to `Basic`. Basic mode avoids RGB and indexed foreground color escapes and is saved in `~/.config/ktop/config.json`.

To check whether a terminal is reporting unstable sizes or repeated resize events:

```bash
ktop diagnose-terminal --duration 8
```

If the size report is stable but ktop still flashes, isolate the repaint trigger:

```bash
ktop diagnose-render --case plain --duration 4
ktop diagnose-render --case cursor --duration 4
ktop diagnose-render --case color --duration 4
ktop diagnose-render --case rgb --duration 4
ktop diagnose-render --case unicode --duration 4
ktop diagnose-render --case full-paint --duration 4
ktop diagnose-render --case clear --duration 4
ktop diagnose-render --case alternate --duration 4
ktop diagnose-render --case sync --duration 4
```

Report which cases visibly flash. Each case exits by itself.

If a command unexpectedly starts the monitor, check `ktop --version`; terminal diagnostics and compatibility mode require ktop 1.0.10 or newer.

## Requirements

- Linux (reads `/proc` and sysfs directly)
- NVIDIA GPU + drivers (optional — for NVIDIA monitoring)
- AMD GPU + `amdgpu` driver (optional — for AMD monitoring)
- **Intel Arc** (Alchemist/Battlemage via `xe`/`i915`): temperature, power, VRAM, and utilization. VRAM used/total comes from Level Zero sysman (`libze_loader.so.1`, loaded at runtime when installed — e.g. the Intel compute stack / XeStrata) and falls back to DRM fdinfo (`/proc/<pid>/fdinfo`, your own GPU processes) and `mem_info_vram_used` (i915); the total falls back to the PCI BAR2 aperture. Utilization uses the xe PMU (whole card, requires `perf_event_paranoid <= 0`) and falls back to DRM fdinfo engine cycles (your processes, no root needed); shows `n/a` when neither is available.
- No runtime dependencies — single static binary

## License

MIT
