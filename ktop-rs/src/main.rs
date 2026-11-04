mod app;
mod config;
mod gpu;
mod system;
mod theme;
mod ui;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const MIN_REFRESH_SECS: f64 = 0.25;
const COMPAT_REFRESH_SECS: f64 = 2.0;
const DIAGNOSTIC_DURATION_SECS: f64 = 5.0;
const RENDER_DIAGNOSTIC_DURATION_SECS: f64 = 4.0;
const INSTALL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/SleepinDevil/ktop/master/install.sh";

fn main() {
    let mut refresh = 1.0f64;
    let mut refresh_explicit = false;
    let mut sim = false;
    let mut theme_override: Option<String> = None;
    let mut no_alt_screen = env_flag("KTOP_NO_ALT_SCREEN");
    let mut no_sync = env_flag("KTOP_NO_SYNC");
    let mut compat = env_flag("KTOP_COMPAT");
    let mut no_update_check = env_flag("KTOP_NO_UPDATE_CHECK");

    let args: Vec<String> = env::args().collect();
    if let Some(command) = args.get(1).map(String::as_str) {
        match command {
            "update" => {
                if let Some(arg) = args.get(2) {
                    if is_help_arg(arg) {
                        print_update_help();
                        return;
                    }
                    exit_usage_error(format!("unexpected argument '{}' for ktop update", arg));
                }
                if let Err(e) = run_update() {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                return;
            }
            "uninstall" => {
                if let Some(arg) = args.get(2) {
                    if is_help_arg(arg) {
                        print_uninstall_help();
                        return;
                    }
                    exit_usage_error(format!("unexpected argument '{}' for ktop uninstall", arg));
                }
                if let Err(e) = run_uninstall(&args[0]) {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                return;
            }
            "diagnose-terminal" => {
                if let Err(e) = run_terminal_diagnostics(&args[2..]) {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                return;
            }
            "diagnose-render" => {
                if let Err(e) = run_render_diagnostics(&args[2..]) {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                return;
            }
            _ if command.starts_with('-') => {}
            _ => exit_usage_error(format!("unknown command '{}'", command)),
        }
    }

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-v" | "--version" => {
                println!("ktop {}", VERSION);
                return;
            }
            "-h" | "--help" => {
                print_main_help();
                return;
            }
            "-r" | "--refresh" => {
                i += 1;
                if i < args.len() {
                    refresh = parse_refresh(&args[i]);
                    refresh_explicit = true;
                } else {
                    exit_usage_error("missing value for --refresh");
                }
            }
            "--theme" => {
                i += 1;
                if i < args.len() {
                    theme_override = Some(args[i].clone());
                } else {
                    exit_usage_error("missing value for --theme");
                }
            }
            "--sim" => {
                sim = true;
            }
            "--no-alt-screen" => {
                no_alt_screen = true;
            }
            "--no-sync" => {
                no_sync = true;
            }
            "--compat" => {
                compat = true;
            }
            "--no-update-check" => {
                no_update_check = true;
            }
            other if other.starts_with('-') => {
                exit_usage_error(format!("unknown option '{}'", other));
            }
            other => exit_usage_error(format!("unexpected argument '{}'", other)),
        }
        i += 1;
    }

    if compat {
        no_alt_screen = true;
        no_sync = true;
        if !refresh_explicit {
            refresh = COMPAT_REFRESH_SECS;
        }
    }

    if let Err(e) = app::run(
        refresh,
        sim,
        theme_override,
        no_alt_screen,
        no_sync,
        compat,
        !no_update_check,
    ) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn parse_refresh(value: &str) -> f64 {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(|v| v.max(MIN_REFRESH_SECS))
        .unwrap_or(1.0)
}

fn parse_duration_secs(value: &str) -> f64 {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
        .map(|v| v.clamp(1.0, 60.0))
        .unwrap_or(DIAGNOSTIC_DURATION_SECS)
}

fn env_flag(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1")
            | Some("true")
            | Some("TRUE")
            | Some("yes")
            | Some("YES")
            | Some("on")
            | Some("ON")
    )
}

fn is_help_arg(arg: &str) -> bool {
    matches!(arg, "-h" | "--help")
}

fn print_main_help() {
    println!("ktop {} — system monitor for hybrid LLM workloads", VERSION);
    println!();
    println!("Usage: ktop [OPTIONS]");
    println!("       ktop <COMMAND>");
    println!();
    println!("Options:");
    println!("  -v, --version          Print version");
    println!("  -r, --refresh <SECS>   Refresh interval (default: 1.0)");
    println!("  --theme <NAME>         Color theme");
    println!("  --sim                  Simulation mode");
    println!("  --no-alt-screen        Draw in the current terminal screen");
    println!("  --no-sync              Disable synchronized terminal redraws");
    println!("  --compat               Use conservative remote-terminal drawing");
    println!("  --no-update-check      Skip the background release check at startup");
    println!("  -h, --help             Print help");
    println!();
    println!("Commands:");
    println!("  diagnose-terminal      Report terminal size and resize behavior");
    println!("  diagnose-render        Run visual terminal repaint isolation tests");
    println!("  update                 Run the one-line update installer from the README");
    println!("  uninstall              Remove this ktop executable");
}

fn exit_usage_error(message: impl AsRef<str>) -> ! {
    eprintln!("Error: {}", message.as_ref());
    eprintln!("Run 'ktop --help' for usage.");
    std::process::exit(2);
}

fn print_update_help() {
    println!("ktop update");
    println!();
    println!("Runs the one-line installer from the README:");
    println!("  curl -sSfL {} | bash", INSTALL_SCRIPT_URL);
}

fn print_diagnostics_help() {
    println!("ktop diagnose-terminal");
    println!();
    println!("Reports terminal size stability and resize events without starting the TUI.");
    println!();
    println!("Usage: ktop diagnose-terminal [--duration SECS]");
}

fn print_render_diagnostics_help() {
    println!("ktop diagnose-render");
    println!();
    println!("Runs a short visual repaint test without starting the monitor.");
    println!("Use one case at a time and report which case visibly flashes.");
    println!();
    println!("Usage: ktop diagnose-render [--case CASE] [--duration SECS]");
    println!();
    println!("Cases:");
    println!("  plain       Print normal lines only");
    println!("  cursor      Repaint a small ASCII panel with cursor movement");
    println!("  color       Cursor movement plus ANSI color");
    println!("  rgb         Cursor movement plus 24-bit RGB color");
    println!("  unicode     Cursor movement plus Unicode block glyphs");
    println!("  full-paint  Large colored Unicode repaint, no clear/alt/sync");
    println!("  clear       Repeated full-screen clear plus small repaint");
    println!("  alternate   Alternate screen plus small repaint");
    println!("  sync        Synchronized update plus large repaint");
}

fn run_terminal_diagnostics(args: &[String]) -> Result<(), String> {
    let mut duration = DIAGNOSTIC_DURATION_SECS;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_diagnostics_help();
                return Ok(());
            }
            "--duration" => {
                i += 1;
                if let Some(value) = args.get(i) {
                    duration = parse_duration_secs(value);
                } else {
                    return Err("missing value for --duration".to_string());
                }
            }
            other => return Err(format!("unknown diagnose-terminal option '{}'", other)),
        }
        i += 1;
    }

    let initial_size =
        terminal::size().map_err(|e| format!("could not read terminal size: {}", e))?;
    let mut stats = TerminalDiagnostics {
        duration,
        stdin_tty: io::stdin().is_terminal(),
        stdout_tty: io::stdout().is_terminal(),
        stderr_tty: io::stderr().is_terminal(),
        initial_size,
        min_width: initial_size.0,
        max_width: initial_size.0,
        min_height: initial_size.1,
        max_height: initial_size.1,
        ..TerminalDiagnostics::default()
    };

    let raw_mode = stats.stdin_tty && terminal::enable_raw_mode().is_ok();
    let started = Instant::now();
    let deadline = started + Duration::from_secs_f64(duration);

    let collect_result = (|| -> Result<(), String> {
        while Instant::now() < deadline {
            let size = terminal::size()
                .map_err(|e| format!("could not read terminal size during probe: {}", e))?;
            stats.record_size(size);

            if raw_mode {
                while event::poll(Duration::from_millis(0))
                    .map_err(|e| format!("could not poll terminal events: {}", e))?
                {
                    match event::read()
                        .map_err(|e| format!("could not read terminal event: {}", e))?
                    {
                        Event::Resize(width, height) => {
                            stats.resize_events += 1;
                            stats.record_event_size((width, height));
                        }
                        Event::Key(_) => stats.key_events += 1,
                        Event::Mouse(_) => stats.mouse_events += 1,
                        Event::Paste(_) => stats.paste_events += 1,
                        Event::FocusGained | Event::FocusLost => stats.focus_events += 1,
                    }
                }
            }

            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    })();

    if raw_mode {
        let _ = terminal::disable_raw_mode();
    }
    collect_result?;

    stats.print(raw_mode);
    Ok(())
}

