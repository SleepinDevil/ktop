use crate::config::{self, ColorMode};
use crate::gpu::{self, GpuInfo, GpuTemp, NvidiaState};
use crate::system::{self, NetTracker, OomTracker, ProcInfo, ProcScanner};
use crate::theme::{self, Theme};
use crate::ui;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::{Attribute, ResetColor, SetAttribute};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::QueueableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufWriter, Stdout, Write};
use std::time::{Duration, Instant};

use sysinfo::{CpuRefreshKind, Networks, RefreshKind, System};

const HISTORY_LEN: usize = 300;
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(250);
const PROCESS_SCAN_SECS: f64 = 3.0;
const COMPAT_PROCESS_SCAN_SECS: f64 = 10.0;

type TerminalBackend = CrosstermBackend<AnsiColorFilter<BufWriter<Stdout>>>;

struct AnsiColorFilter<W: Write> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: Write> AnsiColorFilter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }

    fn flush_complete(&mut self, force: bool) -> io::Result<()> {
        let mut out = Vec::with_capacity(self.pending.len());
        let mut i = 0;

        while i < self.pending.len() {
            if self.pending[i] != 0x1b {
                let next = self.pending[i..]
                    .iter()
                    .position(|&b| b == 0x1b)
                    .map(|pos| i + pos)
                    .unwrap_or(self.pending.len());
                out.extend_from_slice(&self.pending[i..next]);
                i = next;
                continue;
            }

            if i + 1 >= self.pending.len() {
                if force {
                    out.push(self.pending[i]);
                    i += 1;
                }
                break;
            }

            if self.pending[i + 1] != b'[' {
                out.push(self.pending[i]);
                i += 1;
                continue;
            }

            let Some(end_rel) = self.pending[i + 2..]
                .iter()
                .position(|&b| (0x40..=0x7e).contains(&b))
            else {
                if force {
                    out.extend_from_slice(&self.pending[i..]);
                    i = self.pending.len();
                }
                break;
            };

            let end = i + 2 + end_rel;
            if self.pending[end] == b'm' {
                out.extend(rewrite_sgr(&self.pending[i + 2..end]));
            } else {
                out.extend_from_slice(&self.pending[i..=end]);
            }
            i = end + 1;
        }

        if !out.is_empty() {
            self.inner.write_all(&out)?;
        }
        self.pending.drain(..i);
        Ok(())
    }
}

impl<W: Write> Write for AnsiColorFilter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        self.flush_complete(false)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_complete(true)?;
        self.inner.flush()
    }
}

fn rewrite_sgr(params: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(params) else {
        let mut original = Vec::from(b"\x1b[" as &[u8]);
        original.extend_from_slice(params);
        original.push(b'm');
        return original;
    };

    let parts: Vec<&str> = if text.is_empty() {
        vec!["0"]
    } else {
        text.split(';').collect()
    };
    let mut rewritten = Vec::new();
    let mut i = 0;

    while i < parts.len() {
        if i + 2 < parts.len() && (parts[i] == "38" || parts[i] == "48") && parts[i + 1] == "5" {
            if let Ok(n) = parts[i + 2].parse::<u8>() {
                if n < 16 {
                    rewritten.push(ansi_palette_code(parts[i] == "48", n).to_string());
                    i += 3;
                    continue;
                }
            }
        }

        rewritten.push(parts[i].to_string());
        i += 1;
    }

    format!("\x1b[{}m", rewritten.join(";")).into_bytes()
}

