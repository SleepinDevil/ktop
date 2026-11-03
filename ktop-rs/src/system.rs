use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

// ── Process info (read from /proc directly for efficiency) ──

#[derive(Clone, Debug)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub rss: u64,
    pub shared: u64,
}

/// How many rows each Top Processes lists hold (sort-then-truncate cap).
const MAX_PROCS: usize = 20;

/// Read buffer for /proc/<pid>/stat. The kernel's stat line is bounded
/// well below this (~500 bytes with a 128-CPU affinity mask and a 15-byte
/// comm), so one pass of File::read is complete; a truncated parse simply
/// fails like the original read error did.
const STAT_BUF: usize = 1024;

/// One process as the sampling pass sees it: everything needed to rank,
/// without any heap allocation. Name and shared memory are only fetched
/// for rows that survive truncation into the displayed tables.
#[derive(Clone, Copy)]
struct RawProc {
    pid: u32,
    rss: u64,
    cpu_pct: f64,
    mem_pct: f64,
}

pub struct ProcScanner {
    cpu_prev: HashMap<u32, u64>,
    cpu_smoothed: HashMap<u32, f64>,
    last_scan: Instant,
    page_size: u64,
    clock_ticks: u64,
    num_cpus: u64,
    pub by_mem: Vec<ProcInfo>,
    pub by_cpu: Vec<ProcInfo>,
}

impl ProcScanner {
    pub fn new() -> Self {
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGE_SIZE) as u64 };
        let clock_ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) as u64 };
        let num_cpus = num_cpus_count();

        let mut scanner = Self {
            cpu_prev: HashMap::new(),
            cpu_smoothed: HashMap::new(),
            last_scan: Instant::now(),
            page_size,
            clock_ticks,
            num_cpus,
            by_mem: Vec::new(),
            by_cpu: Vec::new(),
        };

        // Seed CPU baselines
        scanner.seed_baselines();
        scanner
    }

    fn seed_baselines(&mut self) {
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Ok(pid) = name_str.parse::<u32>() {
                    if let Some(cpu_total) = read_proc_cpu_total(pid) {
                        self.cpu_prev.insert(pid, cpu_total);
                    }
                }
            }
        }
    }

    /// Rebuilds the top-process tables. Returns whether this call actually
    /// rebuilt (calls inside the `min_wait_secs` window are no-ops), so the
    /// caller only hands freshly built tables to the display state.
    pub fn scan(&mut self, total_mem: u64, min_wait_secs: f64) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_scan).as_secs_f64();
        let min_wait = if self.by_mem.is_empty() {
            1.0
        } else {
            min_wait_secs.max(1.0)
        };
        if elapsed < min_wait {
            return false;
        }
        let dt = elapsed;
        self.last_scan = now;

        let ps = self.page_size;
        let ct = self.clock_ticks as f64;

        // Compact pass: every live pid yields one RawProc — no per-process
        // path formatting, String allocation, or field-vector allocation.
        // The expensive display fields (name, shared) are only materialized
        // for the ≤2×MAX_PROCS rows actually rendered.
        let mut raw: Vec<RawProc> = Vec::with_capacity(256);
        let mut buf = [0u8; STAT_BUF];
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                let pid = match name_str.parse::<u32>() {
                    Ok(p) if p > 0 => p,
                    _ => continue,
                };
                let stat = match read_proc_stat(pid, &mut buf) {
                    Some(s) => s,
                    None => continue,
                };
                let (utime, stime, rss_pages) = match parse_stat_usage(stat) {
                    Some(v) => v,
                    None => continue,
                };

                let cpu_total = utime + stime;
                let prev = self.cpu_prev.get(&pid).copied().unwrap_or(cpu_total);
                let cpu_delta = cpu_total.saturating_sub(prev);
                self.cpu_prev.insert(pid, cpu_total);

                let raw_cpu_pct = if dt > 0.0 {
                    (cpu_delta as f64 / ct) / dt * 100.0
                } else {
                    0.0
                };
                let previous_smoothed = self.cpu_smoothed.get(&pid).copied().unwrap_or(raw_cpu_pct);
                let cpu_pct = previous_smoothed * 0.65 + raw_cpu_pct * 0.35;
                self.cpu_smoothed.insert(pid, cpu_pct);

                let mem_pct = if total_mem > 0 {
                    (rss_pages * ps) as f64 / total_mem as f64 * 100.0
                } else {
                    0.0
                };
                raw.push(RawProc {
                    pid,
                    rss: rss_pages * ps,
                    cpu_pct,
                    mem_pct,
                });
            }
        }

        // Sort and take the top `MAX_PROCS` by each metric. One vector,
        // two in-place sorts: the comparators are total orders (pid breaks
        // every tie) and stable, so results are identical to the previous
        // clone-sort-truncate. The top-20 copies are taken before the
        // second sort reorders the vector.
        raw.sort_by(|a, b| {
            b.cpu_pct
                .partial_cmp(&a.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.mem_pct
                        .partial_cmp(&a.mem_pct)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.pid.cmp(&b.pid))
        });
        let cpu_top: Vec<RawProc> = raw.iter().copied().take(MAX_PROCS).collect();
        raw.sort_by(|a, b| {
            b.mem_pct
                .partial_cmp(&a.mem_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.pid.cmp(&b.pid))
        });
        let mem_top: Vec<RawProc> = raw.iter().copied().take(MAX_PROCS).collect();

        // Read name and shared memory for displayed procs (the union of
        // the two lists; every entry of a list is displayed)
        let mut details: HashMap<u32, (String, u64)> = HashMap::new();
        for r in cpu_top.iter().chain(mem_top.iter()) {
            details
                .entry(r.pid)
                .or_insert_with(|| read_proc_details(r.pid, ps));
        }
        let take = |r: &RawProc| {
            let (name, shared) = details
                .get(&r.pid)
                .map(|(n, s)| (n.clone(), *s))
                .unwrap_or_default();
            ProcInfo {
                pid: r.pid,
                name,
                cpu_percent: r.cpu_pct,
                memory_percent: r.mem_pct,
                rss: r.rss,
                shared,
            }
        };
        let by_cpu: Vec<ProcInfo> = cpu_top.iter().map(take).collect();
        let by_mem: Vec<ProcInfo> = mem_top.iter().map(take).collect();

        // Clean stale PIDs
        let current: std::collections::HashSet<u32> = raw.iter().map(|p| p.pid).collect();
        self.cpu_prev.retain(|k, _| current.contains(k));
        self.cpu_smoothed.retain(|k, _| current.contains(k));

        self.by_mem = by_mem;
        self.by_cpu = by_cpu;
        true
    }

    pub fn num_cpus(&self) -> u64 {
        self.num_cpus
    }
}

