use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::raw::c_void;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct GpuInfo {
    pub id: usize,
    pub name: String,
    pub util: f64,
    pub has_util: bool,
    pub mem_used_gb: f64,
    pub mem_total_gb: f64,
    pub mem_pct: f64,
    pub has_mem: bool,
    pub pci: Option<PciLink>,
    pub power_limit_w: Option<f64>,
    pub pcie_rx_kbs: Option<f64>,
    pub pcie_tx_kbs: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct GpuTemp {
    pub temp: f64,
    pub max: f64,
}

#[derive(Clone, Debug)]
pub struct PciLink {
    pub cur_speed: String,
    pub cur_width: u32,
    pub max_speed: String,
    pub max_width: u32,
}

// "00000000:0A:00.0" (NVML) -> "0000:0a:00.0" (sysfs)
fn normalize_bdf(bus_id: &str) -> String {
    let parts: Vec<&str> = bus_id.split(':').collect();
    if parts.len() == 3 {
        let domain = u32::from_str_radix(parts[0], 16).unwrap_or(0);
        format!(
            "{:04x}:{}:{}",
            domain,
            parts[1].to_lowercase(),
            parts[2].to_lowercase()
        )
    } else {
        bus_id.to_lowercase()
    }
}

// Live PCIe link state from sysfs (works for every vendor, no root).
// The negotiated current link is volatile and read fresh; the max_link_*
// capability pair is static while the device is bound, so it is resolved
// on first use and cached per BDF. A GPU can be swapped at runtime — but
// GPU discovery itself is startup-only here, so the cache is consistent
// with the rest of the app.
static PCI_MAX_CACHE: std::sync::LazyLock<
    std::sync::Mutex<HashMap<String, (String, u32)>>,
> = std::sync::LazyLock::new(Default::default);

fn read_pci_link(bdf: &str) -> Option<PciLink> {
    if bdf.is_empty() {
        return None;
    }
    let base = format!("/sys/bus/pci/devices/{}", bdf);
    let cur_speed = fs::read_to_string(format!("{}/current_link_speed", base))
        .ok()?
        .trim()
        .to_string();
    let cur_width = fs::read_to_string(format!("{}/current_link_width", base))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let cached = PCI_MAX_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.get(bdf).cloned());
    let (max_speed, max_width) = match cached {
        Some(v) => v,
        None => {
            let max_speed = fs::read_to_string(format!("{}/max_link_speed", base))
                .ok()?
                .trim()
                .to_string();
            let max_width = fs::read_to_string(format!("{}/max_link_width", base))
                .ok()?
                .trim()
                .parse()
                .ok()?;
            if let Ok(mut cache) = PCI_MAX_CACHE.lock() {
                cache.insert(bdf.to_string(), (max_speed.clone(), max_width));
            }
            (max_speed, max_width)
        }
    };
    Some(PciLink {
        cur_speed,
        cur_width,
        max_speed,
        max_width,
    })
}

// The Intel driver leaves the endpoint's own current_link_speed/width
// pinned at the idle floor, so the negotiated link has to be read from
// the card's upstream switch port instead. Given the canonical sysfs
// device path (…/pci0000:00/0000:00:03.2/0000:0b:00.0/0000:0c:01.0/
// 0000:0d:00.0), return the top-most PCI-PCI bridge that belongs to the
// card's own stack: the first BDF component after the root bus. Root
// complex ports (bus 00) are platform devices and are skipped, so a GPU
// plugged straight into a root port resolves to itself (unchanged
// behavior) while a GPU behind a switch resolves to the switch's
// upstream port.
fn topmost_bridge_in(dev_path: &std::path::Path) -> Option<String> {
    let mut names: Vec<&OsStr> = dev_path.ancestors().filter_map(|p| p.file_name()).collect();
    names.reverse(); // root → leaf
    names
        .into_iter()
        .find(|n| {
            n.to_str()
                .and_then(parse_bdf)
                .map_or(false, |(_, bus, _, _)| bus != 0)
        })
        .map(|n| n.to_string_lossy().into_owned())
}

// sysfs-resolved shortcut for a live BDF.
fn topmost_bridge_bdf(bdf: &str) -> Option<String> {
    if bdf.is_empty() {
        return None;
    }
    let path = fs::canonicalize(format!("/sys/bus/pci/devices/{}", bdf)).ok()?;
    topmost_bridge_in(&path)
}

// hwmon power-cap attributes are a set-point, not a sensor: they change
// only when someone writes them. Resolve on first read and keep; failures
// are not cached, so a device that never exposes a cap keeps the original
// per-tick behavior (a None path costs nothing either way).
fn cached_power_limit(
    cache: &std::cell::Cell<Option<Option<f64>>>,
    path: &Option<PathBuf>,
) -> Option<f64> {
    if let Some(v) = cache.get() {
        return v;
    }
    let v = path
        .as_ref()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|v| v / 1_000_000.0);
    if v.is_some() {
        cache.set(Some(v));
    }
    v
}

// ── NVIDIA via NVML ──

pub struct NvidiaState {
    nvml: nvml_wrapper::Nvml,
    pub count: usize,
}

impl NvidiaState {
    pub fn init() -> Option<Self> {
        // Try default path first, then versioned .so.1 (common on Linux where
        // the unversioned symlink is only in -dev packages)
        let nvml = nvml_wrapper::Nvml::init()
            .or_else(|_| {
                nvml_wrapper::Nvml::builder()
                    .lib_path(OsStr::new("libnvidia-ml.so.1"))
                    .init()
            })
            .ok()?;
        let count = nvml.device_count().ok()? as usize;
        if count == 0 {
            return None;
        }
        Some(Self { nvml, count })
    }

