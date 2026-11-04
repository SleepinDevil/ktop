# Repository Guidelines

## Project Overview

ktop is a single-binary, Linux-only terminal (TUI) system resource monitor for hybrid LLM workloads, written in Rust (crate `ktop`, edition 2021). It shows CPU (including a per-core usage grid), memory (incl. PSI pressure, committed/limit, swap I/O), GPU (NVIDIA NVML + AMD sysfs + Intel `xe`/`i915`, with per-GPU power and PCIe rows), network, disks (per-device R/W throughput), temperatures, power draw, process tables, and OOM-kill history. The Rust crate lives in `ktop-rs/`; repo root holds docs, the `install.sh` installer, and GitHub release CI.

## Architecture & Data Flow

Fully synchronous — no async runtime, no tokio. Seven flat modules in `ktop-rs/src/` (~4,100 LOC):

- `main.rs` — entry point: hand-rolled arg parsing (no clap), subcommands (`update`, `uninstall`, `diagnose-terminal`, `diagnose-render`), help text, env-flag reading, uninstall path resolution.
- `app.rs` — `AppState` (one flat pub-field struct), `run()` event loop, terminal setup/teardown, debounced resize, key handling, and `AnsiColorFilter` — a byte-level stdout writer that rewrites SGR `38;5;n`/`48;5;n` (n<16) to classic ANSI in Basic color mode.
- `ui.rs` — pure rendering: `pub fn render(f: &mut Frame, state: &AppState)` plus private `render_*` fns (gpu, cpu, net, mem, disks, temps, procs, status bar, theme picker). No state mutation.
- `system.rs` — data collection: `ProcScanner` (raw `/proc/<pid>/stat` parsing, EMA-smoothed CPU%, top `MAX_PROCS = 20`), `OomTracker` (journalctl grep), `PowerEstimator` (Intel RAPL + hwmon), `NetTracker` (sysinfo deltas, per-direction peaks), `SwapIoTracker` (`pswpin`/`pswpout` from `/proc/vmstat`), plus PSI (`/proc/pressure/memory`) and `Committed_AS`/`CommitLimit` parsing from `/proc/meminfo`, and `DiskTracker` (per-device R/W from `/sys/block/<dev>/stat` deltas, physical disks only).
- `src/gpu.rs` — GPU detection and sampling (NVIDIA NVML, AMD sysfs, Intel `xe`/`i915` sysfs; Intel VRAM via Level Zero sysman dlopen (`L0`, `ZES_ENABLE_SYSMAN=1`) → fdinfo → sysfs, utilization via xe PMU (`perf_event_open`) → DRM fdinfo engine cycles (`fdinfo_scan`), BAR2 VRAM total fallback, PCI device-id name table).
- `theme.rs` — `Theme` struct (~11 `ratatui::style::Color` fields), `theme_names()`, `get_theme(name)`; ~50 built-in themes.
- `config.rs` — `Config { theme, color_mode }` persisted via serde_json to `dirs::config_dir()/ktop/config.json`; IO errors silently ignored.

Data flow: `main()` parses flags/env → `app::run()` → on each refresh tick samples sysinfo, `ProcScanner`, GPU, power, temps, net, swap I/O, and disks into `AppState` (histories are `VecDeque<f64>` capped at `HISTORY_LEN = 300`) → `terminal.draw(|f| ui::render(f, state))` → `CrosstermBackend<AnsiColorFilter<BufWriter<Stdout>>>`, wrapped in `BeginSynchronizedUpdate`/`EndSynchronizedUpdate` unless `--no-sync`/`--compat`.

Key invariants:
- **Compat philosophy**: every draw feature has an opt-out (`--no-alt-screen`, `--no-sync`, `--compat`, `steady_render` flag threaded into ui styling). Fixed viewport + debounced resize (`RESIZE_DEBOUNCE = 250ms`) avoid clear-storms on unstable remote terminals.
- **Basic color mode** must emit zero `38;2;`/`38;5;` foreground SGR — enforced twice: `display_color`/`basic_color` mapping in `ui.rs` and the `rewrite_sgr` stream filter in `app.rs` (defense in depth).
- **glibc dynamic linking is required** (never musl): NVML is dlopen'd at runtime; musl binaries cannot load glibc `libnvidia-ml.so`.

## Key Directories

| Path | Purpose |
|---|---|
| `ktop-rs/src/` | All Rust source (7 modules above) |
| `ktop-rs/Cargo.toml` | Crate manifest; version is source of truth |
| `.github/workflows/release.yml` | Only workflow: tag `v*` → `cross build --release` matrix (`x86_64`/`aarch64-unknown-linux-gnu`), assets named `ktop-<target>` |
| `install.sh` | curl-bash installer; fetches `ktop-${TARGET}` from GitHub releases, installs to `/usr/local/bin/ktop`; asset names must match CI |
| `CHANGELOG.md` | Canonical changelog history (newest-first); `CLAUDE.md` holds mandatory agent rules and points here; `README.md` is user-facing |

## Development Commands

```bash
cd ktop-rs
cargo check              # fast lint
cargo test               # unit tests
cargo build --release    # required after any Rust change
cp target/release/ktop ~/.local/bin/ktop   # install locally (per CLAUDE.md)
```