fn read_file_bytes(path: &str) -> Option<Vec<u8>> {
    let mut f = fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(512);
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn parse_u64(bytes: &[u8]) -> u64 {
    let s = std::str::from_utf8(bytes).unwrap_or("0");
    s.parse().unwrap_or(0)
}

/// Read /proc/<pid>/stat into a caller-owned buffer, returning the bytes
/// actually read (no allocation on the per-process path).
fn read_proc_stat<'a>(pid: u32, buf: &'a mut [u8; STAT_BUF]) -> Option<&'a [u8]> {
    // "/proc/" + up to 10 decimal digits + "/stat"
    let mut path = [0u8; 6 + 10 + 5];
    path[..6].copy_from_slice(b"/proc/");
    let mut n = 6;
    let mut digits = [0u8; 10];
    let mut d = 0;
    let mut p = pid;
    loop {
        digits[d] = b'0' + (p % 10) as u8;
        d += 1;
        p /= 10;
        if p == 0 {
            break;
        }
    }
    for i in (0..d).rev() {
        path[n] = digits[i];
        n += 1;
    }
    path[n..n + 5].copy_from_slice(b"/stat");
    n += 5;
    let mut f = fs::File::open(std::str::from_utf8(&path[..n]).ok()?).ok()?;
    let mut read = 0;
    while read < buf.len() {
        match f.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(k) => read += k,
            Err(_) => return None,
        }
    }
    Some(&buf[..read])
}

/// Extract (utime, stime, rss_pages) from raw /proc/<pid>/stat bytes —
/// the three fields the sampler uses, located after the last ')' so a
/// comm containing spaces or parentheses cannot shift the fields (same
/// indexing as the previous splitn-based parser, including the ≥22-field
/// requirement).
fn parse_stat_usage(stat: &[u8]) -> Option<(u64, u64, u64)> {
    let rparen = stat.iter().rposition(|&b| b == b')')?;
    let fields = &stat[rparen + 2..];
    let mut out = [0u64; 3];
    let mut complete = false;
    for (i, t) in fields.split(|&b| b == b' ').enumerate() {
        match i {
            11 => out[0] = parse_u64(t),
            12 => out[1] = parse_u64(t),
            21 => {
                out[2] = parse_u64(t);
                complete = true;
                break;
            }
            _ => {}
        }
    }
    complete.then(|| (out[0], out[1], out[2]))
}

/// The two expensive display fields for one process: comm-based name and
/// shared memory (RSS-shared from statm). Only called for displayed rows.
fn read_proc_details(pid: u32, page_size: u64) -> (String, u64) {
    let mut buf = [0u8; STAT_BUF];
    let name = match read_proc_stat(pid, &mut buf) {
        Some(stat) => {
            let rparen = stat.iter().rposition(|&b| b == b')');
            match rparen {
                Some(rp) => {
                    let lparen = stat.iter().position(|&b| b == b'(').unwrap_or(0);
                    String::from_utf8_lossy(&stat[lparen + 1..rp])
                        .chars()
                        .take(28)
                        .collect::<String>()
                }
                None => String::new(),
            }
        }
        None => String::new(),
    };
    let shared = read_proc_shared(pid, page_size);
    (name, shared)
}

fn read_proc_cpu_total(pid: u32) -> Option<u64> {
    let mut buf = [0u8; STAT_BUF];
    let stat = read_proc_stat(pid, &mut buf)?;
    let (utime, stime, _) = parse_stat_usage(stat)?;
    Some(utime + stime)
}