    pub fn sample(&self, base_id: usize) -> Vec<GpuInfo> {
        let mut gpus = Vec::new();
        for i in 0..self.count {
            if let Ok(dev) = self.nvml.device_by_index(i as u32) {
                let name = dev.name().unwrap_or_else(|_| format!("GPU {}", i));
                let util = dev
                    .utilization_rates()
                    .map(|u| u.gpu as f64)
                    .unwrap_or(0.0);
                let (mem_used, mem_total) = dev
                    .memory_info()
                    .map(|m| (m.used as f64, m.total as f64))
                    .unwrap_or((0.0, 1.0));
                let mem_pct = if mem_total > 0.0 {
                    mem_used / mem_total * 100.0
                } else {
                    0.0
                };
                gpus.push(GpuInfo {
                    id: base_id + i,
                    name,
                    util,
                    has_util: true,
                    mem_used_gb: mem_used / (1024.0 * 1024.0 * 1024.0),
                    mem_total_gb: mem_total / (1024.0 * 1024.0 * 1024.0),
                    mem_pct,
                    has_mem: true,
                    pci: dev
                        .pci_info()
                        .ok()
                        .and_then(|p| read_pci_link(&normalize_bdf(&p.bus_id))),
                    power_limit_w: dev
                        .power_management_limit()
                        .ok()
                        .map(|mw| mw as f64 / 1000.0),
                    pcie_rx_kbs: dev
                        .pcie_throughput(nvml_wrapper::enum_wrappers::device::PcieUtilCounter::Receive)
                        .ok()
                        .map(|v| v as f64),
                    pcie_tx_kbs: dev
                        .pcie_throughput(nvml_wrapper::enum_wrappers::device::PcieUtilCounter::Send)
                        .ok()
                        .map(|v| v as f64),
                });
            }
        }
        gpus
    }

    pub fn power_watts(&self) -> Vec<Option<f64>> {
        let mut powers = Vec::new();
        for i in 0..self.count {
            let power = self
                .nvml
                .device_by_index(i as u32)
                .ok()
                .and_then(|dev| dev.power_usage().ok())
                .map(|mw| mw as f64 / 1000.0);
            powers.push(power);
        }
        powers
    }

    pub fn temps(&self) -> Vec<Option<GpuTemp>> {
        let mut temps = Vec::new();
        for i in 0..self.count {
            if let Ok(dev) = self.nvml.device_by_index(i as u32) {
                let temp = dev
                    .temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
                    .ok()
                    .map(|t| t as f64);
                let max = dev
                    .temperature_threshold(
                        nvml_wrapper::enum_wrappers::device::TemperatureThreshold::Slowdown,
                    )
                    .ok()
                    .map(|t| t as f64)
                    .unwrap_or(95.0);
                temps.push(temp.map(|t| GpuTemp { temp: t, max }));
            } else {
                temps.push(None);
            }
        }
        temps
    }
}

// ── AMD via sysfs ──

#[derive(Clone, Debug)]
pub struct AmdCard {
    pub name: String,
    pub util_path: PathBuf,
    pub has_util: bool,
    pub vram_used_path: PathBuf,
    pub vram_total_bytes: u64,
    pub has_vram: bool,
    pub temp_path: Option<PathBuf>,
    pub temp_crit_path: Option<PathBuf>,
    pub power_path: Option<PathBuf>,
    pub power_cap_path: Option<PathBuf>,
    pub power_scale: f64,
    pub bdf: String,
    power_limit_cache: std::cell::Cell<Option<Option<f64>>>,
}

pub fn detect_amd_gpus() -> Vec<AmdCard> {
    let mut cards = Vec::new();
    let mut seen_devices: HashSet<PathBuf> = HashSet::new();

    let pattern = "/sys/class/drm/card*/device/vendor";
    let entries: Vec<PathBuf> = glob_paths(pattern);

    for vendor_path in entries {
        let vendor = match fs::read_to_string(&vendor_path) {
            Ok(v) => v.trim().to_string(),
            Err(_) => continue,
        };
        if vendor != "0x1002" {
            continue;
        }

        let dev_dir = vendor_path.parent().unwrap().to_path_buf();
        let real_dev = fs::canonicalize(&dev_dir).unwrap_or_else(|_| dev_dir.clone());
        if seen_devices.contains(&real_dev) {
            continue;
        }
        seen_devices.insert(real_dev.clone());

        // GPU name
        let mut name = "AMD GPU".to_string();
        for name_file in &["product_name", "product_description"] {
            if let Ok(n) = fs::read_to_string(dev_dir.join(name_file)) {
                let n = n.trim().to_string();
                if !n.is_empty() {
                    name = n;
                    break;
                }
            }
        }
        if name == "AMD GPU" {
            if let Ok(d) = fs::read_to_string(dev_dir.join("device")) {
                name = format!("AMD GPU [{}]", d.trim());
            }
        }

        let util_path = dev_dir.join("gpu_busy_percent");
        let has_util = util_path.is_file();

        let vram_total_path = dev_dir.join("mem_info_vram_total");
        let vram_used_path = dev_dir.join("mem_info_vram_used");
        let mut has_vram = vram_total_path.is_file() && vram_used_path.is_file();
        let mut vram_total_bytes = 0u64;
        if has_vram {
            match fs::read_to_string(&vram_total_path) {
                Ok(v) => match v.trim().parse::<u64>() {
                    Ok(val) => vram_total_bytes = val,
                    Err(_) => has_vram = false,
                },
                Err(_) => has_vram = false,
            }
        }

        // hwmon temp paths
        let mut temp_path = None;
        let mut temp_crit_path = None;
        let mut power_path = None;
        let mut power_cap_path = None;
        let mut power_scale = 1_000_000.0;
        let hwmon_dir = dev_dir.join("hwmon");
        if hwmon_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&hwmon_dir) {
                let mut hwmons: Vec<_> = entries.filter_map(|e| e.ok()).collect();
                hwmons.sort_by_key(|e| e.file_name());
                if let Some(hw) = hwmons.first() {
                    let t1 = hw.path().join("temp1_input");
                    if t1.is_file() {
                        temp_path = Some(t1);
                    }
                    let tc = hw.path().join("temp1_crit");
                    if tc.is_file() {
                        temp_crit_path = Some(tc);
                    }
                    let p_avg = hw.path().join("power1_average");
                    let p_input = hw.path().join("power1_input");
                    if p_avg.is_file() {
                        power_path = Some(p_avg);
                    } else if p_input.is_file() {
                        power_path = Some(p_input);
                    }
                    let p_cap = hw.path().join("power1_cap");
                    if p_cap.is_file() {
                        power_cap_path = Some(p_cap);
                    } else {
                        power_scale = 1_000_000.0;
                    }
                }
            }
        }

        cards.push(AmdCard {
            name,
            util_path,
            has_util,
            vram_used_path,
            vram_total_bytes,
            has_vram,
            temp_path,
            temp_crit_path,
            power_path,
            power_scale,
            bdf: real_dev
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            power_cap_path,
            power_limit_cache: std::cell::Cell::new(None),
        });
    }
    cards
}