fn run_render_diagnostics(args: &[String]) -> Result<(), String> {
    let mut duration = RENDER_DIAGNOSTIC_DURATION_SECS;
    let mut case = RenderDiagnosticCase::FullPaint;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_render_diagnostics_help();
                return Ok(());
            }
            "--duration" => {
                i += 1;
                if let Some(value) = args.get(i) {
                    duration = parse_duration_secs(value);
                } else {
                    return Err("missing value for --duration".to_string());
                }
            }
            "--case" => {
                i += 1;
                if let Some(value) = args.get(i) {
                    case = RenderDiagnosticCase::parse(value)?;
                } else {
                    return Err("missing value for --case".to_string());
                }
            }
            other => return Err(format!("unknown diagnose-render option '{}'", other)),
        }
        i += 1;
    }

    let size = terminal::size().map_err(|e| format!("could not read terminal size: {}", e))?;
    println!("ktop render diagnostics");
    println!("version: {}", VERSION);
    println!("case: {}", case.name());
    println!("duration_secs: {:.1}", duration);
    println!("stdout_tty: {}", io::stdout().is_terminal());
    println!(
        "TERM: {}",
        env::var("TERM").unwrap_or_else(|_| "(unset)".to_string())
    );
    println!("size: {}x{}", size.0, size.1);
    println!("watch this case for flashing; the test starts in 1 second");
    io::stdout()
        .flush()
        .map_err(|e| format!("could not flush stdout: {}", e))?;
    std::thread::sleep(Duration::from_secs(1));

    run_render_diagnostic_case(case, duration, size)?;
    println!(
        "render diagnostic complete: case={} frames={}",
        case.name(),
        (duration / 0.25).ceil() as u64
    );
    Ok(())
}