fn read_proc_shared(pid: u32, page_size: u64) -> u64 {
    let path = format!("/proc/{}/statm", pid);
    if let Some(data) = read_file_bytes(&path) {
        let fields: Vec<&[u8]> = data.splitn(4, |&b| b == b' ').collect();
        if fields.len() >= 3 {
            return parse_u64(fields[2]) * page_size;
        }
    }
    0
}

fn num_cpus_count() -> u64 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u64)
        .unwrap_or(1)
}

// ── OOM kill detection ──

pub struct OomTracker {
    last_check: Instant,
    pub last_oom: Option<String>,
    sim: bool,
}

static SIM_PROCS: &[&str] = &[
    "python3", "node", "java", "ollama", "vllm", "ffmpeg", "cc1plus", "rustc", "chrome", "mysqld",
];

// Regex-like UUID pattern for stripping from oomd kill names
fn strip_uuids(s: &str) -> String {
    // Match -XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX patterns
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '-' && i + 36 <= chars.len() {
            // Check if this is a UUID pattern: -8-4-4-4-12 hex digits
            let candidate: String = chars[i..i + 37.min(chars.len())].iter().collect();
            if candidate.len() >= 37 {
                let parts: Vec<&str> = candidate[1..].splitn(6, '-').collect();
                if parts.len() == 5
                    && parts[0].len() == 8
                    && parts[1].len() == 4
                    && parts[2].len() == 4
                    && parts[3].len() == 4
                    && parts[4].len() == 12
                    && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
                {
                    i += 37;
                    continue;
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

// OOM history comes from journalctl, so each poll forks a subprocess and
// may read the whole journal window. Two changes make this cheap:
// poll once a minute (the status bar reports kills up to 8 h old, so a
// slower poll is invisible), and ask journalctl to seek backwards and
// stop at the newest match (`-r -n 1`) instead of piping every match of
// the window through a pipe and keeping the last line.
const OOM_CHECK_SECS: f64 = 60.0;

impl OomTracker {
    pub fn new(sim: bool) -> Self {
        Self {
            last_check: Instant::now() - std::time::Duration::from_secs(10),
            last_oom: None,
            sim,
        }
    }

    pub fn check(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_check).as_secs_f64() < OOM_CHECK_SECS {
            return;
        }
        self.last_check = now;

        if self.sim {
            use std::time::{SystemTime, UNIX_EPOCH};
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            if seed % 2 == 0 {
                self.last_oom = None;
            } else {
                let proc_idx = (seed / 3) as usize % SIM_PROCS.len();
                let fake_offset = (seed % (8 * 3600)) as u64;
                let fake_epoch = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    - fake_offset;
                let dt = chrono_format_full(fake_epoch as f64);
                self.last_oom = Some(format!("{} {}", dt, SIM_PROCS[proc_idx]));
            }
            return;
        }

        let mut candidates: Vec<(f64, String)> = Vec::new();

        // Kernel OOM kills — reverse order with a one-line limit: the
        // newest match in the window is the first line, and a window
        // without matches stays a pure backwards seek. The exit status
        // is failure when nothing matches, which also skips parsing.
        if let Ok(output) = Command::new("journalctl")
            .args([
                "-k",
                "--since",
                "8 hours ago",
                "--no-pager",
                "-o",
                "short-unix",
                "-r",
                "-n",
                "1",
                "--grep",
                "Killed process",
            ])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(line) = stdout.trim().lines().last() {
                    if let Some((epoch, proc_name)) = parse_oom_line(line) {
                        candidates.push((epoch, proc_name));
                    }
                }
            }
        }

        // systemd-oomd kills
        if let Ok(output) = Command::new("journalctl")
            .args([
                "-u",
                "systemd-oomd",
                "--since",
                "8 hours ago",
                "--no-pager",
                "-o",
                "short-unix",
                "-r",
                "-n",
                "1",
                "--grep",
                "Killed",
            ])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(line) = stdout.trim().lines().last() {
                    if let Some((epoch, proc_name)) = parse_oomd_line(line) {
                        candidates.push((epoch, proc_name));
                    }
                }
            }
        }

        if candidates.is_empty() {
            self.last_oom = None;
        } else {
            // Pick the most recent by timestamp
            candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            let (epoch, proc_name) = &candidates[0];
            let dt = chrono_format_full(*epoch);
            self.last_oom = Some(format!("{} {}", dt, proc_name));
        }
    }
}

fn parse_oom_line(line: &str) -> Option<(f64, String)> {
    // Format: "1234567890.123 hostname kernel: Killed process 1234 (name)"
    let proc_start = line.find("Killed process ")?;
    let rest = &line[proc_start..];
    let lparen = rest.find('(')?;
    let rparen = rest.find(')')?;
    if rparen > lparen {
        let ts = line.split_whitespace().next()?;
        let epoch: f64 = ts.parse().ok()?;
        let name = rest[lparen + 1..rparen].to_string();
        Some((epoch, name))
    } else {
        None
    }
}

fn parse_oomd_line(line: &str) -> Option<(f64, String)> {
    let ts = line.split_whitespace().next()?;
    let epoch: f64 = ts.parse().ok()?;
    // "Killed /some/path/unit.scope due to memory"
    if let Some(start) = line.find("Killed ") {
        let rest = &line[start + 7..];
        if let Some(due) = rest.find(" due to") {
            let path = &rest[..due];
            let name = path.rsplit('/').next().unwrap_or("oomd-kill");
            let name = name
                .replace(".scope", "")
                .replace(".service", "");
            let name = strip_uuids(&name);
            return Some((epoch, name));
        }
    }
    Some((epoch, "oomd-kill".to_string()))
}

