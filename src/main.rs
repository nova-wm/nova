#![allow(irrefutable_let_patterns)]

mod config;
mod handlers;
mod layout;
mod ipc;

mod border;
mod drm;
mod grabs;
mod input;
mod record;
mod screenshot;
mod state;
mod winit;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

pub use state::Smallvil;
pub use state::CanvasCamera;

use std::{
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

struct LogPaths {
    life: PathBuf,
    trace: PathBuf,
    crash: PathBuf,
}

/// Crate version + feature tag. The feature list is the reliable "is this
/// binary new?" signal: it only changes when behavior does.
const NOVAWM_VERSION: &str = env!("CARGO_PKG_VERSION");
const NOVAWM_FEATURES: &str = "cursor-themes input-block spawn-action welcome scratchpad-v2 screenshot record idle-notify session-lock window-rules image-copy-capture";

static LOG_PATHS: OnceLock<LogPaths> = OnceLock::new();

fn log_paths() -> &'static LogPaths {
    LOG_PATHS.get_or_init(|| {
        let directory = resolve_log_dir();

        LogPaths {
            life: directory.join("novawm-life.log"),
            trace: directory.join("novawm-trace.log"),
            crash: directory.join("novawm-crash.log"),
        }
    })
}

fn resolve_log_dir() -> PathBuf {
    let mut candidates = Vec::new();

    if let Some(directory) = std::env::var_os("NOVAWM_LOG_DIR") {
        if !directory.is_empty() {
            candidates.push(PathBuf::from(directory));
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            candidates.push(PathBuf::from(&home).join(".local/state/novawm"));

            candidates.push(PathBuf::from(home));
        }
    }

    candidates.push(std::env::temp_dir().join(format!("novawm-{}", std::process::id(),)));

    for directory in candidates {
        if probe_writable(&directory) {
            return directory;
        }
    }

    std::env::temp_dir()
}

fn probe_writable(directory: &Path) -> bool {
    if std::fs::create_dir_all(directory).is_err() {
        return false;
    }

    let probe = directory.join(format!(".novawm-write-probe-{}", std::process::id(),));

    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe);

    match result {
        Ok(mut file) => {
            let writable = file.write_all(b"probe").is_ok();

            drop(file);

            let _ = std::fs::remove_file(probe);

            writable
        }
        Err(_) => false,
    }
}

fn open_appendable(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

fn append_line(path: &Path, line: &str) -> bool {
    match open_appendable(path) {
        Ok(mut file) => {
            let written = writeln!(file, "{line}").is_ok();

            let _ = file.flush();

            written
        }
        Err(_) => false,
    }
}

pub fn life(message: impl AsRef<str>) {
    let line = format!("{:?} {}", std::time::SystemTime::now(), message.as_ref(),);

    {
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "[novawm] {line}");
    }

    if !append_line(&log_paths().life, &line) {
        let fallback =
            std::env::temp_dir().join(format!("novawm-life-fallback-{}.log", std::process::id(),));

        let _ = append_line(&fallback, &line);
    }
}

fn log_env_ctx() {
    life(format!(
        "environment: USER={:?} SUDO_USER={:?} HOME={:?} \
         XDG_SESSION_TYPE={:?} XDG_RUNTIME_DIR={:?} \
         WAYLAND_DISPLAY={:?} PID={}",
        std::env::var_os("USER"),
        std::env::var_os("SUDO_USER"),
        std::env::var_os("HOME"),
        std::env::var_os("XDG_SESSION_TYPE"),
        std::env::var_os("XDG_RUNTIME_DIR"),
        std::env::var_os("WAYLAND_DISPLAY"),
        std::process::id(),
    ));

    life(format!(
        "logs: life={} trace={} crash={}",
        log_paths().life.display(),
        log_paths().trace.display(),
        log_paths().crash.display(),
    ));
}

/// Resolve XDG_RUNTIME_DIR before either backend creates its Wayland socket.
///
fn prepare_runtime_dir() -> std::io::Result<()> {
    let directory = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(value) if !value.is_empty() => PathBuf::from(value),

        _ => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;

                let uid = std::fs::metadata("/proc/self")?.uid();

                PathBuf::from(format!("/run/user/{uid}"))
            }

            #[cfg(not(unix))]
            {
                return Err(std::io::Error::other("XDG_RUNTIME_DIR is not set"));
            }
        }
    };

    if !directory.is_absolute() {
        return Err(std::io::Error::other(format!(
            "XDG_RUNTIME_DIR must be absolute: {}",
            directory.display(),
        )));
    }

    if !directory.is_dir() {
        return Err(std::io::Error::other(format!(
            "runtime directory {} does not exist. \
             Log in normally on the TTY through logind/elogind; \
             do not launch the compositor through sudo.",
            directory.display(),
        )));
    }

    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", &directory);
    }

    life(format!("runtime directory: {}", directory.display(),));

    Ok(())
}