#[derive(Clone, Copy)]
enum RenderDiagnosticCase {
    Plain,
    Cursor,
    Color,
    Rgb,
    Unicode,
    FullPaint,
    Clear,
    Alternate,
    Sync,
}

impl RenderDiagnosticCase {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "plain" => Ok(Self::Plain),
            "cursor" => Ok(Self::Cursor),
            "color" => Ok(Self::Color),
            "rgb" => Ok(Self::Rgb),
            "unicode" => Ok(Self::Unicode),
            "full-paint" => Ok(Self::FullPaint),
            "clear" => Ok(Self::Clear),
            "alternate" => Ok(Self::Alternate),
            "sync" => Ok(Self::Sync),
            _ => Err(format!("unknown render diagnostic case '{}'", value)),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Cursor => "cursor",
            Self::Color => "color",
            Self::Rgb => "rgb",
            Self::Unicode => "unicode",
            Self::FullPaint => "full-paint",
            Self::Clear => "clear",
            Self::Alternate => "alternate",
            Self::Sync => "sync",
        }
    }
}

fn run_render_diagnostic_case(
    case: RenderDiagnosticCase,
    duration: f64,
    size: (u16, u16),
) -> Result<(), String> {
    if matches!(case, RenderDiagnosticCase::Plain) {
        let deadline = Instant::now() + Duration::from_secs_f64(duration);
        let mut frame = 0u64;
        while Instant::now() < deadline {
            frame += 1;
            println!("plain diagnostic frame {:03}", frame);
            std::thread::sleep(Duration::from_millis(250));
        }
        return Ok(());
    }

    terminal::enable_raw_mode()
        .map_err(|e| format!("could not enable raw mode for render diagnostic: {}", e))?;
    let result = run_render_diagnostic_case_raw(case, duration, size);
    let cleanup_result = terminal::disable_raw_mode()
        .map_err(|e| format!("could not disable raw mode after render diagnostic: {}", e));

    match (result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), _) => Err(e),
        (Ok(()), Err(e)) => Err(e),
    }
}