fn chrono_format_full(epoch: f64) -> String {
    // Convert epoch to local time with month, day, and time
    // Using libc localtime for proper timezone handling
    let secs = epoch as i64;
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    };
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                   "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = months.get(tm.tm_mon as usize).unwrap_or(&"???");
    format!("{} {:02} {:02}:{:02}:{:02}",
            month, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

// ── /proc/meminfo: one read and parse per tick ──

/// Everything the Memory panel shows that comes from /proc/meminfo, from a
/// single read+parse. Values mirror what the previous sysinfo-based
/// sampling produced bit-for-bit: "kB" fields are KiB and scaled by 1024,
/// a missing `MemAvailable` (kernel < 3.14) falls back to the pre-3.14
/// estimate `MemFree + Buffers + Cached + SReclaimable − Shmem`, and
/// `committed` is `None` unless both commit keys parse (same hiding
/// semantics as the old standalone `read_committed_limit`).
#[derive(Clone, Copy)]
pub struct MemSample {
    pub total: u64,
    pub available: u64,
    pub cached: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    pub committed: Option<(u64, u64)>,
}

/// `None` when /proc/meminfo cannot be read at all; callers keep the
/// previous values then, exactly as sysinfo kept its stale fields.
pub fn read_mem_sample() -> Option<MemSample> {
    Some(parse_meminfo(&fs::read_to_string("/proc/meminfo").ok()?))
}

fn parse_meminfo(contents: &str) -> MemSample {
    let mut total = 0u64;
    let mut free = 0u64;
    let mut available = 0u64;
    let mut available_found = false;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    let mut shmem = 0u64;
    let mut slab_reclaimable = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;
    let mut committed_as = None;
    let mut commit_limit = None;
    for line in contents.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let kb = rest
            .trim()
            .trim_end_matches(" kB")
            .trim()
            .parse::<u64>()
            .ok();
        match key {
            "MemTotal" => total = kb.unwrap_or(0),
            "MemFree" => free = kb.unwrap_or(0),
            "MemAvailable" => {
                if let Some(v) = kb {
                    available_found = true;
                    available = v;
                }
            }
            "Buffers" => buffers = kb.unwrap_or(0),
            "Cached" => cached = kb.unwrap_or(0),
            "Shmem" => shmem = kb.unwrap_or(0),
            "SReclaimable" => slab_reclaimable = kb.unwrap_or(0),
            "SwapTotal" => swap_total = kb.unwrap_or(0),
            "SwapFree" => swap_free = kb.unwrap_or(0),
            "Committed_AS" => committed_as = kb,
            "CommitLimit" => commit_limit = kb,
            _ => {}
        }
    }
    if !available_found {
        available = free
            .saturating_add(buffers)
            .saturating_add(cached)
            .saturating_add(slab_reclaimable)
            .saturating_sub(shmem);
    }
    MemSample {
        total: total.saturating_mul(1024),
        available: available.saturating_mul(1024),
        cached: cached.saturating_mul(1024),
        swap_total: swap_total.saturating_mul(1024),
        swap_free: swap_free.saturating_mul(1024),
        committed: match (committed_as, commit_limit) {
            (Some(c), Some(l)) => Some((c.saturating_mul(1024), l.saturating_mul(1024))),
            _ => None,
        },
    }
}

#[cfg(test)]
mod meminfo_tests {
    use super::*;

    #[test]
    fn parses_committed_and_limit_kib_as_bytes() {
        let content = "MemTotal:   65000000 kB\nCommitLimit:  40209796 kB\nCommitted_AS:  91134676 kB\n";
        assert_eq!(
            parse_meminfo(content).committed,
            Some((91134676 * 1024, 40209796 * 1024))
        );
    }

    #[test]
    fn missing_or_malformed_commit_keys_are_unavailable() {
        assert!(parse_meminfo("MemTotal: 1 kB\n").committed.is_none());
        assert!(parse_meminfo("CommitLimit: 1 kB\n").committed.is_none());
        assert!(parse_meminfo("Committed_AS: x kB\nCommitLimit: 1 kB\n")
            .committed
            .is_none());
    }

    #[test]
    fn parses_panel_fields_from_kib_to_bytes() {
        let content = concat!(
            "MemTotal:       65000000 kB\n",
            "MemFree:          500000 kB\n",
            "MemAvailable:   12000000 kB\n",
            "Buffers:         1000000 kB\n",
            "Cached:         19000000 kB\n",
            "SwapTotal:       8388604 kB\n",
            "SwapFree:        3000000 kB\n",
        );
        let s = parse_meminfo(content);
        assert_eq!(s.total, 65000000 * 1024);
        assert_eq!(s.available, 12000000 * 1024);
        assert_eq!(s.cached, 19000000 * 1024);
        assert_eq!(s.swap_total, 8388604 * 1024);
        assert_eq!(s.swap_free, 3000000 * 1024);
    }