impl AmdCard {
    pub fn sample(&self, id: usize) -> GpuInfo {
        let util = if self.has_util {
            fs::read_to_string(&self.util_path)
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.0)
        } else {
            0.0
        };

        let mem_used = if self.has_vram {
            fs::read_to_string(&self.vram_used_path)
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.0)
        } else {
            0.0
        };

        let mem_total = self.vram_total_bytes as f64;
        let mem_pct = if mem_total > 0.0 {
            mem_used / mem_total * 100.0
        } else {
            0.0
        };

        GpuInfo {
            id,
            name: self.name.clone(),
            util,
            has_util: self.has_util,
            mem_used_gb: mem_used / (1024.0 * 1024.0 * 1024.0),
            mem_total_gb: mem_total / (1024.0 * 1024.0 * 1024.0),
            mem_pct,
            has_mem: true,
            pci: read_pci_link(&self.bdf),
            power_limit_w: cached_power_limit(&self.power_limit_cache, &self.power_cap_path),
            pcie_rx_kbs: None,
            pcie_tx_kbs: None,
        }
    }

    pub fn temp(&self) -> Option<GpuTemp> {
        let temp_path = self.temp_path.as_ref()?;
        let t = fs::read_to_string(temp_path)
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()?
            / 1000.0;
        let max = self
            .temp_crit_path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(|v| v / 1000.0)
            .unwrap_or(95.0);
        Some(GpuTemp { temp: t, max })
    }

    pub fn power_watts(&self) -> Option<f64> {
        let path = self.power_path.as_ref()?;
        let raw = fs::read_to_string(path).ok()?;
        let value = raw.trim().parse::<f64>().ok()?;
        Some(value / self.power_scale)
    }
}

// ── Intel Arc via sysfs (i915/xe) ──

#[derive(Clone, Debug)]
pub struct IntelCard {
    pub name: String,
    pub vram_used_path: Option<PathBuf>,
    pub vram_total_bytes: u64,
    pub temp_path: Option<PathBuf>,
    pub temp_crit_path: Option<PathBuf>,
    pub energy_path: Option<PathBuf>,
    pub power_live_path: Option<PathBuf>,
    pub power_cap_path: Option<PathBuf>,
    last_energy_us: Option<u64>,
    last_energy_at: Option<Instant>,
    pmu_type: Option<u64>,
    active_fd: i32,
    total_fd: i32,
    last_active: Option<u64>,
    last_total: Option<u64>,
    bdf: String,
    // sysfs device whose link attributes reflect the live negotiated
    // PCIe link (the endpoint itself, or its upstream switch port)
    link_bdf: String,
    driver: String,
    l0: Option<L0>,
    fd_prev: HashMap<String, (f64, u64)>,
    power_limit_cache: std::cell::Cell<Option<Option<f64>>>,
}

pub fn detect_intel_gpus() -> Vec<IntelCard> {
    detect_intel_gpus_in(std::path::Path::new("/sys"), true)
}