fn run_render_diagnostic_case_raw(
    case: RenderDiagnosticCase,
    duration: f64,
    size: (u16, u16),
) -> Result<(), String> {
    let mut stdout = io::stdout();
    let use_alt = matches!(case, RenderDiagnosticCase::Alternate);

    if use_alt {
        execute!(stdout, EnterAlternateScreen)
            .map_err(|e| format!("could not enter alternate screen: {}", e))?;
    }
    execute!(stdout, Hide).map_err(|e| format!("could not hide cursor: {}", e))?;

    let result = (|| -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs_f64(duration);
        let mut frame = 0u64;
        while Instant::now() < deadline {
            frame += 1;
            match case {
                RenderDiagnosticCase::Cursor | RenderDiagnosticCase::Alternate => {
                    draw_small_ascii_panel(&mut stdout, case.name(), frame, size)?;
                }
                RenderDiagnosticCase::Color => {
                    draw_color_panel(&mut stdout, case.name(), frame, size)?;
                }
                RenderDiagnosticCase::Rgb => {
                    draw_rgb_panel(&mut stdout, case.name(), frame, size)?;
                }
                RenderDiagnosticCase::Unicode => {
                    draw_unicode_panel(&mut stdout, case.name(), frame, size)?;
                }
                RenderDiagnosticCase::FullPaint => {
                    draw_full_paint(&mut stdout, case.name(), frame, size)?;
                }
                RenderDiagnosticCase::Clear => {
                    queue!(stdout, Clear(ClearType::All))
                        .map_err(|e| format!("could not queue clear: {}", e))?;
                    draw_small_ascii_panel(&mut stdout, case.name(), frame, size)?;
                }
                RenderDiagnosticCase::Sync => {
                    queue!(stdout, BeginSynchronizedUpdate)
                        .map_err(|e| format!("could not start synchronized update: {}", e))?;
                    let draw_result = draw_full_paint(&mut stdout, case.name(), frame, size);
                    let end_result = queue!(stdout, EndSynchronizedUpdate)
                        .map_err(|e| format!("could not end synchronized update: {}", e));
                    draw_result?;
                    end_result?;
                }
                RenderDiagnosticCase::Plain => {}
            }
            stdout
                .flush()
                .map_err(|e| format!("could not flush render frame: {}", e))?;
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    })();

    let cleanup = (|| -> Result<(), String> {
        queue!(stdout, ResetColor, Show)
            .map_err(|e| format!("could not queue terminal reset: {}", e))?;
        if use_alt {
            queue!(stdout, LeaveAlternateScreen)
                .map_err(|e| format!("could not leave alternate screen: {}", e))?;
        } else {
            queue!(stdout, MoveTo(0, size.1.saturating_sub(1)))
                .map_err(|e| format!("could not move cursor for cleanup: {}", e))?;
            queue!(stdout, Print("\r\n"))
                .map_err(|e| format!("could not queue cleanup newline: {}", e))?;
        }
        stdout
            .flush()
            .map_err(|e| format!("could not flush terminal cleanup: {}", e))
    })();

    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), _) => Err(e),
        (Ok(()), Err(e)) => Err(e),
    }
}

fn draw_small_ascii_panel(
    stdout: &mut io::Stdout,
    label: &str,
    frame: u64,
    size: (u16, u16),
) -> Result<(), String> {
    let width = size.0.max(1).min(96) as usize;
    queue!(stdout, MoveTo(0, 0)).map_err(|e| format!("could not move cursor: {}", e))?;
    for row in 0..8u16 {
        queue!(stdout, MoveTo(0, row)).map_err(|e| format!("could not move cursor: {}", e))?;
        let text = match row {
            0 => format!("ktop render diagnostic: {}", label),
            1 => format!("frame: {:03}", frame),
            2 => "ASCII repaint with cursor movement only".to_string(),
            4 => format!(
                "load [{}{}]",
                "#".repeat((frame as usize % 20) + 1),
                " ".repeat(19 - (frame as usize % 20))
            ),
            6 => "If this flashes, cursor-position repaint is the trigger.".to_string(),
            _ => String::new(),
        };
        queue!(stdout, Print(pad_to_width(&text, width)))
            .map_err(|e| format!("could not write panel row: {}", e))?;
    }
    Ok(())
}