    #[test]
    fn absent_memavailable_uses_the_pre_314_estimate() {
        // same formula sysinfo falls back to: free+buffers+cached+slab−shmem
        let content = concat!(
            "MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\n",
            "Cached: 200 kB\nShmem: 30 kB\nSReclaimable: 25 kB\n",
        );
        assert_eq!(parse_meminfo(content).available, (100 + 50 + 200 + 25 - 30) * 1024);
        // saturating: huge Shmem cannot underflow to a huge available
        let content = "MemFree: 10 kB\nCached: 5 kB\nShmem: 999999 kB\n";
        assert_eq!(parse_meminfo(content).available, 0);
    }
}

// ── CPU frequency from sysfs ──

pub fn read_cpu_freq() -> Option<String> {
    if let Ok(freq_str) = fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")
    {
        if let Ok(khz) = freq_str.trim().parse::<u64>() {
            return Some(format!("{} MHz", khz / 1000));
        }
    }
    None
}

pub fn read_cpu_name() -> String {
    let raw = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| "CPU".to_string());
    // Drop core-count/processor suffixes: "AMD Ryzen 9 5950X 16-Core
    // Processor" -> "AMD Ryzen 9 5950X".
    let mut name = raw.clone();
    if let Some(pos) = name.find("-Core Processor") {
        let head = name[..pos].trim_end();
        if let Some(sp) = head.rfind(' ') {
            let tok = &head[sp + 1..];
            if !tok.is_empty() && tok.chars().all(|c| c.is_ascii_digit()) {
                name = head[..sp].to_string();
            }
        }
    }
    name = name.trim_end_matches(" Processor").trim().to_string();
    if name.is_empty() {
        name = raw;
    }
    name
}

// ── Power estimation from Linux sensors ──

struct RaplZone {
    energy_uj_path: PathBuf,
    max_energy_uj: u64,
    last_energy_uj: Option<u64>,
}

pub struct PowerEstimator {
    rapl_zones: Vec<RaplZone>,
    hwmon_cpu_power_paths: Vec<PathBuf>,
    last_sample: Instant,
    pub cpu_watts: Option<f64>,
    pub access_denied: bool,
}

impl PowerEstimator {
    pub fn new() -> Self {
        // Resolved once at startup from the shared hwmon table; the
        // per-tick sample only reads the resulting files.
        let hwmon_cpu_power_paths = hwmon_table()
            .iter()
            .filter(|dev| {
                matches!(
                    dev.name.to_ascii_lowercase().as_str(),
                    "zenpower" | "k10temp" | "coretemp"
                )
            })
            .filter_map(|dev| dev.power_path.clone())
            .collect();
        Self {
            rapl_zones: detect_rapl_zones(),
            hwmon_cpu_power_paths,
            last_sample: Instant::now(),
            cpu_watts: None,
            access_denied: false,
        }
    }

    pub fn sample_cpu_watts(&mut self) -> Option<f64> {
        let now = Instant::now();
        let dt = now.duration_since(self.last_sample).as_secs_f64();
        self.last_sample = now;

        let rapl = if dt > 0.0 {
            sample_rapl_cpu_watts(&mut self.rapl_zones, dt)
        } else {
            (None, false)
        };
        let hwmon = sample_hwmon_cpu_watts(&self.hwmon_cpu_power_paths);

        self.cpu_watts = rapl.0.or(hwmon);
        // Zones exist but the energy counters are root-only (PLATYPUS
        // lockdown): the data is there, the process just may not read it
        self.access_denied = self.cpu_watts.is_none() && rapl.1;
        self.cpu_watts
    }
}

fn detect_rapl_zones() -> Vec<RaplZone> {
    let mut zones = Vec::new();
    let base = PathBuf::from("/sys/class/powercap");
    let entries = match fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(_) => return zones,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("intel-rapl:") {
            continue;
        }
        if name.matches(':').count() != 1 {
            continue;
        }

        let zone_name = fs::read_to_string(path.join("name"))
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if zone_name.contains("psys") {
            continue;
        }

        let energy_uj_path = path.join("energy_uj");
        let max_energy_path = path.join("max_energy_range_uj");
        if !energy_uj_path.is_file() || !max_energy_path.is_file() {
            continue;
        }
        let max_energy_uj = fs::read_to_string(max_energy_path)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if max_energy_uj == 0 {
            continue;
        }

        zones.push(RaplZone {
            energy_uj_path,
            max_energy_uj,
            last_energy_uj: None,
        });
    }

    zones
}

fn sample_rapl_cpu_watts(zones: &mut [RaplZone], dt: f64) -> (Option<f64>, bool) {
    let mut total_watts = 0.0;
    let mut found = false;
    let mut denied = false;

    for zone in zones {
        let energy_uj = match fs::read_to_string(&zone.energy_uj_path) {
            Ok(v) => match v.trim().parse::<u64>() {
                Ok(value) => value,
                Err(_) => continue,
            },
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    denied = true;
                }
                continue;
            }
        };

        if let Some(prev) = zone.last_energy_uj {
            let delta_uj = if energy_uj >= prev {
                energy_uj - prev
            } else {
                (zone.max_energy_uj.saturating_sub(prev)) + energy_uj
            };
            total_watts += delta_uj as f64 / 1_000_000.0 / dt;
            found = true;
        }

        zone.last_energy_uj = Some(energy_uj);
    }

    (found.then_some(total_watts), denied)
}