- Release build profile: `opt-level = 3`, `lto = true`, `strip = true`.
- No rustfmt/clippy config files, no lint CI. CI runs only on tag push.
- **NEVER push to GitHub** without explicitly asking the user (CLAUDE.md).
- Every change requires a changelog entry in `CHANGELOG.md` (`## X.Y.Z — YYYY-MM-DD`, past-tense bullets, ending `- Tested: ...` line). `CHANGELOG.md` is the single canonical history — do not duplicate entries into `CLAUDE.md` or `AGENTS.md`. Keep `Cargo.toml` version in sync; releases are cut by pushing a `v*` tag.

## Code Conventions & Common Patterns

- **Dependencies are deliberately minimal**: crossterm 0.28, ratatui 0.29, sysinfo 0.33, nvml-wrapper 0.10, dirs 6, serde/serde_json, libc. No chrono/regex/glob/clap/anyhow — time formatting uses `libc::localtime_r`, UUID stripping and path globbing are hand-rolled. Do not add new crates casually.
- **Error handling**: subcommand fns return `Result<(), String>` with `.map_err(|e| format!(...))` context; `app::run` returns `Box<dyn std::error::Error>`; unknown CLI args → `exit_usage_error` (exit 2). Sensor reads never fail — degrade via `.ok()` / `unwrap_or` / `unwrap_or_default`.
- **Cleanup-always-runs pattern**: loop body is a closure `(|| -> Result<...>)()` whose result is paired with the terminal-restore result; cleanup always runs, first error wins (see `app::run`, `run_render_diagnostics`).
- **Naming**: SCREAMING_CASE module consts (`HISTORY_LEN`, `MIN_REFRESH_SECS`, `COMPAT_REFRESH_SECS`, `INSTALL_SCRIPT_URL`); `pub fn` for module APIs, private helpers `snake_case`; section dividers `// ── Name ──`; enum CLI cases are PascalCase with kebab-case string parse/name pairs (`RenderDiagnosticCase`).
- **Rendering**: every `ui.rs` render fn starts with `let theme = &state.theme; let truecolor = truecolor_enabled(state);` and routes all colors through `display_color(c, truecolor)`.
- **CLI flags**: `-v/--version`, `-h/--help`, `-r/--refresh <SECS>` (clamped to `MIN_REFRESH_SECS = 0.25`), `--theme <NAME>`, `--sim` (fake OOM events), `--no-alt-screen`, `--no-sync`, `--compat`; env flags via `env_flag()`: `KTOP_COMPAT`, `KTOP_NO_ALT_SCREEN`, `KTOP_NO_SYNC` (accept 1/true/yes/on). `--compat` forces no-alt-screen + no-sync + 2s refresh.

## Important Files

- `ktop-rs/src/main.rs` — entry point, CLI surface, diagnostic subcommands.
- `ktop-rs/src/app.rs` — `AppState`, event loop, `AnsiColorFilter`/`rewrite_sgr`.
- `ktop-rs/src/ui.rs` — all layout/rendering.
- `ktop-rs/Cargo.toml` — version source of truth.
- `install.sh` + `.github/workflows/release.yml` — coupled: asset name `ktop-<target-triple>` must match in both.
- `CLAUDE.md` — mandatory build/copy + changelog + no-push rules; points to `AGENTS.md` for everything else.
- `~/.config/ktop/config.json` — runtime config (`theme`, `color_mode`; truecolor default).

## Runtime/Tooling Preferences

- Rust stable, edition 2021; no pinned toolchain file.
- Linux-only: requires `/proc`, `/sys/class/hwmon`, `/sys/class/powercap`, `/sys/class/drm`, `journalctl`; optional NVIDIA (NVML) / AMD GPUs.
- CI builds via `cross` in older-glibc Docker containers for portability; targets are gnu triples only.
- Install/update path: `curl -sSfL https://raw.githubusercontent.com/SleepinDevil/ktop/master/install.sh | bash` (same command upgrades; `ktop update` re-runs it; `ktop uninstall` removes only the resolved binary). Releases are cut by pushing a `v*` tag to `SleepinDevil/ktop` (origin); `brontoguana/ktop` is `upstream` only.

## Testing & QA

- Framework: std `#[cfg(test)]` + `cargo test` (run from `ktop-rs/`). Unit tests live in `app.rs` (`rewrite_sgr` indexed→classic ANSI mapping, RGB/extended passthrough), `ui.rs` (braille glyph tables/bucketing, per-core grid layout, idle-flicker determinism, ASCII fallbacks), `system.rs` (swap I/O, PSI, `Committed_AS` parsing), and `gpu.rs` (fdinfo scan, PCIe bridge selection). Test names are descriptive `snake_case` sentences, e.g. `leaves_extended_and_rgb_colors_unchanged`.
- No integration tests, no dev-dependencies, no PTY harness checked in; CI runs no tests.
- Manual QA is first-class: `ktop diagnose-terminal --duration 8` (size/resize-event stability) and `ktop diagnose-render --case plain|cursor|color|rgb|unicode|full-paint|clear|alternate|sync --duration 4` (visual repaint isolation); `--sim` for OOM-path profiling. PTY capture smoke tests (e.g. via `script`) verify color-mode SGR invariants.
- Every change's CHANGELOG entry must list what was run, e.g. `- Tested: \`cargo check\`, \`cargo test\`, \`cargo build --release\`, PTY capture ...`.