fn read_u64(path: &std::path::Path) -> Option<u64> {
    fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

fn read_bar2_bytes(dev_dir: &std::path::Path) -> Option<u64> {
    // Intel discrete GPUs map the full VRAM as a 1:1 MMIO aperture in BAR2;
    // the xe driver exposes no mem_info_vram_total, so use the BAR size.
    let res = fs::read_to_string(dev_dir.join("resource")).ok()?;
    let line = res.lines().nth(2)?;
    let mut it = line.split_whitespace();
    let start = u64::from_str_radix(it.next()?.trim_start_matches("0x"), 16).ok()?;
    let end = u64::from_str_radix(it.next()?.trim_start_matches("0x"), 16).ok()?;
    let flags = u64::from_str_radix(it.next()?.trim_start_matches("0x"), 16).ok()?;
    if flags & 1 != 0 {
        return None; // I/O port, not memory
    }
    let size = end.checked_sub(start)?.checked_add(1)?;
    (size >= 1 << 30 && size <= 256 << 30).then_some(size)
}

// ── Level Zero sysman: VRAM via libze_loader (userspace, no root/perf) ──

#[repr(C)]
struct ZePciAddr {
    domain: u32,
    bus: u32,
    device: u32,
    function: u32,
}

#[repr(C)]
struct ZePciSpeed {
    gen: i32,
    width: i32,
    max_bandwidth: i64,
}

#[repr(C)]
struct ZePciProps {
    stype: u32,
    _pad: u32,
    pnext: *mut c_void,
    address: ZePciAddr,
    max_speed: ZePciSpeed,
    max_bw: u8,
    max_pkt: u8,
    max_replay: u8,
}

#[repr(C)]
struct ZeMemState {
    stype: u32,
    _pad: u32,
    pnext: *mut c_void,
    health: i32,
    _pad2: i32,
    free: u64,
    size: u64,
}

#[derive(Clone, Debug)]
pub struct L0 {
    dev: *mut c_void,
    enum_mem: extern "C" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32,
    mem_state: extern "C" fn(*mut c_void, *mut ZeMemState) -> i32,
}

fn parse_bdf(bdf: &str) -> Option<(u32, u32, u32, u32)> {
    let (d, rest) = bdf.split_once(':')?;
    let (b, rest) = rest.split_once(':')?;
    let (dev, func) = rest.split_once('.')?;
    Some((
        u32::from_str_radix(d, 16).ok()?,
        u32::from_str_radix(b, 16).ok()?,
        u32::from_str_radix(dev, 16).ok()?,
        u32::from_str_radix(func, 16).ok()?,
    ))
}

impl L0 {
    fn open(bdf: &str) -> Option<L0> {
        // NEO exposes VRAM through sysman; engine activity is not exposed on
        // Battlemage, so utilization comes from DRM fdinfo instead.
        let c_name = std::ffi::CString::new("libze_loader.so.1").ok()?;
        let lib = unsafe { libc::dlopen(c_name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if lib.is_null() {
            return None;
        }
        macro_rules! sym {
            ($n:expr, $t:ty) => {{
                let p = unsafe { libc::dlsym(lib, std::ffi::CString::new($n).unwrap().as_ptr()) };
                if p.is_null() {
                    return None;
                }
                unsafe { std::mem::transmute::<*mut c_void, $t>(p) }
            }};
        }
        let init: extern "C" fn(u32) -> i32 = sym!("zesInit", extern "C" fn(u32) -> i32);
        let driver_get: extern "C" fn(*mut u32, *mut *mut c_void) -> i32 =
            sym!("zesDriverGet", extern "C" fn(*mut u32, *mut *mut c_void) -> i32);
        let device_get: extern "C" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32 =
            sym!("zesDeviceGet", extern "C" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32);
        let pci_props: extern "C" fn(*mut c_void, *mut ZePciProps) -> i32 = sym!(
            "zesDevicePciGetProperties",
            extern "C" fn(*mut c_void, *mut ZePciProps) -> i32
        );
        let enum_mem: extern "C" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32 = sym!(
            "zesDeviceEnumMemoryModules",
            extern "C" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32
        );
        let mem_state: extern "C" fn(*mut c_void, *mut ZeMemState) -> i32 = sym!(
            "zesMemoryGetState",
            extern "C" fn(*mut c_void, *mut ZeMemState) -> i32
        );
        // NEO gates sysman behind this env var; set it before zesInit.
        unsafe {
            let k = std::ffi::CString::new("ZES_ENABLE_SYSMAN").unwrap();
            let v = std::ffi::CString::new("1").unwrap();
            libc::setenv(k.as_ptr(), v.as_ptr(), 1);
        }
        if init(0) != 0 {
            return None;
        }
        let (dom, bus, dev, func) = parse_bdf(bdf)?;
        let mut n = 0u32;
        if driver_get(&mut n, std::ptr::null_mut()) != 0 || n == 0 {
            return None;
        }
        let mut drv: Vec<*mut c_void> = vec![std::ptr::null_mut(); n as usize];
        if driver_get(&mut n, drv.as_mut_ptr()) != 0 {
            return None;
        }
        for d in drv {
            let mut m = 0u32;
            if device_get(d, &mut m, std::ptr::null_mut()) != 0 || m == 0 {
                continue;
            }
            let mut devs: Vec<*mut c_void> = vec![std::ptr::null_mut(); m as usize];
            if device_get(d, &mut m, devs.as_mut_ptr()) != 0 {
                continue;
            }
            for devh in devs {
                let mut pp = ZePciProps {
                    stype: 1,
                    _pad: 0,
                    pnext: std::ptr::null_mut(),
                    address: ZePciAddr {
                        domain: 0,
                        bus: 0,
                        device: 0,
                        function: 0,
                    },
                    max_speed: ZePciSpeed {
                        gen: 0,
                        width: 0,
                        max_bandwidth: 0,
                    },
                    max_bw: 0,
                    max_pkt: 0,
                    max_replay: 0,
                };
                if pci_props(devh, &mut pp) != 0 {
                    continue;
                }
                if pp.address.domain == dom
                    && pp.address.bus == bus
                    && pp.address.device == dev
                    && pp.address.function == func
                {
                    return Some(L0 {
                        dev: devh,
                        enum_mem,
                        mem_state,
                    });
                }
            }
        }
        None
    }

    fn mem_used_total(&self) -> Option<(u64, u64)> {
        let mut n = 0u32;
        if (self.enum_mem)(self.dev, &mut n, std::ptr::null_mut()) != 0 || n == 0 {
            return None;
        }
        let mut mods: Vec<*mut c_void> = vec![std::ptr::null_mut(); n as usize];
        if (self.enum_mem)(self.dev, &mut n, mods.as_mut_ptr()) != 0 {
            return None;
        }
        let mut total = 0u64;
        let mut free = 0u64;
        for m in mods {
            let mut st = ZeMemState {
                stype: 16,
                _pad: 0,
                pnext: std::ptr::null_mut(),
                health: 0,
                _pad2: 0,
                free: 0,
                size: 0,
            };
            if (self.mem_state)(m, &mut st) == 0 && st.size > 0 {
                total += st.size;
                free += st.free;
            }
        }
        if total > 0 {
            Some((total.saturating_sub(free), total))
        } else {
            None
        }
    }
}

fn parse_size_kb(s: &str) -> Option<u64> {
    // fdinfo sizes look like "33145396 KiB"
    let mut it = s.split_whitespace();
    let n: u64 = it.next()?.parse().ok()?;
    let mult: u64 = match it.next().unwrap_or("B") {
        "B" => 1,
        "KiB" => 1024,
        "MiB" => 1024 * 1024,
        "GiB" => 1024 * 1024 * 1024,
        _ => return None,
    };
    n.checked_mul(mult)
}

fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn fdinfo_scan(
    proc_root: &std::path::Path,
    driver: &str,
    bdf: &str,
    prev: &mut HashMap<String, (f64, u64)>,
) -> (Option<f64>, Option<u64>) {
    // The xe/i915 drivers expose per-DRM-client engine cycles and VRAM in
    // /proc/<pid>/fdinfo, readable for the user's own processes. Utilization
    // = Δbusy cycles / Δtotal cycles across clients; VRAM used = Σ active-vram0.
    if bdf.is_empty() {
        return (None, None);
    }
    let mut cur: HashMap<String, (f64, u64)> = HashMap::new();
    let mut sum_vram = 0u64;
    let mut saw_vram = false;
    let pids = match fs::read_dir(proc_root) {
        Ok(r) => r,
        Err(_) => return (None, None),
    };
    for pid in pids.flatten() {
        let name = pid.file_name().to_string_lossy().to_string();
        if name.is_empty() || !name.bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let fds = match fs::read_dir(pid.path().join("fdinfo")) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for fd in fds.flatten() {
            let text = match fs::read_to_string(fd.path()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if !text.contains("drm-driver:") {
                continue;
            }
            let mut kv: HashMap<&str, &str> = HashMap::new();
            for line in text.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    kv.entry(k.trim_end()).or_insert(v.trim());
                }
            }
            if kv.get("drm-driver").map(|v| v.trim()) != Some(driver)
                || kv.get("drm-pdev").map(|v| v.trim()) != Some(bdf)
            {
                continue;
            }
            let cid = kv
                .get("drm-client-id")
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let busy = if driver == "i915" {
                ["drm-engine-render", "drm-engine-compute"]
                    .iter()
                    .map(|k| {
                        kv.get(*k)
                            .and_then(|v| v.trim_end_matches(" ns").trim().parse::<f64>().ok())
                            .unwrap_or(0.0)
                    })
                    .sum()
            } else {
                ["drm-cycles-rcs", "drm-cycles-ccs"]
                    .iter()
                    .map(|k| {
                        let c = kv
                            .get(*k)
                            .and_then(|v| v.trim().parse::<f64>().ok())
                            .unwrap_or(0.0);
                        let cap = kv
                            .get(format!("drm-engine-capacity-{}", k.trim_start_matches("drm-cycles-")).as_str())
                            .and_then(|v| v.trim().parse::<f64>().ok())
                            .filter(|v| *v > 0.0)
                            .unwrap_or(1.0);
                        c / cap
                    })
                    .sum()
            };
            let total = if driver == "i915" {
                monotonic_ns()
            } else {
                kv.get("drm-total-cycles-ccs")
                    .or_else(|| kv.get("drm-total-cycles-rcs"))
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(0)
            };
            if !cid.is_empty() && (busy > 0.0 || total > 0) {
                cur.insert(cid, (busy, total));
            }
            if let Some(v) = kv.get("drm-active-vram0").and_then(|v| parse_size_kb(v)) {
                sum_vram += v;
                saw_vram = true;
            }
        }
    }
    let mut max_span = 0u64;
    let mut used = 0f64;
    for (cid, (b, t)) in &cur {
        if let Some((pb, pt)) = prev.get(cid) {
            if t > pt {
                max_span = max_span.max(t - pt);
            }
            used += (b - pb).max(0.0);
        }
    }
    let util = if max_span > 0 {
        Some((used / max_span as f64 * 100.0).clamp(0.0, 100.0))
    } else if !cur.is_empty() {
        Some(0.0)
    } else {
        None
    };
    *prev = cur;
    (util, if saw_vram { Some(sum_vram) } else { None })
}

// Stable perf_event ABI (VER0, 48 bytes); libc does not export the struct.
#[repr(C)]
struct PerfEventAttr {
    type_: u32,
    size: u32,
    config: u64,
    flags: u64,
    sample_period: u64,
    sample_type: u64,
    read_format: u64,
    flags2: u64,
}

fn open_pmu_counter(pmu_type: u64, config: u64, group_fd: i32) -> Option<i32> {
    let mut attr = PerfEventAttr {
        type_: pmu_type as u32,
        size: std::mem::size_of::<PerfEventAttr>() as u32,
        config,
        flags: 0,
        sample_period: 0,
        sample_type: 0,
        read_format: 0,
        flags2: 0,
    };
    // pid=-1, cpu=0: system-wide counting on the device PMU.
    // glibc has no perf_event_open wrapper; use the raw syscall.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            &mut attr as *mut PerfEventAttr,
            -1i32,
            0i32,
            group_fd,
            0u64,
        )
    };
    if fd < 0 {
        None
    } else {
        Some(fd as i32)
    }
}