fn setup_panic_logging() {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_paths().crash)
        .ok();

    let file = Mutex::new(file);

    std::panic::set_hook(Box::new(move |info| {
        let message = format!("{:?}\n{info}\n", std::time::SystemTime::now(),);

        {
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "[novawm] PANIC: {message}");
        }

        let mut written = false;

        if let Ok(mut guard) = file.lock() {
            if let Some(file) = guard.as_mut() {
                written = file.write_all(message.as_bytes()).is_ok();
                let _ = file.flush();
            }
        }

        if !written {
            let fallback = std::env::temp_dir()
                .join(format!("novawm-crash-fallback-{}.log", std::process::id(),));

            let _ = append_line(&fallback, &message);
        }
    }));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("novawm {} (features: {})", NOVAWM_VERSION, NOVAWM_FEATURES);
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "novawm {NOVAWM_VERSION}\n\nUsage:\n  novawm drm          run on real hardware (from a TTY)\n  novawm              run nested in a window (testing)\n  novawm --version    print version and exit\n\nConfig search: $NOVAWM_CONFIG, ./config.kdl, ~/.config/novawm/config.kdl, /usr/share/novawm/config/."
        );
        return Ok(());
    }
    log_env_ctx();
    setup_panic_logging();

    if let Err(error) = prepare_runtime_dir() {
        life(format!("startup failed: {error}"));
        return Err(error.into());
    }

    init_logging();

    life("main: logging initialized");
    life(format!(
        "novawm {NOVAWM_VERSION} features: {NOVAWM_FEATURES}"
    ));

    if std::env::args().any(|argument| argument == "drm") {
        life("main: selecting DRM backend");
        life("drm: direct hardware session (cursor themes + libinput active)");

        return match std::panic::catch_unwind(drm::run_drm) {
            Ok(result) => {
                if let Err(error) = &result {
                    life(format!("DRM backend failed: {error}"));
                }

                result
            }

            Err(payload) => {
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|message| message.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "<non-string panic payload>".to_string());

                life(format!("DRM backend panicked: {message}"));

                Err(std::io::Error::other(format!(
                    "DRM backend panicked: {message}; see {}",
                    log_paths().crash.display(),
                ))
                .into())
            }
        };
    }

    life("main: selecting nested winit backend");
    life(
        "nested: the HOST compositor owns the cursor, layout-switch keys, and may grab          super-binds; cursor themes / touchpad / Alt+Shift need TTY + `drm`",
    );

    let mut event_loop: EventLoop<Smallvil> = EventLoop::try_new()?;

    let display: Display<Smallvil> = Display::new()?;

    let mut state = Smallvil::new(&mut event_loop, display);

    crate::winit::init_winit(&mut event_loop, &mut state)?;

    spawn_client(&state);
    start_portal_stack(&state.socket_name);

    event_loop.run(None, &mut state, |state| {
        state.space.refresh();

        if let Err(error) = state.display_handle.flush_clients() {
            tracing::warn!(?error, "failed to flush Wayland clients");
        }
    })?;

    Ok(())
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    match open_appendable(&log_paths().trace) {
        Ok(file) => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(TeeWriter {
                    file: std::sync::Arc::new(Mutex::new(file)),
                })
                .with_ansi(false)
                .init();
        }

        Err(error) => {
            life(format!("cannot open trace log: {error}; using stderr"));

            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }
}

#[derive(Clone)]
struct TeeWriter {
    file: std::sync::Arc<Mutex<std::fs::File>>,
}

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for TeeWriter {
    type Writer = TeeGuard;

    fn make_writer(&self) -> Self::Writer {
        TeeGuard {
            file: self.file.clone(),
            buffer: Vec::new(),
        }
    }
}

struct TeeGuard {
    file: std::sync::Arc<Mutex<std::fs::File>>,
    buffer: Vec<u8>,
}

impl Write for TeeGuard {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(&self.buffer);
            let _ = stderr.flush();
        }

        if let Ok(mut file) = self.file.lock() {
            let _ = file.write_all(&self.buffer);
            let _ = file.flush();
        }

        self.buffer.clear();

        Ok(())
    }
}

impl Drop for TeeGuard {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

/// Run an arbitrary command line through the shell (`spawn` keybind and
/// each `startup` entry), so pipes, quotes, `~`, and `$VARS` all work.
pub(crate) fn spawn_shell(cmdline: &str, socket: &OsStr) -> bool {
    spawn_wayland_client("sh", &["-c".to_string(), cmdline.to_string()], socket)
}

/// State dir for the welcome marker + generated tour script.
fn welcome_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("novawm");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(home).join(".local/state/novawm");
    }
    std::env::temp_dir().join("novawm")
}