// ── hwmon path table (discovered once, re-read every 60 s) ──
//
// Consumers (CPU/mem temperature sampling, CPU power estimation) used to
// re-enumerate /sys/class/hwmon and probe dozens of nonexistent
// temp{N}_{input,crit,max} files on every refresh tick; a single sysfs
// attribute read costs ~150 µs, so the probe storm dominated the tick.
// The table resolves the sensor files per hwmon device once and the
// consumers read only files that exist. The 60 s refresh still picks up
// sensors whose driver module loads while ktop is running.

const HWMON_REFRESH_SECS: f64 = 60.0;

#[derive(Default)]
pub struct HwmonDevice {
    /// Contents of `name`, verbatim (some drivers use mixed case).
    pub name: String,
    /// temp{N}_input paths, ascending N.
    pub temp_inputs: Vec<PathBuf>,
    /// (temp{N}_crit, temp{N}_max) paths per N over 1..=16; the crit slot
    /// when the crit file exists, else the max slot when the max file
    /// exists (mirrors the consumers' crit-else-max precedence).
    pub temp_limits: Vec<(Option<PathBuf>, Option<PathBuf>)>,
    /// power1_average, or power1_input when no average exists.
    pub power_path: Option<PathBuf>,
}

type HwmonTable = Vec<HwmonDevice>;
type HwmonArc = std::sync::Arc<HwmonTable>;

fn discover_hwmon_table() -> HwmonTable {
    let mut devices = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let dir = entry.path();
            let name_path = dir.join("name");
            let name = fs::read_to_string(&name_path)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let mut dev = HwmonDevice {
                name,
                ..Default::default()
            };
            if name_path.is_file() {
                for i in 1..=16 {
                    let input = dir.join(format!("temp{}_input", i));
                    if input.is_file() {
                        dev.temp_inputs.push(input);
                    }
                    let crit = dir.join(format!("temp{}_crit", i));
                    if crit.is_file() {
                        dev.temp_limits.push((Some(crit), None));
                    } else {
                        let max = dir.join(format!("temp{}_max", i));
                        let max = max.is_file().then_some(max);
                        dev.temp_limits.push((None, max));
                    }
                }
                let avg = dir.join("power1_average");
                if avg.is_file() {
                    dev.power_path = Some(avg);
                } else {
                    let input = dir.join("power1_input");
                    if input.is_file() {
                        dev.power_path = Some(input);
                    }
                }
            }
            devices.push(dev);
        }
    }
    devices
}

pub fn hwmon_table() -> HwmonArc {
    static CACHE: std::sync::LazyLock<std::sync::Mutex<Option<(HwmonArc, Instant)>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(None));
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let stale = match guard.as_ref() {
        Some((_, at)) => at.elapsed().as_secs_f64() >= HWMON_REFRESH_SECS,
        None => true,
    };
    if stale {
        *guard = Some((std::sync::Arc::new(discover_hwmon_table()), Instant::now()));
    }
    guard.as_ref().unwrap().0.clone()
}

fn sample_hwmon_cpu_watts(paths: &[PathBuf]) -> Option<f64> {
    let mut total = 0.0;
    let mut found = false;
    for path in paths {
        let value = match fs::read_to_string(path)
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
        {
            Some(value) => value,
            None => continue,
        };
        total += value / 1_000_000.0;
        found = true;
    }
    found.then_some(total)
}

// ── Network speeds ──

pub struct NetTracker {
    last_sent: u64,
    last_recv: u64,
    last_time: Instant,
    pub max_speed: f64,
    pub max_up: f64,
    pub max_down: f64,
}

impl NetTracker {
    pub fn new() -> Self {
        let networks = sysinfo::Networks::new_with_refreshed_list();
        let (sent, recv) = net_totals(&networks);
        Self {
            last_sent: sent,
            last_recv: recv,
            last_time: Instant::now(),
            max_speed: 1.0,
            max_up: 1.0,
            max_down: 1.0,
        }
    }

    pub fn sample(&mut self, networks: &sysinfo::Networks) -> (f64, f64) {
        let (sent, recv) = net_totals(networks);
        let now = Instant::now();
        let dt = now.duration_since(self.last_time).as_secs_f64();
        let dt = if dt <= 0.0 { 1.0 } else { dt };

        let up = (sent.saturating_sub(self.last_sent)) as f64 / dt;
        let down = (recv.saturating_sub(self.last_recv)) as f64 / dt;

        self.last_sent = sent;
        self.last_recv = recv;
        self.last_time = now;
        self.max_speed = self.max_speed.max(up).max(down).max(1.0);
        self.max_up = self.max_up.max(up).max(1.0);
        self.max_down = self.max_down.max(down).max(1.0);

        (up, down)
    }
}

// ── Swap I/O throughput from /proc/vmstat ──

pub struct SwapIoTracker {
    last_in_pages: Option<u64>,
    last_out_pages: Option<u64>,
    last_time: Instant,
    page_size: u64,
}

