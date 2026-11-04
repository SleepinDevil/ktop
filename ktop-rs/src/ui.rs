use crate::app::{AppState, ProfBlock, ProfStats, PROF_BLOCKS};
use crate::theme::Theme;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, Widget};
use ratatui::Frame;
use std::collections::VecDeque;

const SPARK: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const SPARK_DOWN: &[char] = &[
    ' ',
    '▔',
    '\u{1FB82}',
    '\u{1FB83}',
    '▀',
    '\u{1FB84}',
    '\u{1FB85}',
    '\u{1FB86}',
    '█',
];

// ── Sparkline helpers ──

// `scale` maps raw values (e.g. bytes/s, watts) onto the 0..100 range the
// glyph tables expect: v/scale*100 clamped. None passes percentage values
// through unchanged. Callers avoid building a renormalized copy per frame.

fn sparkline(values: &VecDeque<f64>, width: usize, scale: Option<f64>) -> String {
    let mut out = String::new();
    for v in values.iter().skip(values.len().saturating_sub(width)) {
        let v = match scale {
            Some(mx) => (v / mx * 100.0).clamp(0.0, 100.0),
            None => *v,
        }
        .clamp(0.0, 100.0);
        let idx = if v <= 5.0 {
            0
        } else {
            let i = 1 + ((v - 5.0) / 95.0 * (SPARK.len() as f64 - 2.0)) as usize;
            i.min(SPARK.len() - 1)
        };
        out.push(SPARK[idx]);
    }
    out
}

fn sparkline_double(values: &VecDeque<f64>, width: usize) -> (String, String) {
    let n = SPARK.len() - 1; // 8 levels per row
    let mut top = String::new();
    let mut bot = String::new();
    for v in values.iter().skip(values.len().saturating_sub(width)) {
        let v = v.clamp(0.0, 100.0);
        let level = ((v / 100.0) * (2 * n) as f64) as usize;
        let level = level.min(2 * n);
        if level <= n {
            top.push(SPARK[0]);
            bot.push(SPARK[level]);
        } else {
            bot.push(SPARK[n]);
            top.push(SPARK[level - n]);
        }
    }
    (top, bot)
}

fn sparkline_down(values: &VecDeque<f64>, width: usize, scale: Option<f64>) -> String {
    let mut out = String::new();
    for v in values.iter().skip(values.len().saturating_sub(width)) {
        let v = match scale {
            Some(mx) => (v / mx * 100.0).clamp(0.0, 100.0),
            None => *v,
        }
        .clamp(0.0, 100.0);
        let idx = (v / 100.0 * (SPARK_DOWN.len() as f64 - 1.0)) as usize;
        out.push(SPARK_DOWN[idx.min(SPARK_DOWN.len() - 1)]);
    }
    out
}

// ── Bar rendering ──

fn color_to_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Green => (0, 128, 0),
        Color::Yellow => (255, 255, 0),
        Color::Red => (255, 0, 0),
        Color::Cyan => (0, 255, 255),
        Color::Magenta => (255, 0, 255),
        Color::White => (255, 255, 255),
        Color::LightGreen => (144, 238, 144),
        Color::LightYellow => (255, 255, 224),
        Color::LightRed => (255, 128, 128),
        Color::LightCyan => (224, 255, 255),
        Color::LightMagenta => (255, 128, 255),
        Color::DarkGray => (128, 128, 128),
        _ => (255, 255, 255),
    }
}

fn lerp_color(c1: Color, c2: Color, t: f64) -> Color {
    let (r1, g1, b1) = color_to_rgb(c1);
    let (r2, g2, b2) = color_to_rgb(c2);
    let r = (r1 as f64 + (r2 as f64 - r1 as f64) * t) as u8;
    let g = (g1 as f64 + (g2 as f64 - g1 as f64) * t) as u8;
    let b = (b1 as f64 + (b2 as f64 - b1 as f64) * t) as u8;
    Color::Rgb(r, g, b)
}

fn truecolor_enabled(state: &AppState) -> bool {
    state.color_mode == crate::config::ColorMode::Truecolor
}

fn display_color(c: Color, truecolor: bool) -> Color {
    if truecolor {
        c
    } else {
        basic_color(c)
    }
}

fn basic_color(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => rgb_to_basic_color(r, g, b),
        Color::Indexed(n) => indexed_to_basic_color(n),
        Color::Black | Color::DarkGray => Color::White,
        Color::Gray => Color::White,
        Color::Reset => Color::Reset,
        _ => c,
    }
}

fn indexed_to_basic_color(n: u8) -> Color {
    match n {
        0 | 8 => Color::White,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::White,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White,
        _ => Color::White,
    }
}

fn rgb_to_basic_color(r: u8, g: u8, b: u8) -> Color {
    const CANDIDATES: &[(Color, (i32, i32, i32))] = &[
        (Color::Red, (205, 49, 49)),
        (Color::Green, (13, 188, 121)),
        (Color::Yellow, (229, 229, 16)),
        (Color::Blue, (36, 114, 200)),
        (Color::Magenta, (188, 63, 188)),
        (Color::Cyan, (17, 168, 205)),
        (Color::White, (229, 229, 229)),
        (Color::LightRed, (241, 76, 76)),
        (Color::LightGreen, (35, 209, 139)),
        (Color::LightYellow, (245, 245, 67)),
        (Color::LightBlue, (59, 142, 234)),
        (Color::LightMagenta, (214, 112, 214)),
        (Color::LightCyan, (41, 184, 219)),
    ];

    let r = i32::from(r);
    let g = i32::from(g);
    let b = i32::from(b);
    CANDIDATES
        .iter()
        .min_by_key(|(_, (cr, cg, cb))| {
            let dr = r - cr;
            let dg = g - cg;
            let db = b - cb;
            dr * dr + dg * dg + db * db
        })
        .map(|(color, _)| *color)
        .unwrap_or(Color::White)
}