fn welcome_marker() -> PathBuf {
    welcome_state_dir().join("welcomed")
}

fn welcome_seen() -> bool {
    welcome_marker().exists()
}

fn mark_welcome_seen() {
    let marker = welcome_marker();
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(marker, "1");
}

/// Show the welcome window: generate a tour script with the LIVE keybind
/// table and open it in the configured terminal. Bound to mod+F1 and shown
pub(crate) fn show_welcome(state: &Smallvil) {
    let Some(script) = write_welcome_script(&state.config_watcher.config) else {
        return;
    };
    let terminal = state.config_watcher.config.terminal.clone();
    let mut parts = terminal.split_whitespace();
    let binary = parts.next().unwrap_or("foot");
    let mut args: Vec<String> = parts.map(str::to_string).collect();
    args.push("sh".to_string());
    args.push(script.display().to_string());
    let spawned = spawn_wayland_client(binary, &args, &state.socket_name);
    life(format!("welcome: show {} result={spawned}", script.display()));
}

/// Generate the welcome tour script. Returns the script path, or None if
/// the state dir is not writable.
fn write_welcome_script(config: &config::Config) -> Option<PathBuf> {
    let dir = welcome_state_dir();
    if let Err(error) = std::fs::create_dir_all(&dir) {
        life(format!(
            "welcome: cannot create {}: {error}",
            dir.display()
        ));
        return None;
    }

    let mut rows = String::new();
    for kb in config.keybindings.iter().take(64) {
        if kb.is_mouse() {
            continue;
        }
        let mut combo = kb.modifiers.join("+");
        if !combo.is_empty() {
            combo.push('+');
        }
        combo.push_str(&kb.key);
        let mut action = kb.action.clone();
        if !kb.args.is_empty() {
            action.push(' ');
            action.push_str(&kb.args.join(" "));
        }
        rows.push_str(&format!("  {combo:<22} {action}\n"));
    }
    if rows.is_empty() {
        rows.push_str("  (no keybindings configured)\n");
    }

    let text = format!(
        r#"#!/bin/sh
clear 2>/dev/null || true
cat <<"NOVAWELCOME"
  +--------------------------------------------------------------+
  |  Welcome to NovaWM                                           |
  |  A scrollable-tiling Wayland compositor                      |
  +--------------------------------------------------------------+
  |  Windows live on a horizontal strip you scroll through.      |
  |  Each monitor keeps its own workspaces (1-9).                |
  |  mod+o opens the overview; mod+Return opens a terminal.      |
  |  Your config is config.kdl next to the NovaWM checkout;      |
  |  most settings reload live when you save.                    |
  +--------------------------------------------------------------+

  Your keybindings (live, from your config):

{rows}
  Tips:
    scroll the strip ......... mod+h / mod+l
    move a window ............ mod+shift+h / mod+shift+l
    columns .................. mod+comma eats a window into a column,
                               mod+period pops it back out
    scratchpad ............... mod+grave stashes / recalls a window
    help ..................... this window reopens with mod+F1

NOVAWELCOME
printf '\nPress Enter to close this window.\n'
read novawm_dummy
"#,
        rows = rows
    );

    let path = dir.join("novawm-welcome.sh");
    match std::fs::write(&path, text) {
        Ok(()) => Some(path),
        Err(error) => {
            life(format!("welcome: cannot write {}: {error}", path.display()));
            None
        }
    }
}


/// Push Wayland env into the systemd user bus and (re)start the portal stack
/// so Discord/OBS screencast works without a manual systemctl dance.
pub(crate) fn start_portal_stack(socket_name: &std::ffi::OsStr) {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/run/user/1000".into());
    let display = socket_name.to_string_lossy();
    unsafe {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            std::env::set_var("WAYLAND_DISPLAY", socket_name);
        }
        if std::env::var_os("XDG_CURRENT_DESKTOP").is_none() {
            std::env::set_var("XDG_CURRENT_DESKTOP", "wlroots");
        }
        if std::env::var_os("XDG_SESSION_TYPE").is_none() {
            std::env::set_var("XDG_SESSION_TYPE", "wayland");
        }
    }
    life(format!(
        "portal: priming systemd user env WAYLAND_DISPLAY={display} XDG_RUNTIME_DIR={runtime:?}"
    ));

    let run = |cmd: &str, args: &[&str]| {
        match std::process::Command::new(cmd).args(args).status() {
            Ok(status) if status.success() => life(format!("portal: {cmd} ok")),
            Ok(status) => life(format!("portal: {cmd} exited {status}")),
            Err(error) => life(format!("portal: {cmd} failed: {error}")),
        }
    };

    run(
        "systemctl",
        &[
            "--user",
            "import-environment",
            "WAYLAND_DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_TYPE",
            "XDG_RUNTIME_DIR",
        ],
    );
    run(
        "dbus-update-activation-environment",
        &[
            "--systemd",
            "WAYLAND_DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_TYPE",
            "XDG_RUNTIME_DIR",
        ],
    );
    run(
        "systemctl",
        &["--user", "reset-failed", "xdg-desktop-portal-wlr.service"],
    );
    run(
        "systemctl",
        &[
            "--user",
            "restart",
            "xdg-desktop-portal.service",
            "xdg-desktop-portal-wlr.service",
        ],
    );
}