fn draw_color_panel(
    stdout: &mut io::Stdout,
    label: &str,
    frame: u64,
    size: (u16, u16),
) -> Result<(), String> {
    draw_small_ascii_panel(stdout, label, frame, size)?;
    let colors = [
        Color::Blue,
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Red,
        Color::Magenta,
    ];
    queue!(stdout, MoveTo(0, 9)).map_err(|e| format!("could not move cursor: {}", e))?;
    for (idx, color) in colors.iter().enumerate() {
        queue!(
            stdout,
            SetForegroundColor(*color),
            Print(format!(" color{} ", idx + 1))
        )
        .map_err(|e| format!("could not write color sample: {}", e))?;
    }
    queue!(stdout, ResetColor).map_err(|e| format!("could not reset color: {}", e))?;
    Ok(())
}

fn draw_rgb_panel(
    stdout: &mut io::Stdout,
    label: &str,
    frame: u64,
    size: (u16, u16),
) -> Result<(), String> {
    draw_small_ascii_panel(stdout, label, frame, size)?;
    let colors = [
        Color::Rgb {
            r: 70,
            g: 130,
            b: 180,
        },
        Color::Rgb {
            r: 40,
            g: 190,
            b: 140,
        },
        Color::Rgb {
            r: 230,
            g: 180,
            b: 60,
        },
        Color::Rgb {
            r: 220,
            g: 90,
            b: 80,
        },
    ];
    queue!(stdout, MoveTo(0, 9)).map_err(|e| format!("could not move cursor: {}", e))?;
    for (idx, color) in colors.iter().enumerate() {
        queue!(
            stdout,
            SetForegroundColor(*color),
            Print(format!(" rgb{} ", idx + 1))
        )
        .map_err(|e| format!("could not write RGB sample: {}", e))?;
    }
    queue!(stdout, ResetColor).map_err(|e| format!("could not reset color: {}", e))?;
    Ok(())
}

fn draw_unicode_panel(
    stdout: &mut io::Stdout,
    label: &str,
    frame: u64,
    size: (u16, u16),
) -> Result<(), String> {
    draw_small_ascii_panel(stdout, label, frame, size)?;
    let blocks = ["▁▂▃▄▅▆▇█", "░░▒▒▓▓██", "╭────╮ │ │ ╰────╯"];
    for (idx, sample) in blocks.iter().enumerate() {
        queue!(stdout, MoveTo(0, 9 + idx as u16))
            .map_err(|e| format!("could not move cursor: {}", e))?;
        queue!(
            stdout,
            Print(format!("unicode sample {}: {}", idx + 1, sample))
        )
        .map_err(|e| format!("could not write unicode sample: {}", e))?;
    }
    Ok(())
}

fn draw_full_paint(
    stdout: &mut io::Stdout,
    label: &str,
    frame: u64,
    size: (u16, u16),
) -> Result<(), String> {
    let width = size.0.max(1) as usize;
    let height = size.1.max(8).min(40);
    let palette = [
        Color::Rgb {
            r: 70,
            g: 130,
            b: 180,
        },
        Color::Rgb {
            r: 40,
            g: 190,
            b: 140,
        },
        Color::Rgb {
            r: 230,
            g: 180,
            b: 60,
        },
        Color::Rgb {
            r: 220,
            g: 90,
            b: 80,
        },
    ];

    for row in 0..height {
        queue!(stdout, MoveTo(0, row)).map_err(|e| format!("could not move cursor: {}", e))?;
        queue!(
            stdout,
            SetForegroundColor(palette[row as usize % palette.len()])
        )
        .map_err(|e| format!("could not set foreground color: {}", e))?;

        let line = if row == 0 {
            format!("╭─ ktop render diagnostic: {} frame {:03}", label, frame)
        } else if row == height - 1 {
            "╰".to_string() + &"─".repeat(width.saturating_sub(1))
        } else if row % 4 == 0 {
            format!(
                "│ GPU{} {} {}",
                row % 8,
                "█".repeat((frame as usize + row as usize) % width.max(1)),
                "░".repeat(width / 4)
            )
        } else if row % 4 == 1 {
            format!(
                "│ CPU  {:>3}% {}",
                (frame * 7 + row as u64) % 100,
                "▁▂▃▄▅▆▇█".repeat((width / 8).max(1))
            )
        } else if row % 4 == 2 {
            format!(
                "│ MEM  {:>3}% {}",
                (frame * 5 + row as u64) % 100,
                "░▒▓█".repeat((width / 4).max(1))
            )
        } else {
            format!(
                "│ PROC {:>5} command-{} {}",
                frame * row as u64,
                row,
                "─".repeat(width / 2)
            )
        };
        queue!(stdout, Print(truncate_pad_to_width(&line, width)))
            .map_err(|e| format!("could not write full paint row: {}", e))?;
    }
    queue!(stdout, ResetColor).map_err(|e| format!("could not reset color: {}", e))?;
    Ok(())
}