fn muted_style(steady: bool, truecolor: bool) -> Style {
    if steady || !truecolor {
        Style::default()
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn muted_bold_style(steady: bool, truecolor: bool) -> Style {
    muted_style(steady, truecolor).add_modifier(Modifier::BOLD)
}

fn white_style(truecolor: bool) -> Style {
    Style::default().fg(display_color(Color::White, truecolor))
}

// ── Per-core grid ──

const GRID_COLS: usize = 8;
const GRID_CELLS: usize = 32;
// Braille glyph buckets for the per-core grid: index = round(pct / 12.5),
// i.e. the bucket holds every U+2800..U+28FF glyph with exactly that many
// raised dots (bucket size = C(8, dots)). Each render picks two glyphs at
// random from the core's bucket and colors the pair with the same
// bar_low..bar_high gradient the utilization bars use.
const BRAILLE: &[&[char]] = &[
    // 0 dots — 0%
    &['\u{2800}'],
    // 1 dot — 12.5%
    &[
        '\u{2801}', '\u{2802}', '\u{2804}', '\u{2808}', '\u{2810}', '\u{2820}', '\u{2840}',
        '\u{2880}',
    ],
    // 2 dots — 25%
    &[
        '\u{2803}', '\u{2805}', '\u{2806}', '\u{2809}', '\u{280A}', '\u{280C}', '\u{2811}',
        '\u{2812}', '\u{2814}', '\u{2818}', '\u{2821}', '\u{2822}', '\u{2824}', '\u{2828}',
        '\u{2830}', '\u{2841}', '\u{2842}', '\u{2844}', '\u{2848}', '\u{2850}', '\u{2860}',
        '\u{2881}', '\u{2882}', '\u{2884}', '\u{2888}', '\u{2890}', '\u{28A0}', '\u{28C0}',
    ],
    // 3 dots — 37.5%
    &[
        '\u{2807}', '\u{280B}', '\u{280D}', '\u{280E}', '\u{2813}', '\u{2815}', '\u{2816}',
        '\u{2819}', '\u{281A}', '\u{281C}', '\u{2823}', '\u{2825}', '\u{2826}', '\u{2829}',
        '\u{282A}', '\u{282C}', '\u{2831}', '\u{2832}', '\u{2834}', '\u{2838}', '\u{2843}',
        '\u{2845}', '\u{2846}', '\u{2849}', '\u{284A}', '\u{284C}', '\u{2851}', '\u{2852}',
        '\u{2854}', '\u{2858}', '\u{2861}', '\u{2862}', '\u{2864}', '\u{2868}', '\u{2870}',
        '\u{2883}', '\u{2885}', '\u{2886}', '\u{2889}', '\u{288A}', '\u{288C}', '\u{2891}',
        '\u{2892}', '\u{2894}', '\u{2898}', '\u{28A1}', '\u{28A2}', '\u{28A4}', '\u{28A8}',
        '\u{28B0}', '\u{28C1}', '\u{28C2}', '\u{28C4}', '\u{28C8}', '\u{28D0}', '\u{28E0}',
    ],
    // 4 dots — 50%
    &[
        '\u{280F}', '\u{2817}', '\u{281B}', '\u{281D}', '\u{281E}', '\u{2827}', '\u{282B}',
        '\u{282D}', '\u{282E}', '\u{2833}', '\u{2835}', '\u{2836}', '\u{2839}', '\u{283A}',
        '\u{283C}', '\u{2847}', '\u{284B}', '\u{284D}', '\u{284E}', '\u{2853}', '\u{2855}',
        '\u{2856}', '\u{2859}', '\u{285A}', '\u{285C}', '\u{2863}', '\u{2865}', '\u{2866}',
        '\u{2869}', '\u{286A}', '\u{286C}', '\u{2871}', '\u{2872}', '\u{2874}', '\u{2878}',
        '\u{2887}', '\u{288B}', '\u{288D}', '\u{288E}', '\u{2893}', '\u{2895}', '\u{2896}',
        '\u{2899}', '\u{289A}', '\u{289C}', '\u{28A3}', '\u{28A5}', '\u{28A6}', '\u{28A9}',
        '\u{28AA}', '\u{28AC}', '\u{28B1}', '\u{28B2}', '\u{28B4}', '\u{28B8}', '\u{28C3}',
        '\u{28C5}', '\u{28C6}', '\u{28C9}', '\u{28CA}', '\u{28CC}', '\u{28D1}', '\u{28D2}',
        '\u{28D4}', '\u{28D8}', '\u{28E1}', '\u{28E2}', '\u{28E4}', '\u{28E8}', '\u{28F0}',
    ],
    // 5 dots — 62.5%
    &[
        '\u{281F}', '\u{282F}', '\u{2837}', '\u{283B}', '\u{283D}', '\u{283E}', '\u{284F}',
        '\u{2857}', '\u{285B}', '\u{285D}', '\u{285E}', '\u{2867}', '\u{286B}', '\u{286D}',
        '\u{286E}', '\u{2873}', '\u{2875}', '\u{2876}', '\u{2879}', '\u{287A}', '\u{287C}',
        '\u{288F}', '\u{2897}', '\u{289B}', '\u{289D}', '\u{289E}', '\u{28A7}', '\u{28AB}',
        '\u{28AD}', '\u{28AE}', '\u{28B3}', '\u{28B5}', '\u{28B6}', '\u{28B9}', '\u{28BA}',
        '\u{28BC}', '\u{28C7}', '\u{28CB}', '\u{28CD}', '\u{28CE}', '\u{28D3}', '\u{28D5}',
        '\u{28D6}', '\u{28D9}', '\u{28DA}', '\u{28DC}', '\u{28E3}', '\u{28E5}', '\u{28E6}',
        '\u{28E9}', '\u{28EA}', '\u{28EC}', '\u{28F1}', '\u{28F2}', '\u{28F4}', '\u{28F8}',
    ],
    // 6 dots — 75%
    &[
        '\u{283F}', '\u{285F}', '\u{286F}', '\u{2877}', '\u{287B}', '\u{287D}', '\u{287E}',
        '\u{289F}', '\u{28AF}', '\u{28B7}', '\u{28BB}', '\u{28BD}', '\u{28BE}', '\u{28CF}',
        '\u{28D7}', '\u{28DB}', '\u{28DD}', '\u{28DE}', '\u{28E7}', '\u{28EB}', '\u{28ED}',
        '\u{28EE}', '\u{28F3}', '\u{28F5}', '\u{28F6}', '\u{28F9}', '\u{28FA}', '\u{28FC}',
    ],
    // 7 dots — 87.5%
    &[
        '\u{287F}', '\u{28BF}', '\u{28DF}', '\u{28EF}', '\u{28F7}', '\u{28FB}', '\u{28FD}',
        '\u{28FE}',
    ],
    // 8 dots — 100%
    &['\u{28FF}'],
];

// ASCII fallback buckets for --compat/steady rendering (same bucketing).
const BRAILLE_ASCII: &[char] = &[' ', ' ', '.', '.', ':', ':', '=', '=', '#'];

/// Bucket index (0..8): nearest multiple of 12.5%, i.e. dots raised out of 8.
fn braille_bucket(pct: f64) -> usize {
    ((pct.clamp(0.0, 100.0) / 12.5).round() as usize).min(8)
}

/// SplitMix64 finalizer: stable per (second, cell) pseudo-random pick.
fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The eight braille dot bit values, for single-dot flickers.
const DOT_BITS: [u32; 8] = [1, 2, 4, 8, 16, 32, 64, 128];

/// Two random glyphs from the core's dot-count bucket. Reseeded once per
/// wall-clock second (via `second`), so the pair is stable across frames
/// inside the same second even at sub-second refresh intervals.
/// Sub-threshold idle cores (bucket 0) instead get a single dot at any of
/// the 16 positions across the two glyphs with 20% chance per frame
/// (`idle_frame` advances every redraw), so the grid keeps a little
/// life when nothing is running.
fn core_braille_glyphs(
    pct: f64,
    second: u64,
    idle_frame: u64,
    cell: usize,
    seed: u64,
) -> String {
    let bucket = braille_bucket(pct);
    let cell_seed = seed ^ ((cell as u64) << 40);
    if bucket == 0 {
        let fk = idle_frame.wrapping_mul(0xD6E8_FEB8_6659_FD93);
        if mix64(cell_seed ^ fk ^ 0xB105) % 5 != 0 {
            return "\u{2800}\u{2800}".to_string();
        }
        let pos = mix64(cell_seed ^ fk ^ 0x51D1) as usize % 16; // 16 dot slots: 8 per glyph
        let glyph = 0x2800 + DOT_BITS[pos % 8];
        let (left, right) = if pos < 8 { (glyph, 0x2800) } else { (0x2800, glyph) };
        let ch = |v: u32| char::from_u32(v).unwrap_or('\u{2800}');
        return format!("{}{}", ch(left), ch(right));
    }
    let glyphs = BRAILLE[bucket];
    let pick = |k: u64| glyphs[mix64(cell_seed ^ (second << 20) ^ k) as usize % glyphs.len()];
    format!("{}{}", pick(1), pick(2))
}

/// ASCII fallback pair for --compat/steady rendering: the bucket char plus
/// the bucket below it (blank at 0%), mirroring the two-glyph braille cell.
fn core_ascii_glyphs(pct: f64) -> String {
    let bucket = braille_bucket(pct);
    let lo = if bucket == 0 { ' ' } else { BRAILLE_ASCII[bucket - 1] };
    format!("{}{}", BRAILLE_ASCII[bucket], lo)
}

/// Collapse per-core usage into grid rows (row-major, max 8 columns).
/// When there are more cores than `GRID_CELLS`, cores are grouped and
/// each cell shows the hottest core of its group.
fn core_grid_cells(pcts: &[f64]) -> Vec<Vec<f64>> {
    if pcts.is_empty() {
        return Vec::new();
    }
    let group = (pcts.len() + GRID_CELLS - 1) / GRID_CELLS;
    pcts
        .chunks(group)
        .map(|g| g.iter().cloned().fold(0f64, f64::max))
        .collect::<Vec<f64>>()
        .chunks(GRID_COLS)
        .map(|r| r.to_vec())
        .collect()
}

fn empty_bar_span(width: usize, steady: bool, truecolor: bool) -> Span<'static> {
    if steady || !truecolor {
        Span::raw(" ".repeat(width))
    } else {
        Span::styled("░".repeat(width), Style::default().fg(Color::DarkGray))
    }
}

fn bar_spans(
    pct: f64,
    width: usize,
    theme: &Theme,
    steady: bool,
    truecolor: bool,
) -> Vec<Span<'static>> {
    let pct = pct.clamp(0.0, 100.0);
    let filled = (pct / 100.0 * width as f64) as usize;
    let empty = width.saturating_sub(filled);
    let mut spans = Vec::new();
    if steady {
        if filled > 0 {
            spans.push(Span::styled(
                "█".repeat(filled),
                Style::default().fg(display_color(color_for_pct(pct, theme), truecolor)),
            ));
        }
        if empty > 0 {
            spans.push(empty_bar_span(empty, true, truecolor));
        }
        return spans;
    }

    for i in 0..filled {
        let t = i as f64 / (width.max(1) - 1) as f64;
        let c = lerp_color(theme.bar_low, theme.bar_high, t);
        spans.push(Span::styled(
            "█",
            Style::default().fg(display_color(c, truecolor)),
        ));
    }
    if empty > 0 {
        spans.push(empty_bar_span(empty, steady, truecolor));
    }
    spans
}

