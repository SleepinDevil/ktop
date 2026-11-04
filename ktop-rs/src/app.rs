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
    use super::{ProfBlock, Profiler};

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

    #[test]
    fn profiler_reports_only_the_last_window_after_wrapping() {
        let mut p = Profiler::new();
        assert_eq!(p.samples(), 0);
        assert_eq!(p.stats(ProfBlock::Total).avg_us, 0.0);
        // 130 ticks with value = tick index; only the last 60 stay visible.
        for i in 0..130usize {
            p.begin_slot();
            p.add(ProfBlock::Total, i as f64);
        }
        let s = p.stats(ProfBlock::Total);
        assert_eq!(p.samples(), super::PROF_HISTORY);
        assert_eq!(s.last_us, 129.0);
        assert_eq!(s.max_us, 129.0);
        // Mean and median of ticks 70..=129.
        assert!((s.avg_us - 99.5).abs() < 1e-9);
        assert!((p.median(ProfBlock::Total) - 99.5).abs() < 1e-9);
    }

    #[test]
    fn gpu_rows_share_the_ring_and_get_zeroed_on_reuse() {
        let mut p = Profiler::new();
        assert_eq!(p.gpu_count(), 0);
        assert_eq!(p.stats_gpu(0).avg_us, 0.0);
        // First tick grows the ring set to two cards.
        p.begin_slot();
        p.add(ProfBlock::Total, 1.0);
        p.add_gpu(0, 10.0);
        p.add_gpu(1, 20.0);
        assert_eq!(p.gpu_count(), 2);
        // A later tick that skips the slow path must not inherit the old
        // value: begin_slot zeroes the shared slot for gpu rows too.
        for _ in 0..70 {
            p.begin_slot();
            p.add(ProfBlock::Total, 1.0);
            p.add_gpu(0, 40.0);
            p.add_gpu(1, 0.0);
        }
        let g0 = p.stats_gpu(0);
        assert_eq!(g0.last_us, 40.0);
        assert!((g0.avg_us - 40.0).abs() < 1e-9);
        assert_eq!(p.stats_gpu(1).last_us, 0.0);
    }

    #[test]
    fn median_ignores_an_isolated_spike_and_discard_rolls_the_ring_back() {
        let mut p = Profiler::new();
        for i in 0..10usize {
            p.begin_slot();
            p.add(ProfBlock::Total, if i == 5 { 9000.0 } else { i as f64 });
        }
        // One 9000 tick (a full-walk tick) moves the mean but not the median
        // (sorted middle pair of 0,1,2,3,4,[9000],6,7,8,9 is 4 and 6).
        assert!((p.median(ProfBlock::Total) - 5.0).abs() < 1e-9);
        assert!(p.stats(ProfBlock::Total).avg_us > 900.0);
        // Discarding the newest tick leaves the previous nine, and the next
        // recorded tick overwrites the discarded slot.
        p.discard_last();
        assert_eq!(p.samples(), 9);
        assert_eq!(p.stats(ProfBlock::Total).last_us, 8.0);
        p.begin_slot();
        p.add(ProfBlock::Total, 7.0);
        assert_eq!(p.samples(), 10);
        assert_eq!(p.stats(ProfBlock::Total).last_us, 7.0);
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
    // In-process performance profile of the sampling loop (shown with `p`).
    pub prof: Profiler,
    pub show_stats: bool,
}

// ── Performance profile ──
// The event loop times its sampling blocks and the draw into a fixed ring of
// the last PROF_HISTORY ticks. Recording is always on — a handful of
// `Instant::now()` reads (vDSO clock_gettime, tens of ns) plus f64 stores
// per tick — because the stats page must show what the loop cost *before* it
// was opened. Recording allocates nothing and touches no filesystem; the
// only allocation anywhere near it is the 60-element median sort the stats
// page performs while it is open.
//
// Wall time and CPU time are separate on purpose: the per-block rows and
// `total` are wall durations — what a tick takes, including off-CPU waits
// (GPU driver round-trips block in-kernel without burning CPU) — while
// `CpuTime` is this process's own utime+stime per tick, the quantity that
// answers "how many % of a core does ktop use" and matches what process
// accounting tools report.

const PROF_HISTORY: usize = 60;

/// One timed region of the refresh tick, in execution order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfBlock {
    Cpu = 0,
    CpuFreq = 1,
    Memory = 2,
    Gpu = 3,
    Temps = 4,
    Net = 5,
    Disks = 6,
    Procs = 7,
    Oom = 8,
    Draw = 9,
    Total = 10,
    /// Process CPU time (utime+stime) for the tick, not wall — header only.
    CpuTime = 11,
}

