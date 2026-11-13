//! CodeCanvas (GPUI). Usage: codecanvas [folder] [file]

mod agent;
mod diff;
mod highlight;
mod llm;
mod model;
mod settings;
mod text;
mod ui;
mod web;

use gpui::{App, Application, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, channel};

const CHILD_FLAG: &str = "--cc-child";

/// Zed tasks wait for their program, so the visible process relaunches itself detached and exits.
fn relaunch_detached(args: &[String]) -> std::io::Result<()> {
    let mut c = Command::new(std::env::current_exe()?);
    c.args(args).arg(CHILD_FLAG).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    { use std::os::unix::process::CommandExt; c.process_group(0); }
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; c.creation_flags(0x0000_0008 | 0x0000_0200); } // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    c.spawn().map(|_| ())
}

fn port_file(project: u64) -> PathBuf { model::runtime_dir().join(format!("{project:016x}.port")) }

/// If this project is already open, ask that instance to show `file` and report true.
fn hand_off(project: u64, file: Option<&Path>) -> bool {
    let Ok(port) = std::fs::read_to_string(port_file(project)) else { return false };
    let Ok(port) = port.trim().parse::<u16>() else { return false };
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else { return false };
    writeln!(s, "{}", file.map(|f| f.display().to_string()).unwrap_or_default()).is_ok()
}

fn listen(project: u64, tx: Sender<String>) {
    let Ok(l) = TcpListener::bind(("127.0.0.1", 0)) else { return };
    let Ok(addr) = l.local_addr() else { return };
    let file = port_file(project);
    if let Some(d) = file.parent() { let _ = std::fs::create_dir_all(d); }
    let _ = std::fs::write(file, addr.port().to_string());
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(s).read_line(&mut line).is_ok() { let _ = tx.send(line.trim().to_string()); }
        }
    });
}

/// glibc's allocator settings. By default it raises its "use a separate mapping" threshold whenever a
/// large block is freed, after which big buffers (text cells, file reads) come from its pools and are
/// kept after being freed; and it gives every thread its own pool. Both keep memory the app no longer uses.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn tune_allocator() {
    unsafe extern "C" { fn mallopt(param: i32, value: i32) -> i32; }
    const M_TRIM_THRESHOLD: i32 = -1;
    const M_MMAP_THRESHOLD: i32 = -3;
    const M_ARENA_MAX: i32 = -8;
    // SAFETY: plain libc calls with documented parameters, made before any other thread exists.
    unsafe {
        mallopt(M_MMAP_THRESHOLD, 128 * 1024);
        mallopt(M_TRIM_THRESHOLD, 1024 * 1024);
        mallopt(M_ARENA_MAX, 2);
    }
}
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn tune_allocator() {}

fn main() {
    tune_allocator();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if !argv.iter().any(|a| a == CHILD_FLAG) && relaunch_detached(&argv).is_ok() { return; }
    let args: Vec<PathBuf> = argv.iter().filter(|a| !a.is_empty() && !a.starts_with("--")).map(PathBuf::from).collect();
    let mut root = args.iter().find(|p| p.is_dir()).cloned();
    let file = args.iter().find(|p| p.is_file()).cloned();
    if root.is_none() { root = file.as_ref().and_then(|f| f.parent().map(PathBuf::from)).filter(|p| !p.as_os_str().is_empty()); }
    let root = root.unwrap_or_else(|| std::env::current_dir().expect("no current directory"));
    let id = model::Project::new(root.clone(), Vec::new()).expect("can't open the project folder").id();
    if hand_off(id, file.as_deref()) { return; }
    let (tx, rx) = channel();
    listen(id, tx);

    Application::new().with_assets(ui::fx::Icons).run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1500.), px(950.)), cx);
        // The app id ties the window to codecanvas.desktop, which is where the taskbar gets its name and icon.
        let title = format!("CodeCanvas — {}", root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("codecanvas".into()),
            titlebar: Some(gpui::TitlebarOptions { title: Some(title.into()), ..Default::default() }),
            ..Default::default()
        };
        cx.open_window(opts, |window, cx| cx.new(|cx| ui::CanvasView::new(root, file, rx, window, cx))).unwrap();
        cx.on_window_closed(|cx| { if cx.windows().is_empty() { cx.quit(); } }).detach();
        cx.activate(true);
    });
}