fn color_for_pct(pct: f64, theme: &Theme) -> Color {
    if pct < 50.0 {
        theme.bar_low
    } else if pct < 80.0 {
        theme.bar_mid
    } else {
        theme.bar_high
    }
}

fn fmt_bytes(b: f64) -> String {
    let mb = b / (1024.0 * 1024.0);
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{:.1} MB", mb)
    }
}

fn fmt_speed(b: f64) -> String {
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB/s", b / (1024.0 * 1024.0 * 1024.0))
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.1} KB/s", b / 1024.0)
    } else {
        format!("{:.0} B/s", b)
    }
}

// ── Helpers ──

fn styled_block<'a>(title: &str, color: Color, truecolor: bool) -> Block<'a> {
    let color = display_color(color, truecolor);
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Center)
}

fn with_margin(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    })
}

// ── Main render function ──

pub fn render(f: &mut Frame, state: &AppState) {
    if state.picking_theme {
        render_theme_picker(f, state);
        return;
    }

    let area = f.area();

    // Main vertical layout: GPU | mid(net+cpu+mem) | temps | procs | status
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(13), // GPU
            Constraint::Length(10), // mid row: net/cpu/mem are exactly 8 content lines
            Constraint::Length(3), // temps
            Constraint::Length(((state.disks.len() + 2) / 3).max(1) as u16 + 2), // disks
            Constraint::Min(5),    // procs: absorbs slack, shrinks first to a 2-row floor
            Constraint::Length(1), // status bar
        ])
        .split(area);

    render_gpu(f, with_margin(chunks[0]), state);
    render_mid_row(f, with_margin(chunks[1]), state);
    render_temps(f, with_margin(chunks[2]), state);
    render_disks(f, with_margin(chunks[3]), state);
    render_procs(f, with_margin(chunks[4]), state);
    render_status_bar(f, with_margin(chunks[5]), state);
    if state.show_stats {
        render_stats(f, state);
    }
}

fn render_gpu(f: &mut Frame, area: Rect, state: &AppState) {
    let theme = &state.theme;
    let truecolor = truecolor_enabled(state);
    let gpus = &state.gpu_infos;

    if gpus.is_empty() {
        let block = styled_block("GPU", theme.gpu, truecolor);
        let text = Paragraph::new(
            "No GPUs detected (install NVIDIA drivers for NVIDIA, load amdgpu for AMD, or xe/i915 for Intel Arc)",
        )
        .style(muted_style(state.steady_render, truecolor))
        .block(block);
        f.render_widget(text, area);
        return;
    }

    let constraints: Vec<Constraint> = gpus
        .iter()
        .map(|_| Constraint::Ratio(1, gpus.len() as u32))
        .collect();
    let gpu_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    for (i, g) in gpus.iter().enumerate() {
        let inner_w = gpu_chunks[i].width.saturating_sub(4) as usize;
        let bar_w = inner_w.saturating_sub(14).max(5);
        let spark_w = inner_w.saturating_sub(6).max(10);

        let uc = color_for_pct(g.util, theme);
        let mc = color_for_pct(g.mem_pct, theme);

        let util_bar = bar_spans(g.util, bar_w, theme, state.steady_render, truecolor);
        let mem_bar = bar_spans(g.mem_pct, bar_w, theme, state.steady_render, truecolor);

        let spark_u = if state.steady_render {
            String::new()
        } else {
            state
                .gpu_util_hist
                .get(&g.id)
                .map(|h| sparkline(h, spark_w, None))
                .unwrap_or_default()
        };
        let spark_m = if state.steady_render {
            String::new()
        } else {
            state
                .gpu_mem_hist
                .get(&g.id)
                .map(|h| sparkline(h, spark_w, None))
                .unwrap_or_default()
        };

        let mut lines = Vec::new();

        // Util line
        let mut util_line = vec![Span::styled(
            "Util ",
            Style::default().add_modifier(Modifier::BOLD),
        )];
        if g.has_util {
            util_line.extend(util_bar);
            util_line.push(Span::styled(
                format!(" {:5.1}%", g.util),
                Style::default().fg(display_color(uc, truecolor)),
            ));
        } else {
            util_line.push(Span::styled(
                "n/a",
                muted_style(state.steady_render, truecolor),
            ));
        }
        lines.push(Line::from(util_line));

        // Util sparkline
        if g.has_util {
            lines.push(Line::from(Span::styled(
                format!("     {}", spark_u),
                Style::default().fg(display_color(uc, truecolor)),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "     n/a",
                muted_style(state.steady_render, truecolor),
            )));
        }
        lines.push(Line::from(""));

        // Mem line
        let mut mem_line = vec![Span::styled(
            "Mem  ",
            Style::default().add_modifier(Modifier::BOLD),
        )];
        if g.has_mem {
            mem_line.extend(mem_bar);
            mem_line.push(Span::styled(
                format!(" {:5.1}%", g.mem_pct),
                Style::default().fg(display_color(mc, truecolor)),
            ));
        } else {
            mem_line.push(Span::styled("n/a", muted_style(state.steady_render, truecolor)));
        }
        lines.push(Line::from(mem_line));

        // Mem info
        if g.has_mem {
            lines.push(Line::from(format!(
                "     {:.1}/{:.1} GB",
                g.mem_used_gb, g.mem_total_gb
            )));
        } else if g.mem_total_gb > 0.0 {
            lines.push(Line::from(Span::styled(
                format!("     n/a / {:.1} GB", g.mem_total_gb),
                muted_style(state.steady_render, truecolor),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "     n/a",
                muted_style(state.steady_render, truecolor),
            )));
        }

        // Mem sparkline
        if g.has_mem {
            lines.push(Line::from(Span::styled(
                format!("     {}", spark_m),
                Style::default().fg(display_color(mc, truecolor)),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "     n/a",
                muted_style(state.steady_render, truecolor),
            )));
        }

        lines.push(Line::from(""));

        // Pwr: live watts bar + history sparkline (per-GPU, grouped with
        // Util/Mem in the same column).
        let watts = state.gpu_power.get(i).copied().flatten();
        let phist = state.gpu_power_hist.get(&g.id);
        // Scale the bar to the card's power limit when the driver exposes
        // one; fallback: observed peak with a 100 W floor.
        let pscale = g.power_limit_w.unwrap_or_else(|| {
            phist
                .map(|h| h.iter().cloned().fold(0f64, f64::max))
                .unwrap_or(0.0)
        }).max(100.0);
        let ppct = watts
            .map(|w| (w / pscale * 100.0).clamp(0.0, 100.0))
            .unwrap_or(0.0);
        let pc = color_for_pct(ppct, theme);
        let pw = if g.power_limit_w.is_some() {
            bar_w.saturating_sub(6).max(5)
        } else {
            bar_w
        };
        let mut pwr_line = vec![Span::styled(
            "Pwr  ",
            Style::default().add_modifier(Modifier::BOLD),
        )];
        if let Some(w) = watts {
            pwr_line.extend(bar_spans(ppct, pw, theme, state.steady_render, truecolor));
            pwr_line.push(Span::styled(
                match g.power_limit_w {
                    Some(lim) => format!(" {:5.1}/{:4.0} W", w, lim),
                    None => format!(" {:6.1} W", w),
                },
                Style::default().fg(display_color(pc, truecolor)),
            ));
            lines.push(Line::from(pwr_line));
            let pspark = if state.steady_render {
                String::new()
            } else {
                phist
                    .map(|h| sparkline(h, spark_w, Some(pscale)))
                    .unwrap_or_default()
            };
            lines.push(Line::from(Span::styled(
                format!("     {}", pspark),
                Style::default().fg(display_color(pc, truecolor)),
            )));
        } else {
            pwr_line.push(Span::styled("n/a", muted_style(state.steady_render, truecolor)));
            lines.push(Line::from(pwr_line));
            lines.push(Line::from(Span::styled(
                "     n/a",
                muted_style(state.steady_render, truecolor),
            )));
        }
        lines.push(Line::from(""));

        // PCIe link: live trained speed/width (sysfs, same source as
        // nvtop); device max shown only when running below it.
        if let Some(p) = &g.pci {
            let cg = pci_gen(&p.cur_speed);
            let mg = pci_gen(&p.max_speed);
            let mut txt = format!("PCIe {} x{}", cg, p.cur_width);
            if p.cur_width < p.max_width || cg != mg {
                txt.push_str(&format!(" (max {} x{})", mg, p.max_width));
            }
            if let (Some(rx), Some(tx)) = (g.pcie_rx_kbs, g.pcie_tx_kbs) {
                txt.push_str(&format!(
                    "  ↓{:5.1} ↑{:5.1} MB/s",
                    rx / 1024.0,
                    tx / 1024.0
                ));
            }
            lines.push(Line::from(Span::styled(txt, Style::default())));
        }

        let name_short = gpu_name_short(&g.name);

        let block = styled_block(&format!("GPU {}", g.id), theme.gpu, truecolor).title_bottom(
            Line::from(Span::styled(
                format!(" {} ", name_short),
                Style::default().fg(display_color(theme.gpu, truecolor)),
            )),
        );

        let paragraph = Paragraph::new(lines).block(block);
        f.render_widget(paragraph, gpu_chunks[i]);
    }
}