fn ansi_palette_code(background: bool, n: u8) -> u8 {
    match (background, n) {
        (false, 0..=7) => 30 + n,
        (false, 8..=15) => 90 + (n - 8),
        (true, 0..=7) => 40 + n,
        (true, 8..=15) => 100 + (n - 8),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::rewrite_sgr;

    #[test]
    fn rewrites_standard_indexed_foreground_to_classic_ansi() {
        assert_eq!(rewrite_sgr(b"38;5;13"), b"\x1b[95m");
        assert_eq!(rewrite_sgr(b"1;38;5;6;49"), b"\x1b[1;36;49m");
    }

    #[test]
    fn rewrites_standard_indexed_background_to_classic_ansi() {
        assert_eq!(rewrite_sgr(b"48;5;10"), b"\x1b[102m");
    }

    #[test]
    fn leaves_extended_and_rgb_colors_unchanged() {
        assert_eq!(rewrite_sgr(b"38;5;128"), b"\x1b[38;5;128m");
        assert_eq!(rewrite_sgr(b"38;2;1;2;3"), b"\x1b[38;2;1;2;3m");
    }
}

pub struct AppState {
    // Theme
    pub theme_name: String,
    pub theme: Theme,
    pub picking_theme: bool,
    pub theme_cursor: usize,
    pub theme_scroll: usize,
    pub color_mode: ColorMode,

    // CPU
    pub cpu_pct: f64,
    pub cpu_total_pct: f64,
    pub cpu_hist: VecDeque<f64>,
    pub cpu_cores: usize,
    pub cpu_core_pcts: Vec<f64>,
    pub cpu_power_watts: Option<f64>,
    pub cpu_power_denied: bool,
    pub cpu_freq: String,
    pub cpu_name: String,

    // Memory
    pub ram_pct: f64,
    pub ram_used: u64,
    pub ram_total: u64,
    pub ram_available: u64,
    pub ram_cached: u64,
    pub swap_pct: f64,
    pub swap_used: u64,
    pub swap_total: u64,
    // Swap I/O throughput (bytes/sec), from /proc/vmstat pswpin/pswpout.
    pub swap_io_read: f64,
    pub swap_io_write: f64,

    // Memory pressure: PSI `some` stall percentages (avg10/60/300), from
    // /proc/pressure/memory; None when PSI is unavailable.
    pub mem_psi_some: Option<(f64, f64, f64)>,

    // Committed virtual memory and kernel commit limit (bytes), from
    // Committed_AS/CommitLimit in /proc/meminfo; None when unparseable.
    pub mem_committed: Option<(u64, u64)>,

    // GPU
    pub gpu_infos: Vec<GpuInfo>,
    pub gpu_util_hist: HashMap<usize, VecDeque<f64>>,
    pub gpu_mem_hist: HashMap<usize, VecDeque<f64>>,
    pub gpu_power: Vec<Option<f64>>,
    pub gpu_power_hist: HashMap<usize, VecDeque<f64>>,

    // Network
    pub net_up: f64,
    pub net_down: f64,
    pub net_up_hist: VecDeque<f64>,
    pub net_down_hist: VecDeque<f64>,
    pub net_max_speed: f64,
    pub net_max_up: f64,
    pub net_max_down: f64,
    pub disks: Vec<system::DiskStat>,

    // Temps
    pub cpu_temp: Option<f64>,
    pub cpu_temp_max: f64,
    pub mem_temp: Option<f64>,
    pub mem_temp_max: f64,
    pub gpu_temps: Vec<Option<GpuTemp>>,

    // Processes
    pub procs_by_mem: Vec<ProcInfo>,
    pub procs_by_cpu: Vec<ProcInfo>,
    pub num_cpus: u64,

    // OOM
    pub oom_str: Option<String>,
    pub est_power_watts: Option<f64>,
    pub steady_render: bool,
    // Wall-clock reference for the per-second core-glyph reseed.
    pub render_base: Instant,
    // Per-process jitter so paired glyph picks differ across runs.
    pub rand_base: u64,
    // Effective refresh interval (drives the idle-flicker frame counter).
    pub refresh_secs: f64,
}

pub fn run(
    refresh: f64,
    sim: bool,
    theme_override: Option<String>,
    no_alt_screen: bool,
    no_sync: bool,
    compat: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Init system (CPU only — memory comes from a direct /proc/meminfo read)
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_cpu_usage()),
    );
    let mut networks = Networks::new_with_refreshed_list();

    // First CPU sample (baseline)
    sys.refresh_cpu_usage();
    std::thread::sleep(Duration::from_millis(100));

    // Init GPU
    let nvidia = NvidiaState::init();
    let nvidia_count = nvidia.as_ref().map(|n| n.count).unwrap_or(0);
    let amd_cards = gpu::detect_amd_gpus();
    let mut intel_cards = gpu::detect_intel_gpus();
    let intel_base = nvidia_count + amd_cards.len();
    let gpu_count = intel_base + intel_cards.len();

    // Init histories
    let mut gpu_util_hist: HashMap<usize, VecDeque<f64>> = HashMap::new();
    let mut gpu_mem_hist: HashMap<usize, VecDeque<f64>> = HashMap::new();
    let mut gpu_power_hist: HashMap<usize, VecDeque<f64>> = HashMap::new();
    for i in 0..gpu_count {
        gpu_util_hist.insert(i, VecDeque::with_capacity(HISTORY_LEN));
        gpu_mem_hist.insert(i, VecDeque::with_capacity(HISTORY_LEN));
        gpu_power_hist.insert(i, VecDeque::with_capacity(HISTORY_LEN));
    }

    // Load theme
    let cfg = config::load_config();
    let theme_name = theme_override
        .or_else(|| cfg.theme.clone())
        .unwrap_or_else(|| "Vaporwave".to_string());
    let color_mode = cfg.color_mode.unwrap_or_default();
    let theme_name = if theme::theme_names().contains(&theme_name.as_str()) {
        theme_name
    } else {
        "Default".to_string()
    };

    let names = theme::theme_names();
    let theme_cursor = names.iter().position(|&n| n == theme_name).unwrap_or(0);

    // Init other subsystems
    let mut net_tracker = NetTracker::new();
    let mut disk_tracker = system::DiskTracker::new();
    let mut proc_scanner = ProcScanner::new();
    let mut oom_tracker = OomTracker::new(sim);
    let mut swap_tracker = system::SwapIoTracker::new();
    let mut power_estimator = system::PowerEstimator::new();
    let mut last_freq_check = Instant::now() - Duration::from_secs(10);

    // Terminal setup
    terminal::enable_raw_mode()?;
    let mut stdout = AnsiColorFilter::new(BufWriter::new(io::stdout()));
    if !no_alt_screen {
        crossterm::execute!(stdout, EnterAlternateScreen)?;
    }
    crossterm::execute!(stdout, Hide)?;
    let backend = CrosstermBackend::new(stdout);
    let mut active_area = terminal_area()?;
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fixed(active_area),
        },
    )?;
    if !no_alt_screen || compat {
        terminal.clear()?;
    }

    let mut state = AppState {
        theme_name: theme_name.clone(),
        theme: theme::get_theme(&theme_name),
        picking_theme: false,
        theme_cursor,
        theme_scroll: 0,
        color_mode,
        cpu_pct: 0.0,
        cpu_total_pct: 0.0,
        cpu_hist: VecDeque::with_capacity(HISTORY_LEN),
        cpu_cores: sys.cpus().len(),
        cpu_core_pcts: Vec::new(),
        cpu_freq: system::read_cpu_freq().unwrap_or_else(|| "N/A".to_string()),
        cpu_power_watts: None,
        cpu_power_denied: false,
        cpu_name: system::read_cpu_name(),
        ram_pct: 0.0,
        ram_used: 0,
        ram_total: 0,
        ram_available: 0,
        ram_cached: 0,
        swap_pct: 0.0,
        swap_used: 0,
        swap_total: 0,
        swap_io_read: 0.0,
        swap_io_write: 0.0,
        mem_psi_some: None,
        mem_committed: None,
        gpu_infos: Vec::new(),
        gpu_util_hist,
        gpu_mem_hist,
        gpu_power: Vec::new(),
        gpu_power_hist,
        net_up: 0.0,
        net_down: 0.0,
        net_up_hist: VecDeque::with_capacity(HISTORY_LEN),
        net_down_hist: VecDeque::with_capacity(HISTORY_LEN),
        net_max_speed: 1.0,
        net_max_up: 1.0,
        net_max_down: 1.0,
        disks: Vec::new(),
        cpu_temp: None,
        cpu_temp_max: 100.0,
        mem_temp: None,
        mem_temp_max: 85.0,
        gpu_temps: Vec::new(),
        procs_by_mem: Vec::new(),
        procs_by_cpu: Vec::new(),
        num_cpus: proc_scanner.num_cpus(),
        oom_str: None,
        est_power_watts: None,
        steady_render: compat,
        render_base: Instant::now(),
        rand_base: (std::process::id() as u64) ^ 0x9E37_79B9_7F4A_7C15,
        refresh_secs: refresh,
    };

    let refresh_dur = Duration::from_secs_f64(refresh);
    let mut last_refresh = Instant::now() - refresh_dur;
    let mut pending_resize: Option<(Rect, Instant)> = None;
    let mut core_smooth: Vec<f64> = Vec::new();

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        loop {
            let now = Instant::now();

            // Wake exactly when there is work: the next refresh tick, or —
            // while a resize is debouncing — when the debounce can settle.
            // Key/resize events wake the poll immediately regardless.
            let mut wait = refresh_dur.saturating_sub(now.duration_since(last_refresh));
            if let Some((_, since)) = pending_resize {
                let debounce_left = RESIZE_DEBOUNCE.saturating_sub(now.duration_since(since));
                if debounce_left < wait {
                    wait = debounce_left;
                }
            }
            if event::poll(wait)? {
                match event::read()? {
                    Event::Key(key) => {
                        let color_mode_before = state.color_mode;
                        let (quit, changed) = handle_key(key, &mut state);
                        if quit {
                            break;
                        }
                        if state.color_mode != color_mode_before {
                            reset_and_clear_terminal(&mut terminal)?;
                        }
                        // Immediate redraw when the key changed something;
                        // unhandled keys leave the screen untouched.
                        if changed && pending_resize.is_none() {
                            draw_app(&mut terminal, &state, !no_sync)?;
                        }
                        continue;
                    }
                    Event::Resize(width, height) => {
                        note_resize(
                            &mut pending_resize,
                            Rect::new(0, 0, width.max(1), height.max(1)),
                            now,
                        );
                        continue;
                    }
                    _ => {}
                }
            }

            if let Some(area) = stable_resize(&mut pending_resize, active_area, now)? {
                active_area = area;
                terminal.resize(active_area)?;
                draw_app(&mut terminal, &state, !no_sync)?;
                continue;
            }
            if pending_resize.is_some() {
                continue;
            }

            if now.duration_since(last_refresh) >= refresh_dur {
                last_refresh = now;

                // Sample CPU
                sys.refresh_cpu_usage();
                state.cpu_pct = sys.global_cpu_usage() as f64;
                let core_raws: Vec<f64> =
                    sys.cpus().iter().map(|c| c.cpu_usage() as f64).collect();
                state.cpu_total_pct = core_raws.iter().sum();
                // EMA-smooth per-core usage (same 0.65/0.35 blend as the
                // process table) so the grid cells don't flicker between ticks
                if core_smooth.len() != core_raws.len() {
                    core_smooth = core_raws.clone();
                } else {
                    for (s, r) in core_smooth.iter_mut().zip(core_raws.iter()) {
                        *s = *s * 0.65 + r * 0.35;
                    }
                }
                state.cpu_core_pcts = core_smooth.clone();
                state.cpu_hist.push_back(state.cpu_pct);
                if state.cpu_hist.len() > HISTORY_LEN {
                    state.cpu_hist.pop_front();
                }

                // CPU frequency (every 5s)
                if now.duration_since(last_freq_check) >= Duration::from_secs(5) {
                    last_freq_check = now;
                    state.cpu_freq =
                        system::read_cpu_freq().unwrap_or_else(|| state.cpu_freq.clone());
                }

                // Memory — one /proc/meminfo read feeds every field the
                // panel shows; used/swap_used are the same differences
                // sysinfo computed (total − available, total − free). On a
                // read failure the previous values stay, like sysinfo's
                // stale fields did.
                if let Some(m) = system::read_mem_sample() {
                    state.ram_total = m.total;
                    state.ram_available = m.available;
                    state.ram_cached = m.cached;
                    state.ram_used = state.ram_total.saturating_sub(state.ram_available);
                    state.ram_pct = if state.ram_total > 0 {
                        state.ram_used as f64 / state.ram_total as f64 * 100.0
                    } else {
                        0.0
                    };
                    state.swap_total = m.swap_total;
                    state.swap_used = m.swap_total.saturating_sub(m.swap_free);
                    state.swap_pct = if state.swap_total > 0 {
                        state.swap_used as f64 / state.swap_total as f64 * 100.0
                    } else {
                        0.0
                    };
                    state.mem_committed = m.committed;
                }
                let (swap_r, swap_w) = swap_tracker.sample();
                state.swap_io_read = swap_r;
                state.swap_io_write = swap_w;
                state.mem_psi_some = system::read_mem_psi_some();

                // GPUs
                let mut gpu_infos = Vec::new();
                if let Some(ref nv) = nvidia {
                    gpu_infos.extend(nv.sample(0));
                }
                for (j, card) in amd_cards.iter().enumerate() {
                    gpu_infos.push(card.sample(nvidia_count + j));
                }
                for (j, card) in intel_cards.iter_mut().enumerate() {
                    gpu_infos.push(card.sample(intel_base + j));
                }
                for g in &gpu_infos {
                    if g.has_util {
                        if let Some(hist) = state.gpu_util_hist.get_mut(&g.id) {
                            hist.push_back(g.util);
                            if hist.len() > HISTORY_LEN {
                                hist.pop_front();
                            }
                        }
                    }
                    if g.has_mem {
                        if let Some(hist) = state.gpu_mem_hist.get_mut(&g.id) {
                            hist.push_back(g.mem_pct);
                            if hist.len() > HISTORY_LEN {
                                hist.pop_front();
                            }
                        }
                    }
                }
                state.gpu_infos = gpu_infos;

                // GPU temps
                let mut gpu_temps = Vec::new();
                if let Some(ref nv) = nvidia {
                    gpu_temps.extend(nv.temps());
                }
                for card in &amd_cards {
                    gpu_temps.push(card.temp());
                }
                for card in &intel_cards {
                    gpu_temps.push(card.temp());
                }
                state.gpu_temps = gpu_temps;

                // Per-GPU live power (NVML mW / AMD power1_average / Intel
                // energy-delta), sampled once per tick and reused by the
                // footer estimate. Order matches gpu_infos: NVIDIA, AMD, Intel.
                let cpu_power = power_estimator.sample_cpu_watts();
                state.cpu_power_watts = cpu_power;
                state.cpu_power_denied = power_estimator.access_denied;
                let mut gpu_power: Vec<Option<f64>> = Vec::new();
                if let Some(nv) = &nvidia {
                    gpu_power.extend(nv.power_watts());
                }
                for card in amd_cards.iter() {
                    gpu_power.push(card.power_watts());
                }
                for card in intel_cards.iter_mut() {
                    gpu_power.push(card.power_watts());
                }
                let gpu_power_total: f64 = gpu_power.iter().flatten().sum();
                for (g, p) in state.gpu_infos.iter().zip(gpu_power.iter()) {
                    if let Some(w) = p {
                        if let Some(hist) = state.gpu_power_hist.get_mut(&g.id) {
                            hist.push_back(*w);
                            if hist.len() > HISTORY_LEN {
                                hist.pop_front();
                            }
                        }
                    }
                }
                state.gpu_power = gpu_power;
                let total_power = cpu_power.unwrap_or(0.0) + gpu_power_total;
                state.est_power_watts = if cpu_power.is_some() || gpu_power_total > 0.0 {
                    Some(total_power)
                } else {
                    None
                };

                // CPU/mem temps via the hwmon table
                sample_temps(&mut state);

                // Network
                networks.refresh(true);
                let (up, down) = net_tracker.sample(&networks);
                state.net_up = up;
                state.net_down = down;
                state.net_max_speed = net_tracker.max_speed;
                state.net_max_up = net_tracker.max_up;
                state.net_max_down = net_tracker.max_down;
                state.net_up_hist.push_back(up);
                state.net_down_hist.push_back(down);
                if state.net_up_hist.len() > HISTORY_LEN {
                    state.net_up_hist.pop_front();
                }
                if state.net_down_hist.len() > HISTORY_LEN {
                    state.net_down_hist.pop_front();
                }

                // Disks: live per-device read/write throughput.
                state.disks = disk_tracker.sample();

                // Processes
                let proc_scan_secs = if state.steady_render {
                    COMPAT_PROCESS_SCAN_SECS
                } else {
                    PROCESS_SCAN_SECS
                };
                if proc_scanner.scan(state.ram_total, proc_scan_secs) {
                    // Hand the freshly built tables to the state without
                    // copying; the scanner never reads them back between builds.
                    std::mem::swap(&mut state.procs_by_mem, &mut proc_scanner.by_mem);
                    std::mem::swap(&mut state.procs_by_cpu, &mut proc_scanner.by_cpu);
                }

                // OOM
                oom_tracker.check();
                state.oom_str = oom_tracker.last_oom.clone();

                draw_app(&mut terminal, &state, !no_sync)?;
            }
        }
        Ok(())
    })();

    // Cleanup
    let cleanup_result = restore_terminal(&mut terminal, !no_alt_screen, compat);
    match (result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(e)) => Err(Box::new(e)),
        (Err(e), _) => Err(e),
    }
}