pub(crate) fn spawn_client(state: &Smallvil) {
    life(format!(
        "config: cursor theme={:?} size={} | xkb layout={:?} options={:?} | welcome={} tap={} follows_mouse={}",
        state.config_watcher.config.xcursor_theme,
        state.config_watcher.config.xcursor_size,
        state.config_watcher.config.xkb_layout,
        state.config_watcher.config.xkb_options,
        state.config_watcher.config.welcome,
        state.config_watcher.config.touchpad_tap,
        state.config_watcher.config.focus_follows_mouse,
    ));
    let mut arguments = std::env::args().skip(1).peekable();

    if arguments.peek().map(String::as_str) == Some("drm") {
        arguments.next();
    }

    let flag = arguments.next();

    if matches!(flag.as_deref(), Some("-c") | Some("--command")) {
        if let Some(binary) = arguments.next() {
            let remaining: Vec<String> = arguments.collect();

            spawn_wayland_client(&binary, &remaining, &state.socket_name);

            return;
        }

        life("spawn_client: command flag has no executable");
    }

    let applications = state.config_watcher.config.startup_apps.clone();

    for application in applications {
        let cmdline = application.trim();
        if cmdline.is_empty() {
            continue;
        }
        spawn_shell(cmdline, &state.socket_name);
    }

    if state.config_watcher.config.welcome {
        if !welcome_seen() {
            mark_welcome_seen();
            show_welcome(state);
        } else {
            life(format!(
                "welcome: already shown before, skipping (delete {} to reshow)",
                welcome_marker().display()
            ));
        }
    } else {
        life("welcome: disabled by config (`welcome #false`)");
    }
}

/// Accepts &OsStr so callers can pass &OsString without lossy conversion.
pub(crate) fn spawn_wayland_client(bin: &str, args: &[String], socket_name: &OsStr) -> bool {
    let safe_name: String = bin
        .rsplit('/')
        .next()
        .unwrap_or(bin)
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'
                || character == '.'
            {
                character
            } else {
                '_'
            }
        })
        .collect();

    let client_log = log_paths()
        .life
        .parent()
        .unwrap_or(Path::new("/tmp"))
        .join(format!(
            "novawm-client-{safe_name}-{}.log",
            std::process::id(),
        ));

    let mut command = std::process::Command::new(bin);

    command.args(args).env("WAYLAND_DISPLAY", socket_name);

    if let Some(directory) = std::env::var_os("XDG_RUNTIME_DIR") {
        command.env("XDG_RUNTIME_DIR", directory);
    }

    if let Some(home) = std::env::var_os("HOME") {
        command.current_dir(home);
    }

    match open_appendable(&client_log) {
        Ok(stdout) => match stdout.try_clone() {
            Ok(stderr) => {
                command.stdout(std::process::Stdio::from(stdout));
                command.stderr(std::process::Stdio::from(stderr));
            }

            Err(error) => {
                life(format!("spawn: cannot clone log handle: {error}"));
            }
        },

        Err(error) => {
            life(format!(
                "spawn: cannot open {}: {error}",
                client_log.display(),
            ));
        }
    }

    match command.spawn() {
        Ok(mut child) => {
            life(format!(
                "spawn: {bin} args={args:?} pid={} \
                 WAYLAND_DISPLAY={socket_name:?} \
                 XDG_RUNTIME_DIR={:?} log={}",
                child.id(),
                std::env::var_os("XDG_RUNTIME_DIR"),
                client_log.display(),
            ));

            let binary = bin.to_string();
            let log = client_log.display().to_string();

            std::thread::spawn(move || match child.wait() {
                Ok(status) if status.success() => {
                    life(format!("spawn: {binary} exited normally"));
                }

                Ok(status) => {
                    life(format!(
                        "spawn: {binary} exited with \
                             {status:?}; check {log}"
                    ));
                }

                Err(error) => {
                    life(format!("spawn: {binary} wait failed: {error}"));
                }
            });

            true
        }

        Err(error) => {
            life(format!("spawn: {bin} FAILED: {error}"));

            false
        }
    }
}