fn gpu_name_short(name: &str) -> String {
    name.replace("Advanced Micro Devices, Inc. ", "AMD ")
        .replace(" Generation", "")
}

// "16.0 GT/s PCIe" -> "G4"
fn pci_gen(speed: &str) -> String {
    let v: f64 = speed
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let g = if v >= 64.0 {
        6
    } else if v >= 32.0 {
        5
    } else if v >= 16.0 {
        4
    } else if v >= 8.0 {
        3
    } else if v >= 5.0 {
        2
    } else {
        1
    };
    format!("G{}", g)
}


fn render_mid_row(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(area);

    render_net(f, chunks[0], state);
    render_cpu(f, chunks[1], state);
    render_mem(f, chunks[2], state);
}

fn render_cpu(f: &mut Frame, area: Rect, state: &AppState) {
    let theme = &state.theme;
    let truecolor = truecolor_enabled(state);
    let pct = state.cpu_pct;
    let c = color_for_pct(pct, theme);
    let inner_w = area.width.saturating_sub(4) as usize;
    let bar_w = inner_w.saturating_sub(18).max(5);
    let spark_w = inner_w.saturating_sub(10).max(10);

    let cpu_bar = bar_spans(pct, bar_w, theme, state.steady_render, truecolor);
    let (spark_top, spark_bot) = if state.steady_render {
        (String::new(), String::new())
    } else {
        sparkline_double(&state.cpu_hist, spark_w)
    };

    let mut lines = Vec::new();
    let mut overall = vec![Span::styled(
        "Overall  ",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    overall.extend(cpu_bar);
    overall.push(Span::styled(
        format!(" {:5.1}%", pct),
        Style::default().fg(display_color(c, truecolor)),
    ));
    lines.push(Line::from(overall));

    lines.push(Line::from(Span::styled(
        format!("         {}", spark_top),
        Style::default().fg(display_color(c, truecolor)),
    )));
    lines.push(Line::from(Span::styled(
        format!("         {}", spark_bot),
        Style::default().fg(display_color(c, truecolor)),
    )));
    lines.push(Line::from(""));
    // Per-core braille grid: two random glyphs per core from the dot-count
    // bucket matching the core's usage (8 columns, max 4 rows), colored with
    // the smooth bar gradient, with cores/total/freq/power readouts down the
    // right side. The whole block is centered when the panel is wide enough
    // for it; otherwise the grid is clipped column-by-column so the readouts
    // still show.
    let grid = core_grid_cells(&state.cpu_core_pcts);
    if !grid.is_empty() {
        // Glyph pairs are re-rolled once per wall-clock second, staying
        // stable across every frame drawn inside the same second.
        let glyph_second = state.render_base.elapsed().as_secs();
        // Monotonic draw-frame counter for the idle-core dot flicker.
        let idle_frame = (state.render_base.elapsed().as_secs_f64() / state.refresh_secs.max(0.01)) as u64;
        let mut cell_idx = 0usize;
        let (cores_sym, total_sym, freq_sym, power_sym) = if state.steady_render {
            ("C", "T", "F", "P")
        } else {
            ("\u{229E}", "\u{3A3}", "\u{192}", "\u{2301}")
        };
        let info = [
            format!("{cores_sym}: {}", state.cpu_cores),
            format!("{total_sym}: {:.0}%", state.cpu_total_pct),
            format!("{freq_sym}: {}", state.cpu_freq),
            match state.cpu_power_watts {
                Some(w) => format!("{power_sym}: {:.0} W", w),
                None if state.cpu_power_denied => format!("{power_sym}: SU req"),
                None => format!("{power_sym}: n/a"),
            },
        ];
        let label_w = info.iter().map(|s| s.chars().count()).max().unwrap_or(0);
        let inner_w = area.width.saturating_sub(2) as usize;
        let block_w = GRID_COLS * 2 + 2 + label_w;
        let pad = inner_w.saturating_sub(block_w) / 2;
        let shown_w = if pad > 0 {
            GRID_COLS * 2
        } else {
            let avail = inner_w.saturating_sub(2 + label_w);
            (avail / 2).min(GRID_COLS) * 2
        };
        let max_cells = shown_w / 2;
        for i in 0..4 {
            let mut spans = Vec::new();
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
            }
            let mut grid_w = 0usize;
            if let Some(row) = grid.get(i) {
                for v in row.iter().take(max_cells) {
                    let pair = if state.steady_render {
                        core_ascii_glyphs(*v)
                    } else {
                        core_braille_glyphs(*v, glyph_second, idle_frame, cell_idx, state.rand_base)
                    };
                    spans.push(Span::styled(
                        pair,
                        Style::default()
                            .fg(display_color(lerp_color(theme.bar_low, theme.bar_high, v / 100.0), truecolor)),
                    ));
                    grid_w += 2;
                    cell_idx += 1;
                }
            }
            spans.push(Span::raw(" ".repeat(shown_w - grid_w + 2)));
            spans.push(Span::styled(info[i].clone(), white_style(truecolor)));
            lines.push(Line::from(spans));
        }
    }

    let block =
        styled_block("CPU", theme.cpu, truecolor).title_bottom(Line::from(Span::styled(
            format!(" {} ", state.cpu_name),
            Style::default().fg(display_color(theme.cpu, truecolor)),
        )));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_net(f: &mut Frame, area: Rect, state: &AppState) {
    let theme = &state.theme;
    let truecolor = truecolor_enabled(state);
    let (up, down) = (state.net_up, state.net_down);
    let mx = state.net_max_speed;
    let up_pct = if mx > 0.0 {
        (up / mx * 100.0).min(100.0)
    } else {
        0.0
    };
    let down_pct = if mx > 0.0 {
        (down / mx * 100.0).min(100.0)
    } else {
        0.0
    };

    let inner_w = area.width.saturating_sub(4) as usize;
    let bar_w = inner_w.saturating_sub(18).max(5);
    let spark_w = inner_w.saturating_sub(6).max(10);

    let up_bar = bar_spans(up_pct, bar_w, theme, state.steady_render, truecolor);
    let down_bar = bar_spans(down_pct, bar_w, theme, state.steady_render, truecolor);

    let (spark_up_str, spark_dn_str) = if state.steady_render {
        (String::new(), String::new())
    } else {
        (
            sparkline(&state.net_up_hist, spark_w, Some(mx)),
            sparkline_down(&state.net_down_hist, spark_w, Some(mx)),
        )
    };

    let mut lines = Vec::new();

    let mut up_line = vec![Span::styled(
        "Up   ",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    up_line.extend(up_bar);
    up_line.push(Span::styled(
        format!(" {:>10}", fmt_speed(up)),
        Style::default().fg(display_color(theme.net_up, truecolor)),
    ));
    lines.push(Line::from(up_line));
    lines.push(Line::from(Span::styled(
        format!("Peak Up: {}", fmt_speed(state.net_max_up)),
        white_style(truecolor),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("     {}", spark_up_str),
        Style::default().fg(display_color(theme.net_up, truecolor)),
    )));
    lines.push(Line::from(Span::styled(
        format!("     {}", spark_dn_str),
        Style::default().fg(display_color(theme.net_down, truecolor)),
    )));
    lines.push(Line::from(""));

    let mut down_line = vec![Span::styled(
        "Down ",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    down_line.extend(down_bar);
    down_line.push(Span::styled(
        format!(" {:>10}", fmt_speed(down)),
        Style::default().fg(display_color(theme.net_down, truecolor)),
    ));
    lines.push(Line::from(down_line));
    lines.push(Line::from(Span::styled(
        format!("Peak Down: {}", fmt_speed(state.net_max_down)),
        white_style(truecolor),
    )));

    let block = styled_block("Network", theme.net, truecolor);

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_mem(f: &mut Frame, area: Rect, state: &AppState) {
    let theme = &state.theme;
    let truecolor = truecolor_enabled(state);
    let inner_w = area.width.saturating_sub(4) as usize;
    let bar_w = inner_w.saturating_sub(14).max(5);

    let ram_pct = state.ram_pct;
    let rc = color_for_pct(ram_pct, theme);
    let ram_bar = bar_spans(ram_pct, bar_w, theme, state.steady_render, truecolor);

    let swap_pct = state.swap_pct;
    let swap_bar = bar_spans(swap_pct, bar_w, theme, state.steady_render, truecolor);

    let mut lines = Vec::new();
    let mut ram_line = vec![Span::styled(
        "RAM  ",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    ram_line.extend(ram_bar);
    ram_line.push(Span::styled(
        format!(" {:5.1}%", ram_pct),
        Style::default().fg(display_color(rc, truecolor)),
    ));
    lines.push(Line::from(ram_line));
    lines.push(Line::from(format!(
        "  {} used / {} total",
        fmt_bytes(state.ram_used as f64),
        fmt_bytes(state.ram_total as f64)
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "  {} avail / {} cached",
            fmt_bytes(state.ram_available as f64),
            fmt_bytes(state.ram_cached as f64)
        ),
        white_style(truecolor),
    )));
    if let Some((committed, limit)) = state.mem_committed {
        lines.push(Line::from(Span::styled(
            format!(
                "  {} comitd / {} limit",
                fmt_bytes(committed as f64),
                fmt_bytes(limit as f64)
            ),
            white_style(truecolor),
        )));
    }
    if let Some((p10, p60, p300)) = state.mem_psi_some {
        lines.push(Line::from(Span::styled(
            format!("  PSI some {:.1}/{:.1}/{:.1}%", p10, p60, p300),
            white_style(truecolor),
        )));
    }

    let mut swap_line = vec![Span::styled(
        "Swap ",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    swap_line.extend(swap_bar);
    swap_line.push(Span::styled(
        format!(" {:5.1}%", swap_pct),
        Style::default().fg(display_color(color_for_pct(swap_pct, theme), truecolor)),
    ));
    lines.push(Line::from(swap_line));
    lines.push(Line::from(Span::styled(
        format!(
            "  {} used / {} total",
            fmt_bytes(state.swap_used as f64),
            fmt_bytes(state.swap_total as f64)
        ),
        white_style(truecolor),
    )));
    let mut swap_io_line = Vec::new();
    swap_io_line.push(Span::styled(
        format!("  R {:>9}", fmt_speed(state.swap_io_read)),
        Style::default().fg(display_color(theme.net_down, truecolor)),
    ));
    swap_io_line.push(Span::styled(
        format!("  W {:>9}", fmt_speed(state.swap_io_write)),
        Style::default().fg(display_color(theme.net_up, truecolor)),
    ));
    lines.push(Line::from(swap_io_line));

    let block = styled_block("Memory", theme.mem, truecolor);

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_temps(f: &mut Frame, area: Rect, state: &AppState) {
    let theme = &state.theme;
    let truecolor = truecolor_enabled(state);
    let mut cells: Vec<Vec<Span>> = Vec::new();

    if let Some(cpu_t) = state.cpu_temp {
        cells.push(temp_cell(
            "CPU",
            cpu_t,
            state.cpu_temp_max,
            theme,
            state.steady_render,
            truecolor,
        ));
    }
    if let Some(mem_t) = state.mem_temp {
        cells.push(temp_cell(
            "MEM",
            mem_t,
            state.mem_temp_max,
            theme,
            state.steady_render,
            truecolor,
        ));
    }
    for (i, gt) in state.gpu_temps.iter().enumerate() {
        if let Some(t) = gt {
            cells.push(temp_cell(
                &format!("GPU{}", i),
                t.temp,
                t.max,
                theme,
                state.steady_render,
                truecolor,
            ));
        } else {
            cells.push(temp_cell_na(
                &format!("GPU{}", i),
                state.steady_render,
                truecolor,
            ));
        }
    }

    if cells.is_empty() {
        let block = styled_block("Temps", theme.bar_mid, truecolor);
        f.render_widget(
            Paragraph::new("No temperature data")
                .style(muted_style(state.steady_render, truecolor))
                .block(block),
            area,
        );
        return;
    }

    let ncols = cells.len();

    // We render as a single panel
    let block = styled_block("Temps", theme.bar_mid, truecolor);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let constraints: Vec<Constraint> = (0..ncols)
        .map(|_| Constraint::Ratio(1, ncols as u32))
        .collect();
    let temp_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(inner);

    for (i, cell_spans) in cells.iter().enumerate() {
        if i < temp_cols.len() {
            let line = Line::from(cell_spans.clone());
            f.render_widget(Paragraph::new(vec![line]), temp_cols[i]);
        }
    }
}

fn render_disks(f: &mut Frame, area: Rect, state: &AppState) {
    let theme = &state.theme;
    let truecolor = truecolor_enabled(state);
    let block = styled_block("Disks", theme.bar_mid, truecolor);

    if state.disks.is_empty() {
        f.render_widget(
            Paragraph::new("No disks detected")
                .style(muted_style(state.steady_render, truecolor))
                .block(block),
            area,
        );
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Three columns of disks per row, NVMe first (sorted in the tracker).
    let cell_w = (inner.width as usize) / 3;
    let lines: Vec<Line> = state
        .disks
        .chunks(3)
        .map(|row| {
            let mut spans = Vec::new();
            for d in row {
                let mut cell = vec![Span::styled(
                    format!("{:<10}", d.name),
                    Style::default().add_modifier(Modifier::BOLD),
                )];
                cell.push(Span::styled(
                    format!("R {:>9}  ", fmt_speed(d.read_bps)),
                    Style::default().fg(display_color(theme.net_down, truecolor)),
                ));
                cell.push(Span::styled(
                    format!("W {:>9}", fmt_speed(d.write_bps)),
                    Style::default().fg(display_color(theme.net_up, truecolor)),
                ));
                let used: usize = cell.iter().map(|s| s.content.chars().count()).sum();
                if cell_w > used {
                    cell.push(Span::raw(" ".repeat(cell_w - used)));
                }
                spans.extend(cell);
            }
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn temp_cell(
    label: &str,
    temp: f64,
    max: f64,
    theme: &Theme,
    steady: bool,
    truecolor: bool,
) -> Vec<Span<'static>> {
    let pct = (temp / max * 100.0).min(100.0);
    let ratio = temp / max;
    let c = if ratio < 0.6 {
        theme.bar_low
    } else if ratio < 0.85 {
        theme.bar_mid
    } else {
        theme.bar_high
    };
    let filled = (pct / 100.0 * 8.0) as usize;
    let c = display_color(c, truecolor);
    vec![
        Span::styled(format!("{} ", label), muted_bold_style(steady, truecolor)),
        Span::styled("█".repeat(filled), Style::default().fg(c)),
        empty_bar_span(8 - filled, steady, truecolor),
        Span::styled(format!(" {:.0}/{:.0}°C", temp, max), Style::default().fg(c)),
    ]
}

fn temp_cell_na(label: &str, steady: bool, truecolor: bool) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!("{} ", label), muted_bold_style(steady, truecolor)),
        Span::styled("N/A", muted_style(steady, truecolor)),
    ]
}

fn render_procs(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(area);

    render_proc_table(f, chunks[0], state, true);
    render_proc_table(f, chunks[1], state, false);
}

fn render_proc_table(f: &mut Frame, area: Rect, state: &AppState, by_mem: bool) {
    let theme = &state.theme;
    let truecolor = truecolor_enabled(state);
    let procs = if by_mem {
        &state.procs_by_mem
    } else {
        &state.procs_by_cpu
    };
    let colour = if by_mem {
        theme.proc_mem
    } else {
        theme.proc_cpu
    };
    let title = if by_mem {
        " Top Processes by Memory "
    } else {
        " Top Processes by CPU "
    };

    let text_style = Style::default();
    let bold = text_style.add_modifier(Modifier::BOLD);
    let header = if by_mem {
        Row::new(vec![
            Cell::from("PID").style(muted_bold_style(state.steady_render, truecolor)),
            Cell::from("Name").style(bold),
            Cell::from("Used").style(bold),
            Cell::from("Shared").style(bold),
            Cell::from("Mem %").style(bold),
        ])
        .style(text_style)
    } else {
        Row::new(vec![
            Cell::from("PID").style(muted_bold_style(state.steady_render, truecolor)),
            Cell::from("Name").style(bold),
            Cell::from("Core %").style(bold),
            Cell::from("CPU %").style(bold),
            Cell::from("Mem %").style(bold),
        ])
        .style(text_style)
    };

    let rows: Vec<Row> = procs
        .iter()
        .map(|p| {
            if by_mem {
                let shared = p.shared;
                let used = if p.rss > shared { p.rss - shared } else { 0 };
                Row::new(vec![
                    Cell::from(format!("{}", p.pid))
                        .style(muted_style(state.steady_render, truecolor)),
                    Cell::from(p.name.clone()),
                    Cell::from(fmt_bytes(used as f64)),
                    Cell::from(fmt_bytes(shared as f64)),
                    Cell::from(format!("{:.1}%", p.memory_percent)),
                ])
                .style(text_style)
            } else {
                let sys_pct = p.cpu_percent / state.num_cpus as f64;
                Row::new(vec![
                    Cell::from(format!("{}", p.pid))
                        .style(muted_style(state.steady_render, truecolor)),
                    Cell::from(p.name.clone()),
                    Cell::from(format!("{:.1}%", p.cpu_percent)),
                    Cell::from(format!("{:.1}%", sys_pct)),
                    Cell::from(format!("{:.1}%", p.memory_percent)),
                ])
                .style(text_style)
            }
        })
        .collect();

    let widths = if by_mem {
        vec![
            Constraint::Length(8),
            Constraint::Min(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(7),
        ]
    } else {
        vec![
            Constraint::Length(8),
            Constraint::Min(10),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(7),
        ]
    };

    let table = Table::new(rows, widths)
        .header(header)
        .style(text_style)
        .block(styled_block(title.trim(), colour, truecolor));

    f.render_widget(table, area);
}

fn render_status_bar(f: &mut Frame, area: Rect, state: &AppState) {
    let theme = &state.theme;
    let truecolor = truecolor_enabled(state);
    let color_text = format!(" {}", state.color_mode.label());
    let left_spans = vec![
        Span::styled(
            " q",
            Style::default()
                .fg(display_color(theme.cpu, truecolor))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("/", muted_style(state.steady_render, truecolor)),
        Span::styled(
            "ESC",
            Style::default()
                .fg(display_color(theme.cpu, truecolor))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Quit  ", muted_style(state.steady_render, truecolor)),
        Span::styled(
            " t",
            Style::default()
                .fg(display_color(theme.gpu, truecolor))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Theme  ", muted_style(state.steady_render, truecolor)),
        Span::styled(
            " s",
            Style::default()
                .fg(display_color(theme.proc_cpu, truecolor))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Stats  ", muted_style(state.steady_render, truecolor)),
        Span::styled(
            " c",
            Style::default()
                .fg(display_color(theme.net, truecolor))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Color:", muted_style(state.steady_render, truecolor)),
        Span::styled(
            color_text,
            Style::default()
                .fg(display_color(theme.net, truecolor))
                .add_modifier(Modifier::BOLD),
        ),
    ];

    let power_text = match state.est_power_watts {
        Some(power) => format!("PWR ~{}W  ", power.round() as u64),
        None => "PWR n/a  ".to_string(),
    };
    let oom_text = if let Some(ref oom) = state.oom_str {
        format!("█ OOM Kill: {}", oom)
    } else if state.steady_render {
        "  No OOM kills".to_string()
    } else {
        "░ No OOM kills".to_string()
    };
    // Version stamp on the right, before PWR, separated by a vertical bar —
    // always visible. Once (and only if) the background check confirms a
    // newer release, `| Update available ` pops in between the version and
    // the separator. The styled spans below must reproduce `update_text`
    // exactly — it sizes the segment.
    let update_text = {
        let mut text = format!("v{} ", env!("CARGO_PKG_VERSION"));
        if state.update_available {
            text.push_str("| Update available ");
        }
        text.push_str("│ ");
        text
    };
    let right_len = power_text.len() + oom_text.len() + update_text.len();
    let right_width = right_len.min(area.width as usize) as u16;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(right_width)])
        .split(area);

    f.render_widget(Paragraph::new(Line::from(left_spans)), chunks[0]);

    // Version stamp always visible ahead of PWR; the flag spans appear only
    // once the background check confirmed a newer release.
    let mut right_spans = Vec::new();
    right_spans.push(Span::styled(
        format!("v{} ", env!("CARGO_PKG_VERSION")),
        muted_style(state.steady_render, truecolor),
    ));
    if state.update_available {
        right_spans.push(Span::styled(
            "| ",
            muted_style(state.steady_render, truecolor),
        ));
        right_spans.push(Span::styled(
            "Update available ",
            Style::default()
                .fg(display_color(theme.bar_high, truecolor))
                .add_modifier(Modifier::BOLD),
        ));
    }
    right_spans.push(Span::styled(
        "│ ",
        muted_style(state.steady_render, truecolor),
    ));
    match state.est_power_watts {
        Some(power) => right_spans.push(Span::styled(
            format!("PWR ~{}W  ", power.round() as u64),
            Style::default()
                .fg(display_color(theme.mem, truecolor))
                .add_modifier(Modifier::BOLD),
        )),
        None => right_spans.push(Span::styled(
            "PWR n/a  ",
            muted_style(state.steady_render, truecolor),
        )),
    }
    if state.oom_str.is_some() {
        right_spans.push(Span::styled(
            oom_text,
            Style::default()
                .fg(display_color(theme.bar_high, truecolor))
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        right_spans.push(Span::styled(
            oom_text,
            muted_style(state.steady_render, truecolor),
        ));
    }

    f.render_widget(
        Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right),
        chunks[1],
    );
}

// ── Performance stats ──

/// Microseconds spelled the way the diagnostics table reads them.
fn format_us(us: f64) -> String {
    if us >= 1000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{:.1} us", us)
    }
}

/// Diagnostic page for the sampling loop itself (`s`): per-tick last/avg/max
/// wall duration of every block of the refresh tick, and the process's own
/// CPU time per tick in the header — what a %CPU column would show for the
/// same window. The profiler runs always, so these are retrospective —
/// exactly what the loop cost before the page was opened. Wall durations
/// exceed CPU time when the loop waits in a device driver (GPU queries);
/// both are true statements about different things, so both are shown.
fn render_stats(f: &mut Frame, state: &AppState) {
    let theme = &state.theme;
    let truecolor = truecolor_enabled(state);
    let area = f.area();
    let samples = state.prof.samples();
    // The header carries the per-tick totals; the aggregate `gpu` row is
    // replaced by per-card rows so cost is attributable to a GPU.
    let mut rows: Vec<(String, ProfStats)> = PROF_BLOCKS
        .iter()
        .filter(|b| {
            !matches!(
                **b,
                ProfBlock::Total | ProfBlock::CpuTime | ProfBlock::Gpu
            )
        })
        .map(|b| (b.label().to_string(), state.prof.stats(*b)))
        .collect();
    for i in 0..state.prof.gpu_count() {
        rows.push((format!("gpu{}", i), state.prof.stats_gpu(i)));
    }
    // Hot blocks first; the per-tick total lives in the header line.
    rows.sort_by(|a, b| b.1.avg_us.total_cmp(&a.1.avg_us));

    let total = state.prof.stats(ProfBlock::Total);
    let cpu = state.prof.stats(ProfBlock::CpuTime);
    let median_us = state.prof.median(ProfBlock::Total);
    // Process CPU time over the same window the process-accounting tools
    // would use — this is the number that should match a %CPU column.
    let core_pct = if samples > 0 {
        cpu.avg_us / (state.refresh_secs.max(0.01) * 1.0e6) * 100.0
    } else {
        0.0
    };

    let popup_w = 76u16.min(area.width.saturating_sub(2));
    let popup_h = (rows.len() as u16 + 8).min(area.height);
    let popup = Rect::new(
        area.x + (area.width - popup_w) / 2,
        area.y + (area.height - popup_h) / 2,
        popup_w,
        popup_h,
    );
    Clear.render(popup, f.buffer_mut());
    let block = styled_block("Performance", theme.bar_mid, truecolor);

    let mut lines: Vec<Line> = Vec::new();
    if samples == 0 {
        lines.push(Line::from(Span::styled(
            " collecting first tick...",
            muted_style(state.steady_render, truecolor),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!(
                " last {} ticks: cpu {}/tick = {:.2}% of one core @ {}s",
                samples,
                format_us(cpu.avg_us),
                core_pct,
                state.refresh_secs
            ),
            white_style(truecolor),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                " wall median {} · avg {} · worst {} · rows are wall",
                format_us(median_us),
                format_us(total.avg_us),
                format_us(total.max_us)
            ),
            muted_style(state.steady_render, truecolor),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("{:<14}{:>14}{:>14}{:>14}", "block", "last", "avg/tick", "max"),
            muted_bold_style(state.steady_render, truecolor),
        )));
        for (b, s) in rows {
            // A block eating more than half the tick is worth noticing.
            let hot = total.avg_us > 0.0 && s.avg_us > 0.5 * total.avg_us;
            let value_style = if hot {
                Style::default()
                    .fg(display_color(theme.bar_high, truecolor))
                    .add_modifier(Modifier::BOLD)
            } else {
                white_style(truecolor)
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<14}", b),
                    muted_style(state.steady_render, truecolor),
                ),
                Span::styled(format!("{:>14}", format_us(s.last_us)), value_style),
                Span::styled(format!("{:>14}", format_us(s.avg_us)), value_style),
                Span::styled(format!("{:>14}", format_us(s.max_us)), value_style),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " s/Esc Close ",
        muted_style(state.steady_render, truecolor),
    )));

    f.render_widget(Paragraph::new(lines).block(block), popup);
}

// ── Theme picker ──

fn render_theme_picker(f: &mut Frame, state: &AppState) {
    let area = f.area();
    let names = crate::theme::theme_names();
    let truecolor = truecolor_enabled(state);
    let cols = 3usize;
    let visible_rows = area.height.saturating_sub(8) as usize;
    let total = names.len();
    let cursor = state.theme_cursor;

    let cursor_row = cursor / cols;
    let mut scroll = state.theme_scroll;
    if cursor_row < scroll {
        scroll = cursor_row;
    } else if cursor_row >= scroll + visible_rows {
        scroll = cursor_row.saturating_sub(visible_rows - 1);
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(area);

    // Theme list
    let total_rows = (total + cols - 1) / cols;
    let mut rows = Vec::new();
    for row_idx in scroll..total_rows.min(scroll + visible_rows) {
        let mut cells = Vec::new();
        for col_idx in 0..cols {
            let i = row_idx * cols + col_idx;
            if i < total {
                let name = names[i];
                let th = crate::theme::get_theme(name);
                let gpu = display_color(th.gpu, truecolor);
                let prefix = if i == cursor { " > " } else { "   " };
                let style = if i == cursor {
                    Style::default()
                        .fg(gpu)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else if name == state.theme_name {
                    Style::default().fg(gpu).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(gpu)
                };
                let suffix = if name == state.theme_name && i != cursor {
                    " *"
                } else {
                    ""
                };
                // Build spans with name and swatches
                let line_spans = vec![
                    Span::styled(format!("{}{}{}", prefix, name, suffix), style),
                    Span::raw(" "),
                    Span::styled("  ", Style::default().bg(display_color(th.gpu, truecolor))),
                    Span::raw(" "),
                    Span::styled("  ", Style::default().bg(display_color(th.cpu, truecolor))),
                    Span::raw(" "),
                    Span::styled("  ", Style::default().bg(display_color(th.mem, truecolor))),
                    Span::raw(" "),
                    Span::styled(
                        "  ",
                        Style::default().bg(display_color(th.bar_mid, truecolor)),
                    ),
                ];
                cells.push(Cell::from(Line::from(line_spans)));
            } else {
                cells.push(Cell::from(""));
            }
        }
        rows.push(Row::new(cells));
    }

    let widths = vec![Constraint::Ratio(1, 3); cols];
    let table =
        Table::new(rows, widths).block(styled_block("Select Theme", Color::White, truecolor));
    f.render_widget(table, chunks[0]);

    // Preview
    let preview_name = names.get(cursor).unwrap_or(&"Default");
    let preview = crate::theme::get_theme(preview_name);
    let sample_bar = bar_spans(65.0, 20, &preview, state.steady_render, truecolor);
    let dash = "\u{2501}".repeat(6); // ━ × 6
    let preview_spans = vec![
        Span::styled("Preview: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("{}", preview_name),
            Style::default().fg(display_color(preview.gpu, truecolor)),
        ),
        Span::raw("  GPU "),
        Span::styled(
            dash.clone(),
            Style::default().fg(display_color(preview.gpu, truecolor)),
        ),
        Span::raw("  Net "),
        Span::styled(
            dash.clone(),
            Style::default().fg(display_color(preview.net, truecolor)),
        ),
        Span::raw("  CPU "),
        Span::styled(
            dash.clone(),
            Style::default().fg(display_color(preview.cpu, truecolor)),
        ),
        Span::raw("  Mem "),
        Span::styled(
            dash,
            Style::default().fg(display_color(preview.mem, truecolor)),
        ),
    ];
    let preview_line1 = Line::from(preview_spans);
    let mut bar_line_spans = vec![Span::raw("  Bar: ")];
    bar_line_spans.extend(sample_bar);
    let preview_line2 = Line::from(bar_line_spans);

    let preview_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(muted_style(state.steady_render, truecolor));
    f.render_widget(
        Paragraph::new(vec![Line::from(""), preview_line1, preview_line2]).block(preview_block),
        chunks[1],
    );

    // Hints
    let hint = Line::from(vec![
        Span::styled(
            " UP/DOWN/LEFT/RIGHT",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Navigate  "),
        Span::styled("ENTER", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" Select  "),
        Span::styled("ESC", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" Cancel"),
    ]);
    f.render_widget(Paragraph::new(hint), chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::{braille_bucket, core_ascii_glyphs, core_braille_glyphs, core_grid_cells, BRAILLE};

    #[test]
    fn lays_out_thirty_two_cores_as_four_rows_of_eight() {
        let pcts: Vec<f64> = (0..32).map(|i| i as f64).collect();
        let rows = core_grid_cells(&pcts);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(rows[3][7], 31.0);
    }

    #[test]
    fn groups_beyond_thirty_two_cores_by_max() {
        // 40 cores -> groups of 2 -> 20 cells -> 3 rows; core 37 pegged
        let mut pcts = vec![10.0; 40];
        pcts[37] = 100.0;
        let rows = core_grid_cells(&pcts);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2][2], 100.0); // cell 18 = row 2, col 2
        assert_eq!(rows[0][0], 10.0);
    }

    #[test]
    fn short_core_counts_use_partial_rows() {
        let rows = core_grid_cells(&[5.0, 90.0, 40.0]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec![5.0, 90.0, 40.0]);
    }

    #[test]
    fn braille_tables_cover_every_braille_glyph_exactly_once() {
        let mut seen = [0u32; 256];
        for (bucket, glyphs) in BRAILLE.iter().enumerate() {
            for &g in glyphs.iter() {
                let idx = g as u32 - 0x2800;
                assert!(idx < 256, "glyph outside U+2800..U+28FF: {g:?}");
                seen[idx as usize] += 1;
                let dots = (idx as u8).count_ones() as usize;
                assert_eq!(dots, bucket, "{g:?} has {dots} dots but sits in bucket {bucket}");
            }
        }
        for (idx, &count) in seen.iter().enumerate() {
            assert_eq!(count, 1, "U+{:04X} appears {count} times in BRAILLE tables", idx + 0x2800);
        }
    }

    #[test]
    fn braille_bucket_rounds_usage_to_nearest_eighth() {
        assert_eq!(braille_bucket(0.0), 0);
        assert_eq!(braille_bucket(6.2), 0);
        assert_eq!(braille_bucket(6.3), 1);
        assert_eq!(braille_bucket(37.5), 3);
        assert_eq!(braille_bucket(50.0), 4);
        assert_eq!(braille_bucket(87.5), 7);
        assert_eq!(braille_bucket(93.8), 8);
        assert_eq!(braille_bucket(100.0), 8);
        assert_eq!(braille_bucket(-10.0), 0);
        assert_eq!(braille_bucket(500.0), 8);
    }

    #[test]
    fn core_glyph_pair_is_two_chars_from_the_usage_bucket() {
        for pct in [12.0, 25.0, 37.5, 50.0, 62.5, 75.0, 88.0, 100.0] {
            let pair = core_braille_glyphs(pct, 7, 0, 3, 0xA5A5);
            let chars: Vec<char> = pair.chars().collect();
            assert_eq!(chars.len(), 2, "pair must occupy two cells: {pair:?}");
            let dots = braille_bucket(pct);
            for c in chars {
                assert!((c as u32) >= 0x2800 && (c as u32) <= 0x28FF, "out of block: {c:?}");
                assert_eq!(((c as u32 - 0x2800) as u8).count_ones() as usize, dots, "{c:?} not in bucket for {pct}% usage");
            }
        }
    }

    #[test]
    fn idle_core_flickers_one_dot_across_the_two_glyphs() {
        let mut lit = 0;
        let mut positions = [false; 16];
        let mut on_left = false;
        let mut on_right = false;
        for frame in 0..2000u64 {
            let pair = core_braille_glyphs(0.0, 3, frame, 7, 0xBEEF);
            let chars: Vec<char> = pair.chars().collect();
            assert_eq!(chars.len(), 2);
            let bits: Vec<u32> = chars.iter().map(|c| *c as u32 - 0x2800).collect();
            let total: u32 = bits.iter().map(|b| b.count_ones()).sum();
            assert!(total <= 1, "idle flicker must show at most one dot: {pair:?}");
            if total == 0 {
                assert_eq!(pair, "\u{2800}\u{2800}");
            } else {
                lit += 1;
                let (idx, b) = if bits[0] != 0 { (0, bits[0]) } else { (1, bits[1]) };
                assert_eq!(b.count_ones(), 1);
                on_left |= idx == 0;
                on_right |= idx == 1;
                let dot = b.trailing_zeros() as usize; // 8 dot bits
                positions[idx * 8 + dot] = true;
            }
        }
        assert!((300..500).contains(&lit), "lit frames {lit}/2000, expected ~20%");
        assert!(on_left && on_right, "dot must land on both glyphs");
        assert!(positions.iter().all(|&p| p), "all 16 dot positions must appear");
    }

    #[test]
    fn idle_flicker_is_deterministic_per_frame() {
        assert_eq!(
            core_braille_glyphs(0.0, 3, 42, 7, 0xBEEF),
            core_braille_glyphs(0.0, 3, 42, 7, 0xBEEF)
        );
    }

    #[test]
    fn glyph_pair_is_stable_within_a_second_and_rerolls_across_seconds() {
        let a = core_braille_glyphs(50.0, 100, 0, 5, 0x1234);
        let b = core_braille_glyphs(50.0, 100, 0, 5, 0x1234);
        assert_eq!(a, b);
        let mut rerolled = false;
        for s in 101..140 {
            if core_braille_glyphs(50.0, s, 0, 5, 0x1234) != a {
                rerolled = true;
                break;
            }
        }
        assert!(rerolled, "glyphs never re-roll across seconds");
    }

    #[test]
    fn ascii_fallback_pairs_use_the_bucket_and_the_one_below() {
        assert_eq!(core_ascii_glyphs(0.0), "  ");
        assert_eq!(core_ascii_glyphs(25.0), ". ");
        assert_eq!(core_ascii_glyphs(100.0), "#=");
        assert_eq!(core_ascii_glyphs(50.0), ":.");
    }
}