pub const PROF_BLOCKS: [ProfBlock; 12] = [
    ProfBlock::Cpu,
    ProfBlock::CpuFreq,
    ProfBlock::Memory,
    ProfBlock::Gpu,
    ProfBlock::Temps,
    ProfBlock::Net,
    ProfBlock::Disks,
    ProfBlock::Procs,
    ProfBlock::Oom,
    ProfBlock::Draw,
    ProfBlock::Total,
    ProfBlock::CpuTime,
];

impl ProfBlock {
    pub fn label(self) -> &'static str {
        match self {
            ProfBlock::Cpu => "cpu",
            ProfBlock::CpuFreq => "cpu freq",
            ProfBlock::Memory => "memory",
            ProfBlock::Gpu => "gpu",
            ProfBlock::Temps => "temps",
            ProfBlock::Net => "network",
            ProfBlock::Disks => "disks",
            ProfBlock::Procs => "processes",
            ProfBlock::Oom => "oom poll",
            ProfBlock::Draw => "render",
            ProfBlock::Total => "total",
            ProfBlock::CpuTime => "cpu time",
        }
    }

    fn idx(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct ProfStats {
    pub last_us: f64,
    pub avg_us: f64,
    pub max_us: f64,
}

#[derive(Clone)]
pub struct Profiler {
    /// Microseconds per tick per block; row i is PROF_BLOCKS[i], the slot at
    /// `head` is the tick being recorded right now.
    rings: Vec<Vec<f64>>,
    /// Microseconds per tick per GPU (row i is GPU index i), the same slots
    /// as `rings`. Grown to the card count on the first tick.
    gpu_rings: Vec<Vec<f64>>,
    head: usize,
    next: usize,
    count: usize,
}

impl Profiler {
    fn new() -> Self {
        Self {
            rings: vec![vec![0.0; PROF_HISTORY]; PROF_BLOCKS.len()],
            gpu_rings: Vec::new(),
            head: 0,
            next: 0,
            count: 0,
        }
    }

    /// Start a new tick: advance the ring head and zero its slot.
    fn begin_slot(&mut self) {
        self.head = self.next;
        self.next = (self.next + 1) % PROF_HISTORY;
        if self.count < PROF_HISTORY {
            self.count += 1;
        }
        for ring in self.rings.iter_mut().chain(self.gpu_rings.iter_mut()) {
            ring[self.head] = 0.0;
        }
    }

    fn add(&mut self, block: ProfBlock, us: f64) {
        self.rings[block.idx()][self.head] = us;
    }

    /// Record one tick's sampling time for GPU `idx`, growing the ring set to
    /// the detected card count on first use (a startup-time allocation).
    fn add_gpu(&mut self, idx: usize, us: f64) {
        while self.gpu_rings.len() <= idx {
            self.gpu_rings.push(vec![0.0; PROF_HISTORY]);
        }
        self.gpu_rings[idx][self.head] = us;
    }

    pub fn gpu_count(&self) -> usize {
        self.gpu_rings.len()
    }

    pub fn stats_gpu(&self, idx: usize) -> ProfStats {
        match self.gpu_rings.get(idx) {
            Some(ring) => self.stats_ring(ring),
            None => ProfStats::default(),
        }
    }

    /// Accumulate time measured outside the tick loop (interactive redraws)
    /// into the newest tick's block — every draw belongs to the table.
    fn bump(&mut self, block: ProfBlock, us: f64) {
        self.rings[block.idx()][self.head] += us;
    }

    /// Drop the newest tick's samples. Used for the first tick, which carries
    /// one-time startup work (first full fdinfo walk, journalctl launch,
    /// initial interface enumeration, full-screen first paint) and is not
    /// representative of the steady-state loop cost.
    fn discard_last(&mut self) {
        if self.count == 0 {
            return;
        }
        self.count -= 1;
        self.head = (self.head + PROF_HISTORY - 1) % PROF_HISTORY;
        self.next = (self.next + PROF_HISTORY - 1) % PROF_HISTORY;
    }

    /// Median of the sampled window — the typical tick, immune to the
    /// periodic full-walk spikes that drag the mean up.
    pub fn median(&self, block: ProfBlock) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let mut v: Vec<f64> = self.rings[block.idx()].iter().take(self.count).copied().collect();
        v.sort_by(f64::total_cmp);
        let n = v.len();
        if n % 2 == 1 {
            v[n / 2]
        } else {
            (v[n / 2 - 1] + v[n / 2]) / 2.0
        }
    }

    pub fn samples(&self) -> usize {
        self.count
    }

    pub fn stats(&self, block: ProfBlock) -> ProfStats {
        self.stats_ring(&self.rings[block.idx()])
    }

    fn stats_ring(&self, ring: &[f64]) -> ProfStats {
        if self.count == 0 {
            return ProfStats::default();
        }
        // Before the first wrap only 0..count have been written at all, so
        // those are exactly the valid samples either way.
        let mut sum = 0.0;
        let mut max = 0.0;
        for v in ring.iter().take(self.count) {
            sum += v;
            if *v > max {
                max = *v;
            }
        }
        ProfStats {
            last_us: ring[self.head],
            avg_us: sum / self.count as f64,
            max_us: max,
        }
    }
}