fn pad_to_width(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        text.chars().take(width).collect()
    } else {
        format!("{}{}", text, " ".repeat(width - len))
    }
}

fn truncate_pad_to_width(text: &str, width: usize) -> String {
    pad_to_width(text, width)
}

#[derive(Default)]
struct TerminalDiagnostics {
    duration: f64,
    stdin_tty: bool,
    stdout_tty: bool,
    stderr_tty: bool,
    initial_size: (u16, u16),
    samples: u64,
    size_changes: u64,
    resize_events: u64,
    key_events: u64,
    mouse_events: u64,
    focus_events: u64,
    paste_events: u64,
    min_width: u16,
    max_width: u16,
    min_height: u16,
    max_height: u16,
    last_size: Option<(u16, u16)>,
    sampled_sizes: BTreeMap<(u16, u16), u64>,
    resize_event_sizes: BTreeMap<(u16, u16), u64>,
}

impl TerminalDiagnostics {
    fn record_size(&mut self, size: (u16, u16)) {
        self.samples += 1;
        if self.last_size.is_some_and(|last| last != size) {
            self.size_changes += 1;
        }
        self.last_size = Some(size);
        self.min_width = self.min_width.min(size.0);
        self.max_width = self.max_width.max(size.0);
        self.min_height = self.min_height.min(size.1);
        self.max_height = self.max_height.max(size.1);
        *self.sampled_sizes.entry(size).or_insert(0) += 1;
    }

    fn record_event_size(&mut self, size: (u16, u16)) {
        *self.resize_event_sizes.entry(size).or_insert(0) += 1;
    }

    fn print(&self, raw_mode: bool) {
        println!("ktop terminal diagnostics");
        println!("version: {}", VERSION);
        println!("duration_secs: {:.1}", self.duration);
        println!("stdin_tty: {}", self.stdin_tty);
        println!("stdout_tty: {}", self.stdout_tty);
        println!("stderr_tty: {}", self.stderr_tty);
        println!("raw_mode_probe: {}", raw_mode);
        println!(
            "TERM: {}",
            env::var("TERM").unwrap_or_else(|_| "(unset)".to_string())
        );
        println!(
            "COLORTERM: {}",
            env::var("COLORTERM").unwrap_or_else(|_| "(unset)".to_string())
        );
        println!(
            "initial_size: {}x{}",
            self.initial_size.0, self.initial_size.1
        );
        println!("samples: {}", self.samples);
        println!("sampled_size_changes: {}", self.size_changes);
        println!("sampled_unique_sizes: {}", self.sampled_sizes.len());
        println!(
            "sampled_width_range: {}..{}",
            self.min_width, self.max_width
        );
        println!(
            "sampled_height_range: {}..{}",
            self.min_height, self.max_height
        );
        println!("resize_events: {}", self.resize_events);
        println!("key_events: {}", self.key_events);
        println!("mouse_events: {}", self.mouse_events);
        println!("focus_events: {}", self.focus_events);
        println!("paste_events: {}", self.paste_events);
        println!("sampled_sizes: {}", format_size_counts(&self.sampled_sizes));
        println!(
            "resize_event_sizes: {}",
            format_size_counts(&self.resize_event_sizes)
        );

        let unstable_size = self.size_changes > 1 || self.sampled_sizes.len() > 1;
        let resize_storm = self.resize_events > 2;
        println!("unstable_size: {}", unstable_size);
        println!("resize_event_storm: {}", resize_storm);
        if unstable_size || resize_storm {
            println!(
                "diagnosis: terminal size is changing during the probe; older full-screen rendering can repeatedly clear in this condition"
            );
        } else {
            println!(
                "diagnosis: terminal size stayed stable during the probe; flashing is likely caused by a specific draw escape or terminal repaint behavior"
            );
        }
    }
}