fn terminal_area() -> io::Result<Rect> {
    let (width, height) = terminal::size()?;
    Ok(Rect::new(0, 0, width.max(1), height.max(1)))
}

fn note_resize(pending_resize: &mut Option<(Rect, Instant)>, area: Rect, now: Instant) {
    match pending_resize {
        Some((pending_area, since)) if *pending_area == area => {
            if now < *since {
                *since = now;
            }
        }
        _ => {
            *pending_resize = Some((area, now));
        }
    }
}

fn stable_resize(
    pending_resize: &mut Option<(Rect, Instant)>,
    active_area: Rect,
    now: Instant,
) -> io::Result<Option<Rect>> {
    let observed_area = terminal_area()?;
    if observed_area == active_area {
        *pending_resize = None;
        return Ok(None);
    }

    note_resize(pending_resize, observed_area, now);
    if let Some((area, since)) = *pending_resize {
        if now.duration_since(since) >= RESIZE_DEBOUNCE {
            *pending_resize = None;
            return Ok(Some(area));
        }
    }

    Ok(None)
}

fn draw_app(
    terminal: &mut Terminal<TerminalBackend>,
    state: &AppState,
    sync_updates: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if sync_updates {
        terminal.backend_mut().queue(BeginSynchronizedUpdate)?;
    }

    let draw_result = terminal.draw(|f| ui::render(f, state)).map(|_| ());

    if sync_updates {
        let end_result = terminal
            .backend_mut()
            .queue(EndSynchronizedUpdate)
            .and_then(|backend| backend.flush());
        if draw_result.is_ok() {
            end_result?;
        }
    }

    draw_result?;
    Ok(())
}