/// This process's own CPU time (utime+stime) in microseconds. Child process
/// time (e.g. the `journalctl` poll) is deliberately not included — process
/// accounting tools charge it to the child, and so do we.
fn proc_cpu_us() -> f64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) } != 0 {
        return 0.0;
    }
    ts.tv_sec as f64 * 1.0e6 + ts.tv_nsec as f64 / 1.0e3
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
    let mut nvidia = NvidiaState::init();
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
        prof: Profiler::new(),
        show_stats: false,
    };

    let refresh_dur = Duration::from_secs_f64(refresh);
    let mut last_refresh = Instant::now() - refresh_dur;
    let mut pending_resize: Option<(Rect, Instant)> = None;
    let mut core_smooth: Vec<f64> = Vec::new();
    let mut first_tick = true;

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
                            let draw_at = Instant::now();
                            let redraw = draw_app(&mut terminal, &state, !no_sync);
                            state
                                .prof
                                .bump(ProfBlock::Draw, draw_at.elapsed().as_secs_f64() * 1e6);
                            redraw?;
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
                let draw_at = Instant::now();
                let redraw = draw_app(&mut terminal, &state, !no_sync);
                state
                    .prof
                    .bump(ProfBlock::Draw, draw_at.elapsed().as_secs_f64() * 1e6);
                redraw?;
                continue;
            }
            if pending_resize.is_some() {
                continue;
            }

            if now.duration_since(last_refresh) >= refresh_dur {
                last_refresh = now;

                // Per-block timing for the `p` stats page: `mark` walks
                // through the tick and each prof_mark! records the wall time
                // of the block that just finished.
                let tick_start = Instant::now();
                let tick_cpu0 = proc_cpu_us();
                let mut mark = tick_start;
                state.prof.begin_slot();
                macro_rules! prof_mark {
                    ($block:expr) => {{
                        let at = Instant::now();
                        state.prof.add($block, mark.elapsed().as_secs_f64() * 1e6);
                        mark = at;
                    }};
                }

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
                prof_mark!(ProfBlock::Cpu);

                // CPU frequency (every 5s)
                if now.duration_since(last_freq_check) >= Duration::from_secs(5) {
                    last_freq_check = now;
                    state.cpu_freq =
                        system::read_cpu_freq().unwrap_or_else(|| state.cpu_freq.clone());
                }
                prof_mark!(ProfBlock::CpuFreq);

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
                prof_mark!(ProfBlock::Memory);

                // GPUs. Each card's sampling is timed so the stats page can
                // attribute cost per GPU. NVML enumerates its devices inside
                // one call, so on a multi-NVIDIA machine that call's time is
                // split evenly across the NVIDIA rows — the per-device NVML
                // calls are not separable here, and threads are not a trade
                // worth making for diagnostics.
                let mut gpu_us: Vec<f64> = vec![0.0; gpu_count];
                let mut gpu_infos = Vec::new();
                if let Some(nv) = &mut nvidia {
                    let at = Instant::now();
                    let infos = nv.sample(0);
                    let us = at.elapsed().as_secs_f64() * 1e6 / nvidia_count.max(1) as f64;
                    for slot in gpu_us[..nvidia_count].iter_mut() {
                        *slot += us;
                    }
                    gpu_infos.extend(infos);
                }
                for (j, card) in amd_cards.iter().enumerate() {
                    let at = Instant::now();
                    let info = card.sample(nvidia_count + j);
                    gpu_us[nvidia_count + j] += at.elapsed().as_secs_f64() * 1e6;
                    gpu_infos.push(info);
                }
                for (j, card) in intel_cards.iter_mut().enumerate() {
                    let at = Instant::now();
                    let info = card.sample(intel_base + j);
                    gpu_us[intel_base + j] += at.elapsed().as_secs_f64() * 1e6;
                    gpu_infos.push(info);
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
                if let Some(nv) = &mut nvidia {
                    let at = Instant::now();
                    let temps = nv.temps();
                    let us = at.elapsed().as_secs_f64() * 1e6 / nvidia_count.max(1) as f64;
                    for slot in gpu_us[..nvidia_count].iter_mut() {
                        *slot += us;
                    }
                    gpu_temps.extend(temps);
                }
                for (j, card) in amd_cards.iter().enumerate() {
                    let at = Instant::now();
                    let temp = card.temp();
                    gpu_us[nvidia_count + j] += at.elapsed().as_secs_f64() * 1e6;
                    gpu_temps.push(temp);
                }
                for (j, card) in intel_cards.iter().enumerate() {
                    let at = Instant::now();
                    let temp = card.temp();
                    gpu_us[intel_base + j] += at.elapsed().as_secs_f64() * 1e6;
                    gpu_temps.push(temp);
                }
                state.gpu_temps = gpu_temps;

                // Per-GPU live power (NVML mW / AMD power1_average / Intel
                // energy-delta), sampled once per tick and reused by the
                // footer estimate. Order matches gpu_infos: NVIDIA, AMD, Intel.
                let cpu_power = power_estimator.sample_cpu_watts();
                state.cpu_power_watts = cpu_power;
                state.cpu_power_denied = power_estimator.access_denied;
                let mut gpu_power: Vec<Option<f64>> = Vec::new();
                if let Some(nv) = &mut nvidia {
                    let at = Instant::now();
                    let watts = nv.power_watts();
                    let us = at.elapsed().as_secs_f64() * 1e6 / nvidia_count.max(1) as f64;
                    for slot in gpu_us[..nvidia_count].iter_mut() {
                        *slot += us;
                    }
                    gpu_power.extend(watts);
                }
                for (j, card) in amd_cards.iter().enumerate() {
                    let at = Instant::now();
                    let watt = card.power_watts();
                    gpu_us[nvidia_count + j] += at.elapsed().as_secs_f64() * 1e6;
                    gpu_power.push(watt);
                }
                for (j, card) in intel_cards.iter_mut().enumerate() {
                    let at = Instant::now();
                    let watt = card.power_watts();
                    gpu_us[intel_base + j] += at.elapsed().as_secs_f64() * 1e6;
                    gpu_power.push(watt);
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
                for (i, us) in gpu_us.iter().enumerate() {
                    state.prof.add_gpu(i, *us);
                }
                prof_mark!(ProfBlock::Gpu);

                // CPU/mem temps via the hwmon table
                sample_temps(&mut state);
                prof_mark!(ProfBlock::Temps);

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
                prof_mark!(ProfBlock::Net);

                // Disks: live per-device read/write throughput.
                state.disks = disk_tracker.sample();
                prof_mark!(ProfBlock::Disks);

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
                prof_mark!(ProfBlock::Procs);

                // OOM
                oom_tracker.check();
                state.oom_str = oom_tracker.last_oom.clone();
                prof_mark!(ProfBlock::Oom);

                draw_app(&mut terminal, &state, !no_sync)?;
                // The chain ends here: draw is timed to this clock read,
                // total covers the whole tick including the profiler's own.
                let draw_us = mark.elapsed().as_secs_f64() * 1e6;
                let total_us = tick_start.elapsed().as_secs_f64() * 1e6;
                state.prof.add(ProfBlock::Draw, draw_us);
                state.prof.add(ProfBlock::Total, total_us);
                state
                    .prof
                    .add(ProfBlock::CpuTime, proc_cpu_us() - tick_cpu0);
                // The first tick carries one-time startup work and would
                // dominate the window's average for a whole minute.
                if first_tick {
                    state.prof.discard_last();
                    first_tick = false;
                }
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

    if state.show_stats {
        // The stats page owns the keyboard while open; s/Esc closes it.
        match key.code {
            KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Esc => state.show_stats = false,
            KeyCode::Char('q') | KeyCode::Char('Q') => return (true, false),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return (true, false)
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
        KeyCode::Char('s') | KeyCode::Char('S') => {
            state.show_stats = true;
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
