//! NovaWM control socket (`novactl`).
//!

use std::{
    io::{self, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    time::{Duration, Instant},
};

use smithay::{
    desktop::layer_map_for_output,
    reexports::{
        calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic},
        wayland_server::Resource,
    },
    wayland::{
        compositor::with_states,
        shell::{wlr_layer::Layer as WlrLayer, xdg::XdgToplevelSurfaceData},
    },
};

use crate::Smallvil;

const SOCKET_NAME: &str = "novawm-ipc.sock";

/// Resolve the IPC socket path, honouring `NOVAWM_IPC_SOCKET` and falling
/// back to `$XDG_RUNTIME_DIR/novawm-ipc.sock`.
fn socket_path() -> Result<PathBuf, io::Error> {
    if let Some(overridden) = std::env::var_os("NOVAWM_IPC_SOCKET") {
        if !overridden.is_empty() {
            return Ok(PathBuf::from(overridden));
        }
    }

    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR is not set"))?;

    if !runtime.is_dir() {
        return Err(io::Error::other(format!(
            "runtime directory {} does not exist",
            runtime.display(),
        )));
    }

    Ok(runtime.join(SOCKET_NAME))
}

/// Bind the control socket and register the accept source on the loop.
pub fn init_ipc(event_loop: &mut EventLoop<Smallvil>) -> io::Result<PathBuf> {
    let path = socket_path()?;

    let _ = std::fs::remove_file(&path);

    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;

    crate::life(format!("ipc: listening on {}", path.display()));

    let loop_handle = event_loop.handle();

    loop_handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, state| {
                while let Ok((stream, _)) = listener.accept() {
                    serve_ipc(state, stream);
                }
                Ok(PostAction::Continue)
            },
        )
        .expect("failed to register IPC listener");

    Ok(path)
}

/// Read one request line, answer it, and close the connection.
fn serve_ipc(state: &mut Smallvil, mut stream: UnixStream) {
    let _ = stream.set_nonblocking(true);

    let request = read_request(&mut stream);

    if let Some(request) = request {
        crate::life(format!("ipc: request {request:?}"));

        let response = state.ipc_command(&request);

        let _ = stream.set_nonblocking(false);
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }

    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// Collect up to one newline-terminated line without blocking the compositor
/// loop for long: poll non-blockingly for a short budget, then give up.
fn read_request(stream: &mut UnixStream) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(150);
    let mut buffer = Vec::with_capacity(128);
    let mut chunk = [0u8; 512];

    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.contains(&b'\n') {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(_) => break,
        }
    }

    let line = String::from_utf8_lossy(&buffer).trim().to_string();
    (!line.is_empty()).then_some(line)
}

/// The state half of the IPC protocol. Every command returns a response that
/// `novactl` prints verbatim; lines prefixed with `E ` signal an error.
impl Smallvil {
    pub fn ipc_command(&mut self, request: &str) -> String {
        let mut words = request.split_whitespace().peekable();
        let command = words.next().unwrap_or("");

        match command {
            "list-windows" => self.ipc_list_windows(),
            "monitors" => self.ipc_monitors(),
            "layers" => self.ipc_layers(),
            "layout" => self.ipc_layout(&mut words.map(str::to_string).collect()),
            "close" => self.ipc_close(&mut words.map(str::to_string).collect()),
            "screenshot" => self.ipc_screenshot(&mut words.map(str::to_string).collect()),
            "record" => self.ipc_record(&mut words.map(str::to_string).collect()),
            "stop" => {
                crate::life("ipc: stop requested");
                let _ = self.display_handle.flush_clients();
                self.loop_signal.stop();
                "ok stopping\n".to_string()
            }
            other => format!("E unknown command: {other}\n"),
        }
    }