impl SwapIoTracker {
    pub fn new() -> Self {
        let page_size = match unsafe { libc::sysconf(libc::_SC_PAGESIZE) } {
            n if n > 0 => n as u64,
            _ => 4096,
        };
        let (in_pages, out_pages) = read_swap_io_pages();
        Self {
            last_in_pages: in_pages,
            last_out_pages: out_pages,
            last_time: Instant::now(),
            page_size,
        }
    }

    /// Bytes/sec read into RAM from swap (pswpin) and written out (pswpout).
    /// (0.0, 0.0) on the first full sample and whenever the counters are
    /// unavailable (e.g. zswap-only setups that do not bump pswpin/pswpout).
    pub fn sample(&mut self) -> (f64, f64) {
        let now = Instant::now();
        let (in_pages, out_pages) = match (read_swap_io_pages(), self.last_in_pages) {
            ((Some(i), Some(o)), Some(_)) => (i, o),
            ((Some(i), Some(o)), None) => {
                // Counters appeared after startup; seed and report zero.
                self.last_in_pages = Some(i);
                self.last_out_pages = Some(o);
                self.last_time = now;
                return (0.0, 0.0);
            }
            _ => return (0.0, 0.0),
        };
        let last_in = self.last_in_pages.unwrap_or(in_pages);
        let last_out = self.last_out_pages.unwrap_or(out_pages);
        let dt = now.duration_since(self.last_time).as_secs_f64();
        let dt = if dt <= 0.0 { 1.0 } else { dt };

        let r = (in_pages.saturating_sub(last_in)) as f64 * self.page_size as f64 / dt;
        let w = (out_pages.saturating_sub(last_out)) as f64 * self.page_size as f64 / dt;

        self.last_in_pages = Some(in_pages);
        self.last_out_pages = Some(out_pages);
        self.last_time = now;
        (r, w)
    }
}

fn read_swap_io_pages() -> (Option<u64>, Option<u64>) {
    let mut in_pages = None;
    let mut out_pages = None;
    if let Ok(contents) = fs::read_to_string("/proc/vmstat") {
        for line in contents.lines() {
            if let Some(rest) = line.strip_prefix("pswpin ") {
                in_pages = rest.trim().parse::<u64>().ok();
            } else if let Some(rest) = line.strip_prefix("pswpout ") {
                out_pages = rest.trim().parse::<u64>().ok();
            }
            if in_pages.is_some() && out_pages.is_some() {
                break;
            }
        }
    }
    (in_pages, out_pages)
}

#[cfg(test)]
mod swap_io_tests {
    use super::*;

    #[test]
    fn reads_pswpin_pswpout_from_vmstat() {
        // /proc/vmstat always carries both counters on Linux; a parse failure
        // means the key/prefix format changed.
        let (pin, pout) = read_swap_io_pages();
        assert!(pin.is_some(), "pswpin missing from /proc/vmstat");
        assert!(pout.is_some(), "pswpout missing from /proc/vmstat");
    }
}

// ── Memory pressure (PSI) from /proc/pressure/memory ──

/// PSI `some` stall percentages (avg10, avg60, avg300) for memory; `None`
/// when PSI is unavailable (kernel < 4.20, pressure disabled at boot, or
/// the file unreadable), in which case the panel hides the row.
pub fn read_mem_psi_some() -> Option<(f64, f64, f64)> {
    parse_mem_psi_some(&fs::read_to_string("/proc/pressure/memory").ok()?)
}

fn parse_mem_psi_some(contents: &str) -> Option<(f64, f64, f64)> {
    let line = contents.lines().find(|l| l.starts_with("some "))?;
    let mut vals = [0.0f64; 3];
    for (i, key) in ["avg10=", "avg60=", "avg300="].iter().enumerate() {
        let tok = line.split(' ').find(|t| t.starts_with(key))?;
        vals[i] = tok[key.len()..].parse().ok()?;
    }
    Some((vals[0], vals[1], vals[2]))
}

#[cfg(test)]
mod proc_stat_tests {
    use super::*;

    /// Build a plausible /proc/<pid>/stat body: `nfields` space-separated
    /// fields after ") ", starting with the state field exactly like the
    /// kernel's layout, with given values at 0-based indices.
    fn stat_bytes(comm: &str, nfields: usize, set: &[(usize, &str)]) -> Vec<u8> {
        let mut fields: Vec<String> = (0..nfields)
            .map(|i| if i == 0 { "S".into() } else { format!("{}", i) })
            .collect();
        for (i, v) in set {
            if *i < nfields {
                fields[*i] = (*v).to_string();
            }
        }
        format!("4242 ({}) {}", comm, fields.join(" ")).into_bytes()
    }

    #[test]
    fn parses_utime_stime_and_rss_after_paren() {
        // utime/stime are fields 11/12 and rss pages field 21 after ') '
        let stat = stat_bytes("ctest", 22, &[(11, "1000"), (12, "23"), (21, "456")]);
        assert_eq!(parse_stat_usage(&stat), Some((1000, 23, 456)));
    }

    #[test]
    fn comm_with_spaces_and_parentheses_cannot_shift_fields() {
        // The last-')' rule: a comm like "x) S 0 (y" must not be mistaken
        // for the end of the comm field.
        let stat = stat_bytes("x) S 0 (y", 22, &[(11, "7"), (12, "8"), (21, "9")]);
        assert_eq!(parse_stat_usage(&stat), Some((7, 8, 9)));
    }