fn reset_and_clear_terminal(terminal: &mut Terminal<TerminalBackend>) -> io::Result<()> {
    crossterm::execute!(
        terminal.backend_mut(),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    terminal.clear()?;
    terminal.backend_mut().flush()?;
    Ok(())
}

fn restore_terminal(
    terminal: &mut Terminal<TerminalBackend>,
    alt_screen: bool,
    compat: bool,
) -> io::Result<()> {
    terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        SetAttribute(Attribute::Reset),
        ResetColor,
        Show
    )?;
    if alt_screen {
        crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    } else if compat {
        crossterm::execute!(
            terminal.backend_mut(),
            Clear(ClearType::All),
            crossterm::cursor::MoveTo(0, 0)
        )?;
    } else {
        writeln!(terminal.backend_mut())?;
    }
    terminal.backend_mut().flush()?;
    Ok(())
}

/// Returns `(quit, redraw)`: whether the app should exit, and whether the
/// key changed any state the UI shows. Unhandled keys report `false` so
/// no-op keypresses skip the full redraw.
fn handle_key(key: KeyEvent, state: &mut AppState) -> (bool, bool) {
    let names = theme::theme_names();
    let total = names.len();
    let cols = 3;

    if state.picking_theme {
        match key.code {
            KeyCode::Esc => state.picking_theme = false,
            KeyCode::Enter => {
                if let Some(&name) = names.get(state.theme_cursor) {
                    state.theme_name = name.to_string();
                    state.theme = theme::get_theme(name);
                    state.picking_theme = false;
                    config::save_config(&config::Config {
                        theme: Some(name.to_string()),
                        color_mode: Some(state.color_mode),
                    });
                }
            }
            KeyCode::Up => {
                state.theme_cursor = state.theme_cursor.saturating_sub(cols);
            }
            KeyCode::Down => {
                state.theme_cursor = (state.theme_cursor + cols).min(total - 1);
            }
            KeyCode::Left => {
                state.theme_cursor = state.theme_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                state.theme_cursor = (state.theme_cursor + 1).min(total - 1);
            }
            _ => return (false, false),
        }
        return (false, true);
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => return (true, false),
        KeyCode::Esc => return (true, false),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return (true, false)
        }
        KeyCode::Char('t') | KeyCode::Char('T') => {
            state.picking_theme = true;
            state.theme_cursor = names
                .iter()
                .position(|&n| n == state.theme_name)
                .unwrap_or(0);
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            state.color_mode = state.color_mode.toggle();
            config::save_config(&config::Config {
                theme: Some(state.theme_name.clone()),
                color_mode: Some(state.color_mode),
            });
        }
        _ => return (false, false),
    }
    (false, true)
}