fn read_u64_fd(fd: i32) -> Option<u64> {
    let mut v: u64 = 0;
    let n = unsafe { libc::read(fd, &mut v as *mut u64 as *mut libc::c_void, 8) };
    if n == 8 {
        Some(v)
    } else {
        None
    }
}

fn detect_intel_gpus_in(sys_root: &std::path::Path, l0_enabled: bool) -> Vec<IntelCard> {
    let mut cards = Vec::new();
    let mut seen_devices: HashSet<PathBuf> = HashSet::new();

    for vendor_path in glob_vendor_paths(&sys_root.join("class/drm")) {
        let vendor = match fs::read_to_string(&vendor_path) {
            Ok(v) => v.trim().to_string(),
            Err(_) => continue,
        };
        if vendor != "0x8086" {
            continue;
        }

        let dev_dir = vendor_path.parent().unwrap().to_path_buf();
        let real_dev = fs::canonicalize(&dev_dir).unwrap_or_else(|_| dev_dir.clone());
        if seen_devices.contains(&real_dev) {
            continue;
        }
        seen_devices.insert(real_dev);

        let device_id = fs::read_to_string(dev_dir.join("device"))
            .map(|d| d.trim().to_lowercase())
            .unwrap_or_default();
        let name = match device_id.as_str() {
            "0x56a0" => "Intel Arc A770",
            "0x56a1" => "Intel Arc A750",
            "0x56b0" => "Intel Arc A380",
            "0x56a9" => "Intel Arc A310",
            "0xe20b" => "Intel Arc B580",
            "0xe223" => "Intel Arc Pro B70",
            _ => "",
        }
        .to_string();
        let name = if !name.is_empty() {
            name
        } else if !device_id.is_empty() {
            format!("Intel GPU [{}]", device_id)
        } else {
            "Intel GPU".to_string()
        };

        // VRAM files exist only on i915 (Alchemist); the xe driver
        // (Battlemage) exposes no mem_info_vram_* files. Fall back to the
        // BAR2 aperture size for the total; used stays unexposed (n/a).
        let vram_total_path = dev_dir.join("mem_info_vram_total");
        let vram_used_path = dev_dir.join("mem_info_vram_used");
        let vram_total_bytes = read_u64(&vram_total_path)
            .filter(|v| *v > 0)
            .or_else(|| read_bar2_bytes(&dev_dir))
            .unwrap_or(0);
        let has_mem = vram_used_path.is_file();
        let vram_used_path = if has_mem {
            Some(vram_used_path)
        } else {
            None
        };

        // uevent gives the PCI slot (PMU + Level Zero + fdinfo key) and driver.
        let uevent = fs::read_to_string(dev_dir.join("uevent")).unwrap_or_default();
        let mut bdf = String::new();
        let mut driver = String::new();
        for line in uevent.lines() {
            if let Some(s) = line.strip_prefix("PCI_SLOT_NAME=") {
                bdf = s.trim().to_string();
            } else if let Some(s) = line.strip_prefix("DRIVER=") {
                driver = s.trim().to_string();
            }
        }
        // xe PMU (engine activity) is keyed by PCI slot: xe_<bdf>/type.
        let pmu_type = if bdf.is_empty() {
            None
        } else {
            read_u64(std::path::Path::new(&format!(
                "/sys/bus/event_source/devices/xe_{}/type",
                bdf.replace(':', "_")
            )))
        };
        // Level Zero sysman gives card-wide VRAM used/total without root.
        let l0 = if l0_enabled && !bdf.is_empty() {
            L0::open(&bdf)
        } else {
            None
        };

        // hwmon layout differs by driver:
        // - i915: temp1_input (m°C), power1_max (µW, live package power)
        // - xe: labeled temps (pkg/vram/mctrl/pcie/vram_ch_*), live power
        //   only as energy1_input (µJ cumulative) → delta per second
        let mut temp_path = None;
        let mut temp_crit_path = None;
        let mut energy_path = None;
        let mut power_live_path = None;
        let mut power_cap_path = None;
        let hwmon_dir = dev_dir.join("hwmon");
        if hwmon_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&hwmon_dir) {
                let mut hwmons: Vec<_> = entries.filter_map(|e| e.ok()).collect();
                hwmons.sort_by_key(|e| e.file_name());
                if let Some(hw) = hwmons.first() {
                    let hw_path = hw.path();
                    // Prefer the input labeled "pkg" (xe), else temp1 (i915).
                    let mut inputs: Vec<u32> = fs::read_dir(&hw_path)
                        .ok()
                        .map(|it| {
                            it.filter_map(|e| e.ok())
                                .filter_map(|e| {
                                    let n = e.file_name().to_str()?.to_string();
                                    let rest = n.strip_prefix("temp")?.strip_suffix("_input")?;
                                    rest.parse::<u32>().ok()
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    inputs.sort_unstable();
                    let pkg = inputs.iter().find(|n| {
                        fs::read_to_string(hw_path.join(format!("temp{}_label", n)))
                            .map(|l| l.trim() == "pkg")
                            .unwrap_or(false)
                    });
                    let chosen = pkg
                        .copied()
                        .or_else(|| if inputs.contains(&1) { Some(1) } else { None });
                    if let Some(n) = chosen {
                        temp_path = Some(hw_path.join(format!("temp{}_input", n)));
                        let crit = hw_path.join(format!("temp{}_crit", n));
                        let max = hw_path.join(format!("temp{}_max", n));
                        if crit.is_file() {
                            temp_crit_path = Some(crit);
                        } else if max.is_file() {
                            temp_crit_path = Some(max);
                        }
                    }
                    let e1 = hw_path.join("energy1_input");
                    let e2 = hw_path.join("energy2_input");
                    if e1.is_file() {
                        energy_path = Some(e1);
                    } else if e2.is_file() {
                        energy_path = Some(e2);
                    }
                    let p_max = hw_path.join("power1_max");
                    let p_avg = hw_path.join("power1_average");
                    let p_cap = hw_path.join("power1_cap");
                    if p_cap.is_file() {
                        power_cap_path = Some(p_cap);
                    } else if p_max.is_file() {
                        power_cap_path = Some(p_max.clone());
                    }
                    if p_max.is_file() {
                        power_live_path = Some(p_max);
                    } else if p_avg.is_file() {
                        power_live_path = Some(p_avg);
                    }
                }
            }
        }

        // Intel drivers pin the endpoint's own link attributes at the
        // idle floor; when the card sits behind a switch, the live
        // negotiated speed/width live on the upstream port instead.
        // Resolve once — the topology is static while running.
        let link_bdf = match topmost_bridge_bdf(&bdf) {
            Some(top) if read_pci_link(&top).is_some() => top,
            _ => bdf.clone(),
        };

        cards.push(IntelCard {
            name,
            vram_used_path,
            vram_total_bytes,
            temp_path,
            temp_crit_path,
            energy_path,
            power_live_path,
            power_cap_path,
            last_energy_us: None,
            last_energy_at: None,
            pmu_type,
            active_fd: -1,
            total_fd: -1,
            last_active: None,
            last_total: None,
            bdf,
            link_bdf,
            driver,
            l0,
            fd_prev: HashMap::new(),
            power_limit_cache: std::cell::Cell::new(None),
        });
    }
    cards
}

impl IntelCard {
    pub fn sample(&mut self, id: usize) -> GpuInfo {
        let pmu_util = self.util_pct();
        let l0_mem = self.l0.as_ref().and_then(|l| l.mem_used_total());
        let sysfs_vram = self
            .vram_used_path
            .as_ref()
            .and_then(|p| read_u64(p))
            .filter(|_| l0_mem.is_none());
        // The fdinfo walk touches every /proc/<pid>/fdinfo on the box, so it
        // runs only when its answers are actually needed: utilization when
        // the PMU is unavailable, and VRAM when neither Level Zero nor sysfs
        // exposes it. With both covered the scan result would be discarded.
        let (fd_util, fd_vram) = if pmu_util.is_some() && (l0_mem.is_some() || sysfs_vram.is_some())
        {
            (None, None)
        } else {
            fdinfo_scan(
                std::path::Path::new("/proc"),
                &self.driver,
                &self.bdf,
                &mut self.fd_prev,
            )
        };
        let util = pmu_util.or(fd_util);
        let mem_used_bytes: Option<u64> = l0_mem
            .map(|(u, _)| u)
            .or(sysfs_vram)
            .or(fd_vram);
        let mem_total_bytes = l0_mem
            .map(|(_, t)| t)
            .filter(|t| *t > 0)
            .unwrap_or(self.vram_total_bytes);
        let has_mem = mem_used_bytes.is_some();
        let mem_used = mem_used_bytes.unwrap_or(0) as f64;
        let mem_total = mem_total_bytes as f64;
        let mem_pct = if has_mem && mem_total > 0.0 {
            mem_used / mem_total * 100.0
        } else {
            0.0
        };
        GpuInfo {
            id,
            name: self.name.clone(),
            util: util.unwrap_or(0.0),
            has_util: util.is_some(),
            has_mem,
            mem_used_gb: mem_used / (1024.0 * 1024.0 * 1024.0),
            mem_total_gb: mem_total / (1024.0 * 1024.0 * 1024.0),
            mem_pct,
            pci: read_pci_link(&self.link_bdf),
            power_limit_w: cached_power_limit(&self.power_limit_cache, &self.power_cap_path),
            pcie_rx_kbs: None,
            pcie_tx_kbs: None,
        }
    }

    pub fn util_pct(&mut self) -> Option<f64> {
        // Intel exposes no sysfs utilization counter; the xe PMU provides
        // engine-active-ticks (0x02) / engine-total-ticks (0x03) with
        // engine_instance at config bits 12-19 and engine_class at 20-27
        // (render class 0, instance 0). Requires perf_event_paranoid <= 0;
        // when the open is denied, utilization stays n/a.
        let pmu_type = self.pmu_type?;
        if self.active_fd < 0 {
            let active = open_pmu_counter(pmu_type, 0x02, -1)?;
            match open_pmu_counter(pmu_type, 0x03, active) {
                Some(total) => {
                    self.active_fd = active;
                    self.total_fd = total;
                }
                None => {
                    unsafe { libc::close(active) };
                    self.pmu_type = None;
                    return None;
                }
            }
        }
        let a = read_u64_fd(self.active_fd)?;
        let t = read_u64_fd(self.total_fd)?;
        let util = match (self.last_active, self.last_total) {
            (Some(la), Some(lt)) if t > lt => {
                Some(a.saturating_sub(la) as f64 / (t - lt) as f64 * 100.0)
            }
            _ => None,
        };
        self.last_active = Some(a);
        self.last_total = Some(t);
        util.map(|u| u.clamp(0.0, 100.0))
    }

    pub fn temp(&self) -> Option<GpuTemp> {
        let t = fs::read_to_string(self.temp_path.as_ref()?)
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()?
            / 1000.0;
        let max = self
            .temp_crit_path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(|v| v / 1000.0)
            .unwrap_or(95.0);
        Some(GpuTemp { temp: t, max })
    }

    pub fn power_watts(&mut self) -> Option<f64> {
        if let Some(path) = self.energy_path.as_ref() {
            let now = Instant::now();
            let energy = read_u64(path)?;
            let watts = match (self.last_energy_us, self.last_energy_at) {
                (Some(prev), Some(prev_at)) if energy >= prev => {
                    let dt = (now - prev_at).as_secs_f64();
                    if dt > 0.0 {
                        Some((energy - prev) as f64 / 1_000_000.0 / dt)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            self.last_energy_us = Some(energy);
            self.last_energy_at = Some(now);
            if watts.is_some() {
                return watts;
            }
            // First sample (no baseline) or counter reset: fall through.
        }
        let raw = fs::read_to_string(self.power_live_path.as_ref()?).ok()?;
        let value = raw.trim().parse::<f64>().ok()?;
        Some(value / 1_000_000.0)
    }
}

fn glob_vendor_paths(drm_dir: &std::path::Path) -> Vec<PathBuf> {
    // List card*/device/vendor paths under a DRM directory, sorted.
    // Connector dirs (cardN-*) have no device/ subdir and are skipped.
    let mut results = Vec::new();
    if let Ok(entries) = fs::read_dir(drm_dir) {
        let mut dirs: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.starts_with("card"))
                    .unwrap_or(false)
            })
            .collect();
        dirs.sort_by_key(|e| e.file_name());
        for d in dirs {
            let p = d.path().join("device/vendor");
            if p.is_file() {
                results.push(p);
            }
        }
    }
    results
}

fn glob_paths(pattern: &str) -> Vec<PathBuf> {
    // Simple glob for /sys/class/drm/card*/device/vendor
    let results = glob_vendor_paths(std::path::Path::new("/sys/class/drm"));
    let _ = pattern; // suppress unused
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(path: &std::path::Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn detects_intel_gpus_across_xe_and_i915_layouts() {
        let root = std::env::temp_dir().join(format!("ktop_intel_test_{}", std::process::id()));
        let drm = root.join("class/drm");
        let _ = fs::remove_dir_all(&root);

        // Battlemage dGPU on xe: labeled temps, energy counter, no VRAM files
        let d0 = drm.join("card0/device");
        write_file(&d0.join("vendor"), "0x8086\n");
        write_file(&d0.join("device"), "0xe223\n");
        write_file(
            &d0.join("resource"),
            "0x00000000c0000000 0x00000000c1ffffff 0x0000000000040200\n\
             0x0000000000000000 0x0000000000000000 0x0000000000000000\n\
             0x00000000c4000000 0x00000008c3ffffff 0x000000000040200c\n",
        );
        write_file(&d0.join("uevent"), "DRIVER=xe\nPCI_SLOT_NAME=0000:99:00.0\n");
        let hw0 = d0.join("hwmon/hwmon10");
        write_file(&hw0.join("temp2_input"), "66000\n");
        write_file(&hw0.join("temp2_label"), "pkg\n");
        write_file(&hw0.join("temp2_crit"), "105000\n");
        write_file(&hw0.join("temp3_input"), "68000\n");
        write_file(&hw0.join("temp3_label"), "vram\n");
        write_file(&hw0.join("energy1_input"), "100000000\n");
        write_file(&hw0.join("energy1_label"), "card\n");

        // iGPU: vendor 0x8086, no VRAM, no hwmon
        let d1 = drm.join("card1/device");
        write_file(&d1.join("vendor"), "0x8086\n");
        write_file(&d1.join("device"), "0x4680\n");

        // AMD card must not be picked up
        let d2 = drm.join("card2/device");
        write_file(&d2.join("vendor"), "0x1002\n");
        write_file(&d2.join("device"), "0x744c\n");

        // Alchemist dGPU on i915: temp1_input, power1_max, VRAM files
        let d3 = drm.join("card3/device");
        write_file(&d3.join("vendor"), "0x8086\n");
        write_file(&d3.join("device"), "0x56a0\n");
        write_file(&d3.join("mem_info_vram_total"), "17179869184\n");
        write_file(&d3.join("mem_info_vram_used"), "2147483648\n");
        let hw3 = d3.join("hwmon/hwmon5");
        write_file(&hw3.join("temp1_input"), "45000\n");
        write_file(&hw3.join("temp1_max"), "110000\n");
        write_file(&hw3.join("power1_max"), "12500000\n");

        let mut cards = detect_intel_gpus_in(&root, false);
        assert_eq!(cards.len(), 3);

        // xe card: product name, pkg-labeled temp, energy-delta power,
        // BAR2 total with used unexposed
        let xe = cards
            .iter_mut()
            .find(|c| c.name == "Intel Arc Pro B70")
            .unwrap();
        let t = xe.temp().unwrap();
        assert!((t.temp - 66.0).abs() < 1e-6);
        assert!((t.max - 105.0).abs() < 1e-6);
        assert!(xe.power_watts().is_none()); // first sample sets the baseline
        write_file(&hw0.join("energy1_input"), "200000000\n");
        std::thread::sleep(std::time::Duration::from_millis(60));
        let w = xe.power_watts().unwrap();
        assert!(w > 100.0 && w < 10000.0, "energy delta power {}W", w);
        let info = xe.sample(0);
        assert!(!info.has_util); // first PMU sample is the baseline
        assert!(!info.has_mem);
        assert!((info.mem_total_gb - 32.0).abs() < 1e-6);

        // iGPU: included with no sensors
        let igpu = cards
            .iter_mut()
            .find(|c| c.name == "Intel GPU [0x4680]")
            .unwrap();
        assert!(igpu.temp().is_none());
        assert!(igpu.power_watts().is_none());
        let info = igpu.sample(1);
        assert!(!info.has_mem);
        assert!(info.mem_total_gb == 0.0);

        // i915 card: product name, temp1 + power1_max + VRAM
        let i915 = cards
            .iter_mut()
            .find(|c| c.name == "Intel Arc A770")
            .unwrap();
        let t = i915.temp().unwrap();
        assert!((t.temp - 45.0).abs() < 1e-6);
        assert!((t.max - 110.0).abs() < 1e-6);
        assert!((i915.power_watts().unwrap() - 12.5).abs() < 1e-6);
        let info = i915.sample(2);
        assert!(info.has_mem);
        assert!((info.mem_total_gb - 16.0).abs() < 1e-6);
        assert!((info.mem_used_gb - 2.0).abs() < 1e-6);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fdinfo_scan_computes_util_and_vram_from_fake_proc() {
        let root = std::env::temp_dir().join(format!("ktop_fdinfo_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let fd = root.join("1234/fdinfo");
        write_file(
            &fd.join("3"),
            "pos:\t0\nflags:\t02100002\n\
             drm-driver:\txe\ndrm-client-id:\t2092\ndrm-pdev:\t0000:99:00.0\n\
             drm-active-vram0:\t33059392 KiB\n\
             drm-cycles-rcs:\t0\ndrm-total-cycles-rcs:\t1000\n\
             drm-cycles-ccs:\t500\ndrm-engine-capacity-ccs:\t2\ndrm-total-cycles-ccs:\t1000\n",
        );
        let mut prev = HashMap::new();
        let (u1, v1) = fdinfo_scan(&root, "xe", "0000:99:00.0", &mut prev);
        assert_eq!(u1, Some(0.0)); // first scan: client visible, no delta yet
        assert_eq!(v1, Some(33059392 * 1024));
        write_file(
            &fd.join("3"),
            "pos:\t0\nflags:\t02100002\n\
             drm-driver:\txe\ndrm-client-id:\t2092\ndrm-pdev:\t0000:99:00.0\n\
             drm-active-vram0:\t33059392 KiB\n\
             drm-cycles-rcs:\t0\ndrm-total-cycles-rcs:\t2000\n\
             drm-cycles-ccs:\t1500\ndrm-engine-capacity-ccs:\t2\ndrm-total-cycles-ccs:\t2000\n",
        );
        let (u2, v2) = fdinfo_scan(&root, "xe", "0000:99:00.0", &mut prev);
        // Δbusy = (1500-500)/2 = 500, Δtotal = 1000 → 50%
        assert!((u2.unwrap() - 50.0).abs() < 1e-6);
        assert_eq!(v2, Some(33059392 * 1024));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn picks_topmost_non_root_bridge_from_sysfs_path() {
        // Battlemage behind a switch: root port 00:03.2 is skipped, the
        // switch upstream port 0b:00.0 wins over downstream 0c:01.0 and
        // the endpoint 0d:00.0
        let p = std::path::Path::new(
            "/sys/devices/pci0000:00/0000:00:03.2/0000:0b:00.0/0000:0c:01.0/0000:0d:00.0",
        );
        assert_eq!(topmost_bridge_in(p).as_deref(), Some("0000:0b:00.0"));

        // GPU straight on a root port: the endpoint itself is the source
        let p = std::path::Path::new("/sys/devices/pci0000:00/0000:00:03.1/0000:0a:00.0");
        assert_eq!(topmost_bridge_in(p).as_deref(), Some("0000:0a:00.0"));

        // Device on the root bus itself: nothing above it
        let p = std::path::Path::new("/sys/devices/pci0000:00/0000:00:1f.0");
        assert_eq!(topmost_bridge_in(p), None);

        // Non-PCI path components never match
        let p = std::path::Path::new("/sys/devices/virtual/thermal/thermal_zone0");
        assert_eq!(topmost_bridge_in(p), None);
    }
}