    fn ipc_list_windows(&mut self) -> String {
        let focus = self.current_focus_surface();
        let elements: Vec<smithay::desktop::Window> = self.space.elements().cloned().collect();
        let mut lines = Vec::with_capacity(elements.len() + 1);
        lines.push(format!(
            "# index\tapp_id\ttitle\toutput\tworkspace\tfloating\tfocused\tgeometry"
        ));

        for (index, window) in elements.iter().enumerate() {
            let Some(toplevel) = window.toplevel() else {
                continue;
            };
            let id = toplevel.wl_surface().id();

            let (title, app_id) = with_states(toplevel.wl_surface(), |states| {
                let data = states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .expect("toplevel surface data must be present");
                let attributes = data.lock().unwrap();
                (attributes.title.clone(), attributes.app_id.clone())
            });

            let output = self
                .window_output
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "<none>".to_string());
            let workspace = self.window_workspace.get(&id).copied().unwrap_or(0);
            let floating = self.floating_windows.contains_key(&id);
            let focused = focus.as_ref() == Some(toplevel.wl_surface());
            let location = self.space.element_location(window).unwrap_or_default();
            let geometry = window.geometry();
            let rect = format!(
                "{} {},{},{}x{}",
                index,
                location.x,
                location.y,
                geometry.size.w,
                geometry.size.h,
            );

            lines.push(format!(
                "{index}\t{}\t{}\t{output}\t{workspace}\t{floating}\t{focused}\t{rect}",
                app_id.unwrap_or_default(),
                title.unwrap_or_default(),
            ));
        }

        lines.join("\n") + "\n"
    }

    fn ipc_monitors(&mut self) -> String {
        let outputs: Vec<smithay::output::Output> = self.space.outputs().cloned().collect();
        let mut lines = Vec::with_capacity(outputs.len() + 1);
        lines.push("# name\tx\ty\twidth\theight".to_string());

        for output in outputs {
            let name = output.name();
            match self.space.output_geometry(&output) {
                Some(geometry) => lines.push(format!(
                    "{name}\t{}\t{}\t{}\t{}",
                    geometry.loc.x, geometry.loc.y, geometry.size.w, geometry.size.h,
                )),
                None => lines.push(format!("{name}\t?")),
            }
        }

        lines.join("\n") + "\n"
    }

    fn ipc_layers(&mut self) -> String {
        let mut lines = vec!["# output\tnamespace\tlayer\tkeyboard_focus\tgeometry".to_string()];

        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let map = layer_map_for_output(&output);
            for layer in map.layers() {
                let layer_name = match layer.layer() {
                    WlrLayer::Background => "background",
                    WlrLayer::Bottom => "bottom",
                    WlrLayer::Top => "top",
                    WlrLayer::Overlay => "overlay",
                };
                let geometry = map
                    .layer_geometry(layer)
                    .map(|geo| {
                        format!("{},{} {}x{}", geo.loc.x, geo.loc.y, geo.size.w, geo.size.h)
                    })
                    .unwrap_or_else(|| "?".to_string());
                lines.push(format!(
                    "{}\t{}\t{layer_name}\t{}\t{geometry}",
                    output.name(),
                    layer.namespace(),
                    layer.can_receive_keyboard_focus(),
                ));
            }
        }

        lines.join("\n") + "\n"
    }

    fn ipc_layout(&mut self, args: &mut Vec<String>) -> String {
        match args.first().map(String::as_str) {
            Some("get") => format!("{}\n", self.config_watcher.config.layout),
            Some("set") => {
                let Some(target) = args.get(1) else {
                    return "E usage: layout set <scroll|dwindle|canvas>\n".to_string();
                };
                let normalized = match target.as_str() {
                    "scroll" | "scrolling" => "scroll",
                    "dwindle" => "dwindle",
                    "canvas" => "canvas",
                    other => {
                        return format!("E unknown layout: {other} (scroll|dwindle|canvas)\n");
                    }
                };
                self.config_watcher.config.layout = normalized.to_string();
                self.tile_resize = None;
                crate::life(format!("ipc: layout set {normalized}"));
                self.arrange_windows();
                format!("ok layout={normalized}\n")
            }
            _ => "E usage: layout get | layout set <scroll|dwindle|canvas>\n".to_string(),
        }
    }

    /// `close` - the focused window; `close <index>` - the window at that
    /// position in `space.elements()` order (as printed by `list-windows`).
    fn ipc_close(&mut self, args: &mut Vec<String>) -> String {
        let window = match args.first().map(String::as_str) {
            None => {
                let focus = self.current_focus_surface();
                self.space
                    .elements()
                    .find(|w| w.toplevel().map(|t| Some(t.wl_surface().clone()) == focus).unwrap_or(false))
                    .cloned()
            }
            Some(index) => {
                let index: usize = match index.parse() {
                    Ok(i) => i,
                    Err(_) => return "E close: index must be an integer\n".to_string(),
                };
                self.space.elements().cloned().nth(index)
            }
        };

        match window {
            Some(window) => {
                self.start_close(&window);
                format!("ok closing\n")
            }
            None => "E close: no matching window\n".to_string(),
        }
    }
}