fn format_size_counts(sizes: &BTreeMap<(u16, u16), u64>) -> String {
    if sizes.is_empty() {
        return "(none)".to_string();
    }

    sizes
        .iter()
        .map(|((width, height), count)| format!("{}x{}={}", width, height, count))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_uninstall_help() {
    println!("ktop uninstall");
    println!();
    println!("Removes only the ktop executable that was resolved from your PATH.");
    println!("User config and other system files are left untouched.");
}

fn run_update() -> Result<(), String> {
    println!("Running ktop update via the official installer...");
    println!("curl -sSfL {} | bash", INSTALL_SCRIPT_URL);

    let status = Command::new("bash")
        .arg("-c")
        .arg(format!("curl -sSfL {} | bash", INSTALL_SCRIPT_URL))
        .status()
        .map_err(|e| format!("failed to start update installer: {}", e))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "update installer exited with status {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ))
    }
}

fn run_uninstall(argv0: &str) -> Result<(), String> {
    let target = resolve_invoked_path(argv0)
        .map_err(|e| format!("could not resolve the ktop executable path: {}", e))?;

    validate_uninstall_target(&target)?;

    println!("Removing {}", target.display());
    match fs::remove_file(&target) {
        Ok(()) => {
            print_uninstall_success(&target);
            Ok(())
        }
        Err(e)
            if e.kind() == io::ErrorKind::PermissionDenied && is_readme_install_path(&target) =>
        {
            let status = Command::new("sudo")
                .arg("rm")
                .arg("-f")
                .arg(&target)
                .status()
                .map_err(|sudo_err| {
                    format!(
                        "permission denied removing {}; sudo failed to start: {}",
                        target.display(),
                        sudo_err
                    )
                })?;

            if status.success() {
                print_uninstall_success(&target);
                Ok(())
            } else {
                Err(format!(
                    "sudo rm exited with status {}",
                    status
                        .code()
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                ))
            }
        }
        Err(e) => Err(format!("failed to remove {}: {}", target.display(), e)),
    }
}

fn resolve_invoked_path(argv0: &str) -> io::Result<PathBuf> {
    let invoked = Path::new(argv0);
    if invoked.is_absolute() {
        return Ok(invoked.to_path_buf());
    }

    if argv0.contains('/') {
        return Ok(env::current_dir()?.join(invoked));
    }

    if let Some(path_var) = env::var_os("PATH") {
        let cwd = env::current_dir()?;
        for dir in env::split_paths(&path_var) {
            let absolute_dir = if dir.is_absolute() {
                dir
            } else {
                cwd.join(dir)
            };
            let candidate = absolute_dir.join(argv0);
            if is_executable_file(&candidate) {
                return Ok(candidate);
            }
        }
    }

    env::current_exe()
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn validate_uninstall_target(path: &Path) -> Result<(), String> {
    if path.file_name().and_then(|name| name.to_str()) != Some("ktop") {
        return Err(format!(
            "refusing to remove {}; expected a file named ktop",
            path.display()
        ));
    }

    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("could not inspect {}: {}", path.display(), e))?;
    if metadata.file_type().is_dir() {
        return Err(format!("refusing to remove directory {}", path.display()));
    }

    Ok(())
}

fn is_readme_install_path(path: &Path) -> bool {
    path == Path::new("/usr/local/bin/ktop")
}

fn print_uninstall_success(path: &Path) {
    println!("Removed {}", path.display());
    println!("Left ktop user config untouched.");
    println!("Run 'hash -r' if your shell still remembers the old command path.");
}