    #[test]
    fn truncated_or_parenless_stat_is_skipped() {
        // fewer than 22 fields after the comm (process vanished mid-read)
        let short = stat_bytes("t", 21, &[(11, "1"), (12, "2")]);
        assert_eq!(parse_stat_usage(&short), None);
        assert_eq!(parse_stat_usage(b"no parens here"), None);
    }

    #[test]
    fn unparseable_usage_fields_read_as_zero_like_before() {
        // parse_u64 semantics: a present-but-garbage field yields 0
        let stat = stat_bytes("g", 22, &[(11, "x"), (12, ""), (21, "5")]);
        assert_eq!(parse_stat_usage(&stat), Some((0, 0, 5)));
    }
}

#[cfg(test)]
mod mem_psi_tests {
    use super::*;

    #[test]
    fn parses_some_stall_averages_from_psi_content() {
        let content =
            "some avg10=0.53 avg60=1.95 avg300=3.73 total=18604409\nfull avg10=0.00 avg60=0.10 avg300=0.31 total=18467777\n";
        assert_eq!(parse_mem_psi_some(content), Some((0.53, 1.95, 3.73)));
    }

    #[test]
    fn parses_some_line_regardless_of_key_order() {
        let content = "some total=18604409 avg300=3.00 avg60=2.00 avg10=1.00\n";
        assert_eq!(parse_mem_psi_some(content), Some((1.0, 2.0, 3.0)));
    }

    #[test]
    fn missing_or_malformed_some_line_is_unavailable() {
        assert!(parse_mem_psi_some("full avg10=0.00 avg60=0.00 avg300=0.00 total=1\n").is_none());
        assert!(parse_mem_psi_some("some avg10=x avg60=0.00 avg300=0.00\n").is_none());
        assert!(parse_mem_psi_some("some avg10=0.00 avg60=0.00\n").is_none());
    }
}

fn net_totals(networks: &sysinfo::Networks) -> (u64, u64) {
    let mut sent = 0u64;
    let mut recv = 0u64;
    for (_name, data) in networks.iter() {
        sent += data.total_transmitted();
        recv += data.total_received();
    }
    (sent, recv)
}

// ── Physical disk read/write throughput from /sys/block ──

#[derive(Clone, Debug)]
pub struct DiskStat {
    pub name: String,
    pub is_nvme: bool,
    pub read_bps: f64,
    pub write_bps: f64,
}

pub struct DiskTracker {
    prev: HashMap<String, (u64, u64, Instant)>,
}

impl DiskTracker {
    pub fn new() -> Self {
        Self {
            prev: HashMap::new(),
        }
    }

    /// Live read/write bytes-per-second for physical block devices
    /// (nvme/sd/vd/mmcblk), NVMe first then by name. Partitions, loop,
    /// ram, zram and device-mapper entries are skipped.
    pub fn sample(&mut self) -> Vec<DiskStat> {
        let mut out: Vec<DiskStat> = Vec::new();
        let entries = match fs::read_dir("/sys/block") {
            Ok(e) => e,
            Err(_) => return out,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_nvme = name.starts_with("nvme");
            let physical = is_nvme
                || name.starts_with("sd")
                || name.starts_with("vd")
                || name.starts_with("mmcblk");
            if !physical {
                continue;
            }
            let stat = match fs::read_to_string(format!("/sys/block/{}/stat", name)) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let f: Vec<&str> = stat.split_whitespace().collect();
            if f.len() < 10 {
                continue;
            }
            let sectors_read = f[2].parse::<u64>().unwrap_or(0);
            let sectors_written = f[6].parse::<u64>().unwrap_or(0);
            let read_bytes = sectors_read.saturating_mul(512);
            let write_bytes = sectors_written.saturating_mul(512);
            let now = Instant::now();
            let (read_bps, write_bps) = match self.prev.get(&name) {
                Some((pr, pw, at)) => {
                    let dt = now.duration_since(*at).as_secs_f64();
                    if dt > 0.0 {
                        (
                            read_bytes.saturating_sub(*pr) as f64 / dt,
                            write_bytes.saturating_sub(*pw) as f64 / dt,
                        )
                    } else {
                        (0.0, 0.0)
                    }
                }
                None => (0.0, 0.0),
            };
            self.prev.insert(name.clone(), (read_bytes, write_bytes, now));
            out.push(DiskStat {
                name,
                is_nvme,
                read_bps,
                write_bps,
            });
        }
        out.sort_by(|a, b| {
            b.is_nvme
                .cmp(&a.is_nvme)
                .then_with(|| a.name.cmp(&b.name))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_energy_counter_is_unavailable_not_denied() {
        // A machine without RAPL must read as "no data" (n/a), never as
        // "root-only data" (SU req)
        let zone = RaplZone {
            energy_uj_path: PathBuf::from("/nonexistent-rapl-zone/energy_uj"),
            max_energy_uj: 1,
            last_energy_uj: None,
        };
        let (watts, denied) = sample_rapl_cpu_watts(&mut [zone], 1.0);
        assert!(watts.is_none());
        assert!(!denied);
    }
}