fn sample_temps(state: &mut AppState) {
    // Read only the sensor files the cached hwmon table resolved to exist
    // (values are re-read every tick; see system::hwmon_table)
    state.cpu_temp = None;
    state.cpu_temp_max = 100.0;
    state.mem_temp = None;
    state.mem_temp_max = 85.0;

    for dev in system::hwmon_table().iter() {
        // CPU temps
        if matches!(
            dev.name.as_str(),
            "coretemp" | "k10temp" | "cpu_thermal" | "zenpower" | "acpitz"
        ) {
            for input in &dev.temp_inputs {
                if let Some(temp) = read_hwmon_millideg(input) {
                    state.cpu_temp =
                        Some(state.cpu_temp.map(|t: f64| t.max(temp)).unwrap_or(temp));
                }
            }
            for (crit, high) in &dev.temp_limits {
                // crit wins when present; _max only when the crit file is absent
                if let Some(val) = crit.as_ref().and_then(|p| read_hwmon_millideg(p)) {
                    if val > state.cpu_temp_max {
                        state.cpu_temp_max = val;
                    }
                } else if let Some(val) = high.as_ref().and_then(|p| read_hwmon_millideg(p)) {
                    if val > state.cpu_temp_max {
                        state.cpu_temp_max = val;
                    }
                }
            }
        }

        // Memory temps (SODIMM, dimm, memory)
        if matches!(dev.name.as_str(), "SODIMM" | "dimm" | "memory") {
            for input in dev.temp_inputs.iter().take(4) {
                if let Some(temp) = read_hwmon_millideg(input) {
                    state.mem_temp =
                        Some(state.mem_temp.map(|t: f64| t.max(temp)).unwrap_or(temp));
                }
            }
            for crit in dev.temp_limits.iter().take(4).filter_map(|(crit, _)| crit.as_ref()) {
                if let Some(val) = read_hwmon_millideg(crit) {
                    state.mem_temp_max = val;
                }
            }
        }
    }
}

fn read_hwmon_millideg(path: &std::path::Path) -> Option<f64> {
    let val = std::fs::read_to_string(path).ok()?;
    let millideg = val.trim().parse::<f64>().ok()?;
    Some(millideg / 1000.0)
}
