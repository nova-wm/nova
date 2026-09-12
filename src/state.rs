use std::{ffi::OsString, sync::Arc};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output},
    input::{Seat, SeatState},
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Physical, Point, Rectangle},
    wayland::{
        background_effect::BackgroundEffectState,
        compositor::{CompositorClientState, CompositorState, with_states},
        fractional_scale::FractionalScaleManagerState,
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        session_lock::{LockSurface, SessionLockManagerState},
        image_capture_source::{ImageCaptureSourceState, OutputCaptureSourceState},
        image_copy_capture::{Frame, ImageCopyCaptureState, Session as CaptureSession},
        shell::{
            wlr_layer::{KeyboardInteractivity, Layer as WlrLayer, WlrLayerShellState},
            xdg::XdgShellState,
        },
        shm::ShmState,
        single_pixel_buffer::SinglePixelBufferState,
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
    },
};

use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::utils::Size;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct CubicBezier {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl CubicBezier {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn ease(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        let x = t;
        let mut t_approx = x;
        for _ in 0..8 {
            let x_calc = Self::sample_x(t_approx, self.x0, self.x1) - x;
            let dx = Self::sample_dx(t_approx, self.x0, self.x1);
            if dx.abs() < 1e-6 {
                break;
            }
            t_approx -= x_calc / dx;
            t_approx = t_approx.clamp(0.0, 1.0);
        }
        Self::sample_y(t_approx, self.y0, self.y1)
    }

    fn sample_x(t: f64, x0: f64, x1: f64) -> f64 {
        3.0 * (1.0 - t).powi(2) * t * x0 + 3.0 * (1.0 - t) * t.powi(2) * x1 + t.powi(3)
    }

    fn sample_y(t: f64, y0: f64, y1: f64) -> f64 {
        3.0 * (1.0 - t).powi(2) * t * y0 + 3.0 * (1.0 - t) * t.powi(2) * y1 + t.powi(3)
    }

    fn sample_dx(t: f64, x0: f64, x1: f64) -> f64 {
        3.0 * (1.0 - t).powi(2) * x0
            + 6.0 * (1.0 - t) * t * (x1 - x0)
            + 3.0 * t.powi(2) * (1.0 - x1)
    }
}

#[derive(Clone, Debug)]
pub struct WindowAnimationState {
    pub start_time: std::time::Instant,
    pub start_loc: Point<f64, Logical>,
    pub start_size: Size<f64, Logical>,
    pub start_alpha: f32,
    pub target_loc: Point<i32, Logical>,
    pub target_size: Size<i32, Logical>,
    pub target_alpha: f32,
    pub is_closing: bool,
}

/// A running workspace switch: the outgoing desktop (`old`) stays mapped and
/// slides out while the incoming desktop (`next`) slides in - the whole
#[derive(Debug, Clone)]
pub struct WsPan {
    pub output: String,
    pub old: usize,
    pub next: usize,
    pub start: std::time::Instant,
}

/// Overview camera transition: entering zooms the fitted grid OUT from the
/// plain 1:1 view (desktops shrink away as units), exiting zooms back IN to
#[derive(Debug, Clone)]
pub enum OverviewTransition {
    Entering { start: std::time::Instant },
    Exiting { start: std::time::Instant },
    Shifting {
        output: String,
        rows: i32,
        start: std::time::Instant,
    },
}

/// Canvas-mode camera for one output: the pan offset (screen pixels) and the
/// zoom factor. The canvas maps a window's stored rect to the screen as
#[derive(Debug, Clone, Copy)]
pub struct CanvasCamera {
    pub offset_x: f64,
    pub offset_y: f64,
    pub zoom: f64,
}

impl Default for CanvasCamera {
    fn default() -> Self {
        Self {
            offset_x: 0.0,
            offset_y: 0.0,
            zoom: 1.0,
        }
    }
}

impl CanvasCamera {
    /// Map a Space rect (raw canvas coordinates, zoom 1) to its on-screen
    /// rect for the output at `origin`: `screen = world * zoom + offset`.
    pub fn to_screen(
        self,
        origin: Point<i32, Logical>,
        space: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        let zoom = self.zoom.max(0.05);
        let sx = origin.x
            + ((space.loc.x - origin.x) as f64 * zoom + self.offset_x).round() as i32;
        let sy = origin.y
            + ((space.loc.y - origin.y) as f64 * zoom + self.offset_y).round() as i32;
        let sw = (space.size.w as f64 * zoom).round() as i32;
        let sh = (space.size.h as f64 * zoom).round() as i32;
        Rectangle::new((sx, sy).into(), (sw.max(1), sh.max(1)).into())
    }

    /// Inverse of [`to_screen`](Self::to_screen): the canvas rect that
    /// renders at `screen` (for maximized windows, which fill the view).
    pub fn to_world(
        self,
        origin: Point<i32, Logical>,
        screen: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        let zoom = self.zoom.max(0.05);
        let cx =
            origin.x + (((screen.loc.x - origin.x) as f64 - self.offset_x) / zoom).round() as i32;
        let cy =
            origin.y + (((screen.loc.y - origin.y) as f64 - self.offset_y) / zoom).round() as i32;
        let cw = (screen.size.w as f64 / zoom).round() as i32;
        let ch = (screen.size.h as f64 / zoom).round() as i32;
        Rectangle::new((cx, cy).into(), (cw.max(1), ch.max(1)).into())
    }

    /// Scale one output-local physical rect through the camera (render paths
    /// only): sizes scale by the zoom, locations additionally shift by the
    pub fn scale_physical(
        self,
        output_scale: f64,
        rect: Rectangle<i32, Physical>,
    ) -> Rectangle<i32, Physical> {
        let zoom = self.zoom.max(0.05);
        let sx = (rect.loc.x as f64 * zoom + self.offset_x * output_scale).round() as i32;
        let sy = (rect.loc.y as f64 * zoom + self.offset_y * output_scale).round() as i32;
        let sw = (rect.size.w as f64 * zoom).round() as i32;
        let sh = (rect.size.h as f64 * zoom).round() as i32;
        Rectangle::new((sx, sy).into(), (sw.max(1), sh.max(1)).into())
    }
}

/// Compositor session state (name kept from the original smithay smallvil
/// example; this is NovaWM's root state for all backends and protocols).
pub struct Smallvil {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,

    pub space: Space<Window>,
    pub loop_signal: LoopSignal,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Smallvil>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,
    pub background_effect_state: BackgroundEffectState,
    pub layer_shell_state: WlrLayerShellState,
    pub viewporter_state: ViewporterState,
    pub fractional_scale_manager_state: FractionalScaleManagerState,
    pub single_pixel_buffer_state: SinglePixelBufferState,
    pub idle_inhibit_manager_state: IdleInhibitManagerState,
    /// Surfaces currently inhibiting idle (media players, shell widgets).
    pub idle_inhibitors: std::collections::HashSet<ObjectId>,
    /// ext-idle-notify: hypridle / swayidle / qs timers listen here.
    pub idle_notifier_state: IdleNotifierState<Self>,
    /// ext-session-lock: swaylock / hyprlock / qs lock screens.
    pub session_lock_state: SessionLockManagerState,
    /// True while a session lock is active (input + render gated).
    pub session_locked: bool,
    /// Lock surfaces keyed by output name (one per output).
    pub lock_surfaces: HashMap<String, LockSurface>,
    /// ext-image-capture-source (opaque sources for screencast).
    pub image_capture_source_state: ImageCaptureSourceState,
    /// Output-scoped capture sources (xdg-desktop-portal / OBS).
    pub output_capture_source_state: OutputCaptureSourceState,
    /// ext-image-copy-capture manager.
    pub image_copy_capture_state: ImageCopyCaptureState,
    /// Live capture sessions (RAII - drop stops the session).
    pub capture_sessions: Vec<CaptureSession>,
    /// Frames waiting for the next DRM composite of `output` name.
    pub pending_capture_frames: Vec<(String, Frame)>,

    pub seat: Seat<Self>,

    pub config_watcher: crate::config::ConfigWatcher,
    pub animations: HashMap<ObjectId, WindowAnimationState>,
    pub pending_close: std::collections::HashSet<ObjectId>,
    pub last_spawn_time: Option<std::time::Instant>,
    pub last_close_time: Option<std::time::Instant>,
    pub last_ws_time: Option<std::time::Instant>,
    pub last_col_time: Option<std::time::Instant>,
    pub window_workspace: HashMap<ObjectId, usize>,
    pub all_windows: Vec<Window>,

    pub column_order: Vec<ObjectId>,

    /// Window ids that start a new scroll column. A window NOT in this set
    /// stacks vertically under its predecessor (niri-style); new windows are
    pub column_starts: std::collections::HashSet<ObjectId>,
    /// Remembered focus row per scroll column (keyed by the column's first
    /// window id), so horizontal paging lands on the last-used row and tall
    pub column_rows: HashMap<ObjectId, usize>,

    pub window_output: HashMap<ObjectId, String>,

    pub output_columns: HashMap<(usize, String), usize>,
    /// Active workspace per output (Niri-style: every monitor keeps its own
    /// independent stack of desktops; switching one never moves the others).
    pub active_workspaces: HashMap<String, usize>,
    /// Zoomed-out all-desktops overview (Super+o): every output fits all
    /// workspaces in a vertical strip through a fitted camera; the layout is frozen
    pub overview_active: bool,
    /// Workspace each output was on when the overview opened (Escape returns here).
    pub overview_entry: HashMap<String, usize>,
    /// Running workspace pan (directional switch gliding whole desktops).
    pub ws_pan: Option<WsPan>,
    /// Running overview camera zoom (enter/exit/shift), if any.
    pub overview_transition: Option<OverviewTransition>,
    pub workspace_column: HashMap<usize, usize>,
    pub scroll_active_idx: usize,
    pub scroll_view_origin: Point<i32, Logical>,
    pub cursor_position: Point<f64, Logical>,
    /// Persistent dwindle tree per output (keyed by output name), so the tree
    /// keeps its split structure between windows instead of being rebuilt on
    pub dwindle_roots: HashMap<String, crate::layout::DwindleNode>,
    /// Last output-local rect of a window that was closed, used to focus the
    /// remaining window nearest to it after a reflow.
    pub last_closed_rect: Option<Rectangle<i32, Logical>>,
    pub alt_tab_active: bool,
    pub alt_tab_index: usize,
    pub scratchpad_visible: bool,
    pub scratchpad_id: Option<ObjectId>,
    pub scratchpad_pending: bool,
    /// Windows maximized to fill their own output edge-to-edge (Super+F).
    /// Per-window (not a single global): multi-monitor / multi-workspace
    pub maximized_windows: std::collections::HashSet<ObjectId>,
    /// The window currently being moved by a pointer grab (Super+drag). While
    /// set, animations and layout must not counter-map it.
    pub dragging_window: Option<ObjectId>,
    /// Windows lifted out of the tiling layout (Super+v / Super+right-drag
    /// resize). Maps the window to the rect it floated at.
    pub floating_windows: HashMap<ObjectId, Rectangle<i32, Logical>>,
    /// Per-window opacity overrides from window rules (None = use config default).
    pub window_opacity: HashMap<ObjectId, f32>,
    /// Windows that already received a full window-rule pass (float/ws/center).
    pub window_rules_done: std::collections::HashSet<ObjectId>,
    /// Transient target rect during a live tile resize (Super+right-drag on a
    /// tiled window). While set, the scroll layout anchors the active column
    pub tile_resize: Option<(ObjectId, Rectangle<i32, Logical>)>,
    /// Persisted per-window scroll column widths (live resize in the scroll
    /// layout makes a column wider than the default half screen).
    pub scroll_widths: HashMap<ObjectId, i32>,
    /// Per-window rects on the infinite canvas (canvas-mode layout), in canvas
    /// coordinates at zoom 1 relative to the window's output origin.
    pub canvas_rects: HashMap<ObjectId, Rectangle<i32, Logical>>,
    /// Per-output canvas camera (pan offset + zoom) for canvas mode.
    pub canvas_camera: HashMap<String, CanvasCamera>,
    /// Pinned windows (Super+t in canvas mode): window id -> the SCREEN rect
    /// it keeps while the camera pans/zooms underneath it.
    pub canvas_pinned: HashMap<ObjectId, Rectangle<i32, Logical>>,
    /// Active canvas pan (Super+drag on empty space in canvas mode): the
    /// output being panned, the last cursor position, and the button that
    pub canvas_pan: Option<(String, Point<f64, Logical>, u32)>,
    /// Canvas camera gestures (pan/zoom) map windows directly instead of
    /// animating: positions track the camera 1:1 rather than lagging behind
    pub canvas_snap: bool,
    /// Bumped on every canvas camera gesture (zoom/pan/fit/reset). Folded into
    /// CanvasElement's commit so a camera-only change still damages the frame
    pub canvas_view_serial: u32,

    pub drm: Option<crate::drm::DrmState>,

    /// Interactive screenshot area-select (modal: input is swallowed).
    pub screenshot_select: Option<crate::screenshot::ScreenshotSelect>,
    /// Screenshot armed for the next frame of the named output.
    pub screenshot_capture: Option<crate::screenshot::ScreenshotCapture>,
    /// Last screenshot PNG bytes served to clipboard paste requests.
    pub clipboard_png: Option<Vec<u8>>,
    /// Interactive monitor picker for `novactl record` (modal input).
    pub record_picker: Option<crate::record::RecordPicker>,
    /// Active ffmpeg recording session, if any.
    pub record_session: Option<crate::record::RecordSession>,
}

impl Smallvil {
    pub fn new(event_loop: &mut EventLoop<'static, Self>, display: Display<Self>) -> Self {
        Self::build_both(event_loop, display, None)
    }

    pub fn new_with_drm(
        event_loop: &mut EventLoop<'static, Self>,
        display: Display<Self>,
        drm: crate::drm::DrmState,
    ) -> Self {
        Self::build_both(event_loop, display, Some(drm))
    }

    fn build_both(
        event_loop: &mut EventLoop<'static, Self>,
        display: Display<Self>,
        drm: Option<crate::drm::DrmState>,
    ) -> Self {
        let start_time = std::time::Instant::now();

        let dh = display.handle();


        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let popups = PopupManager::default();

        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);

        let data_device_state = DataDeviceState::new::<Self>(&dh);

        let background_effect_state = BackgroundEffectState::new::<Self>(&dh);

        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        let fractional_scale_manager_state = FractionalScaleManagerState::new::<Self>(&dh);
        let single_pixel_buffer_state = SinglePixelBufferState::new::<Self>(&dh);
        let idle_inhibit_manager_state = IdleInhibitManagerState::new::<Self>(&dh);
        let idle_notifier_state = IdleNotifierState::<Self>::new(&dh, event_loop.handle());
        let session_lock_state = SessionLockManagerState::new::<Self, _>(&dh, |_| true);
        let image_capture_source_state = ImageCaptureSourceState::new();
        let output_capture_source_state = OutputCaptureSourceState::new::<Self>(&dh);
        let image_copy_capture_state = ImageCopyCaptureState::new::<Self>(&dh);

        let config_watcher = crate::config::ConfigWatcher::new("config.kdl");

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "winit");

        let (xkb_rules, xkb_model, xkb_layout, xkb_variant, xkb_options) =
            config_watcher.config.xkb_parts();
        seat
            .add_keyboard(
                smithay::input::keyboard::XkbConfig {
                    rules: &xkb_rules,
                    model: &xkb_model,
                    layout: &xkb_layout,
                    variant: &xkb_variant,
                    options: xkb_options,
                },
                200,
                25,
            )
            .unwrap();

        seat.add_pointer();

        let space = Space::default();

        let socket_name = Self::init_wayland_listener(display, event_loop);

        let loop_signal = event_loop.get_signal();

        if let Err(error) = crate::ipc::init_ipc(event_loop) {
            crate::life(format!("ipc: failed to start: {error}"));
        }

        Self {
            start_time,
            display_handle: dh,

            space,
            loop_signal,
            socket_name,

            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups,
            background_effect_state,
            layer_shell_state,
            viewporter_state,
            fractional_scale_manager_state,
            single_pixel_buffer_state,
            idle_inhibit_manager_state,
            idle_inhibitors: std::collections::HashSet::new(),
            idle_notifier_state,
            session_lock_state,
            session_locked: false,
            lock_surfaces: HashMap::new(),
            image_capture_source_state,
            output_capture_source_state,
            image_copy_capture_state,
            capture_sessions: Vec::new(),
            pending_capture_frames: Vec::new(),
            seat,

            config_watcher,
            animations: HashMap::new(),
            pending_close: std::collections::HashSet::new(),
            last_spawn_time: None,
            last_close_time: None,
            last_ws_time: None,
            last_col_time: None,
            scroll_active_idx: 0,
            scroll_view_origin: Point::from((0, 0)),
            cursor_position: Point::from((0.0, 0.0)),
            dwindle_roots: HashMap::new(),
            last_closed_rect: None,
            window_workspace: HashMap::new(),
            all_windows: Vec::new(),
            column_order: Vec::new(),
            column_starts: std::collections::HashSet::new(),
            column_rows: HashMap::new(),
            window_output: HashMap::new(),
            output_columns: HashMap::new(),
            active_workspaces: HashMap::new(),
            overview_active: false,
            overview_entry: HashMap::new(),
            ws_pan: None,
            overview_transition: None,
            workspace_column: HashMap::new(),
            alt_tab_active: false,
            alt_tab_index: 0,
            scratchpad_visible: false,
            scratchpad_id: None,
            scratchpad_pending: false,
            maximized_windows: std::collections::HashSet::new(),
            dragging_window: None,
            floating_windows: HashMap::new(),
            window_opacity: HashMap::new(),
            window_rules_done: std::collections::HashSet::new(),
            tile_resize: None,
            scroll_widths: HashMap::new(),
            canvas_rects: HashMap::new(),
            canvas_camera: HashMap::new(),
            canvas_pinned: HashMap::new(),
            canvas_pan: None,
            canvas_snap: false,
            canvas_view_serial: 0,
            drm,
            screenshot_select: None,
            screenshot_capture: None,
            clipboard_png: None,
            record_picker: None,
            record_session: None,
        }
    }

    /// Access the DRM backend state when running on the DRM backend.
    pub fn drm(&self) -> Option<&crate::drm::DrmState> {
        self.drm.as_ref()
    }

    fn init_wayland_listener(
        display: Display<Smallvil>,
        event_loop: &mut EventLoop<'static, Self>,
    ) -> OsString {
        let listening_socket = ListeningSocketSource::new_auto().unwrap();

        let socket_name = listening_socket.socket_name().to_os_string();

        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .unwrap();
            })
            .expect("Failed to init the wayland event source.");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    unsafe {
                        display.get_mut().dispatch_clients(state).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        socket_name
    }

    /// Pick the output that logically "owns" a given point: the output whose
    /// geometry contains it, or failing that (point off every screen, or no
    pub fn output_under(
        &self,
        pos: Option<Point<f64, Logical>>,
    ) -> Option<smithay::output::Output> {
        if let Some(pos) = pos {
            if let Some(o) = self.space.outputs().find(|o| {
                self.space
                    .output_geometry(o)
                    .map(|g| g.to_f64().contains(pos))
                    .unwrap_or(false)
            }) {
                return Some(o.clone());
            }
        }

        let mut best: Option<(smithay::output::Output, f64)> = None;
        for o in self.space.outputs() {
            let Some(geo) = self.space.output_geometry(o) else {
                continue;
            };
            let center = Point::<f64, Logical>::from((
                geo.loc.x as f64 + geo.size.w as f64 / 2.0,
                geo.loc.y as f64 + geo.size.h as f64 / 2.0,
            ));
            let dist = match pos {
                Some(p) => {
                    let dx = center.x - p.x;
                    let dy = center.y - p.y;
                    (dx * dx + dy * dy).sqrt()
                }
                None => 0.0,
            };
            if best
                .as_ref()
                .map_or(true, |(_, best_dist)| dist < *best_dist)
            {
                best = Some((o.clone(), dist));
            }
        }
        best.map(|(o, _)| o)
    }


    /// True when `window` is allowed to receive pointer hits at `pos`.
    ///
    pub(crate) fn window_owned_at(&self, window: &Window, pos: Point<f64, Logical>) -> bool {
        let Some(toplevel) = window.toplevel() else {
            return false;
        };
        let id = toplevel.wl_surface().id();
        if self.dragging_window.as_ref() == Some(&id) {
            return true;
        }
        let Some(output) = self.output_under(Some(pos)) else {
            return true;
        };
        match self.window_output.get(&id) {
            Some(name) => name == &output.name(),
            None => true,
        }
    }

    /// True when `window` is the scratchpad while hidden: invisible,
    /// untiled, and untouchable (hit-testing must skip it too).
    pub fn is_hidden_scratchpad(&self, window: &Window) -> bool {
        if self.scratchpad_visible {
            return false;
        }
        window.toplevel().is_some_and(|t| {
            let id = t.wl_surface().id();
            self.scratchpad_id.as_ref() == Some(&id)
        })
    }

    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.session_locked {
            if let Some(output) = self.output_under(Some(pos)) {
                if let Some(lock) = self.lock_surface_for(&output) {
                    let geo = self.space.output_geometry(&output)?;
                    let local = pos - geo.loc.to_f64();
                    return Some((lock.wl_surface().clone(), local));
                }
            }
            return None;
        }

        if let Some(output) = self.space.outputs().find(|o| {
            self.space
                .output_geometry(o)
                .map(|g| g.to_f64().contains(pos))
                .unwrap_or(false)
        }) {
            let output_geo = self.space.output_geometry(output)?;
            let map = layer_map_for_output(output);
            let local = pos - output_geo.loc.to_f64();

            let upper = [WlrLayer::Overlay, WlrLayer::Top]
                .into_iter()
                .find_map(|tier| {
                    map.layers_on(tier).rev().find_map(|layer| {
                        let layer_loc = map.layer_geometry(layer)?.loc;
                        layer
                            .surface_under(local - layer_loc.to_f64(), WindowSurfaceType::ALL)
                            .map(|(surface, loc)| {
                                (surface, (loc + layer_loc + output_geo.loc).to_f64())
                            })
                    })
                });
            if upper.is_some() {
                return upper;
            }

            let win_pos = self.canvas_pick_pos(pos);
            let window_hit = self
                .space
                .element_under(win_pos)
                .filter(|(window, _)| {
                    !self.is_hidden_scratchpad(window) && self.window_owned_at(window, pos)
                })
                .and_then(|(window, location)| {
                window
                    .surface_under(win_pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(s, p)| (s, (p + location).to_f64()))
            });
            if window_hit.is_some() {
                return window_hit;
            }

            return [WlrLayer::Bottom, WlrLayer::Background]
                .into_iter()
                .find_map(|tier| {
                    map.layers_on(tier).rev().find_map(|layer| {
                        let layer_loc = map.layer_geometry(layer)?.loc;
                        layer
                            .surface_under(local - layer_loc.to_f64(), WindowSurfaceType::ALL)
                            .map(|(surface, loc)| {
                                (surface, (loc + layer_loc + output_geo.loc).to_f64())
                            })
                    })
                });
        }

        let win_pos = self.canvas_pick_pos(pos);
        self.space
            .element_under(win_pos)
            .filter(|(window, _)| {
                !self.is_hidden_scratchpad(window) && self.window_owned_at(window, pos)
            })
            .and_then(|(window, location)| {
                window
                    .surface_under(win_pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(s, p)| (s, (p + location).to_f64()))
            })
    }


    /// Refresh idle-notifier inhibition from the live inhibitor set. Called
    /// whenever inhibitors change and on input so hypridle/swayidle stay in
    pub fn sync_idle_inhibition(&mut self) {
        let inhibited = !self.idle_inhibitors.is_empty();
        self.idle_notifier_state.set_is_inhibited(inhibited);
    }

    /// Notify idle listeners that the user did something (any seat input).
    pub fn notify_idle_activity(&mut self) {
        self.sync_idle_inhibition();
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);
    }

    /// True while ext-session-lock owns the session.
    pub fn is_session_locked(&self) -> bool {
        self.session_locked
    }

    /// Lock surface for `output`, if the session is locked and one was created.
    pub fn lock_surface_for(&self, output: &smithay::output::Output) -> Option<&LockSurface> {
        if !self.session_locked {
            return None;
        }
        self.lock_surfaces.get(&output.name())
    }

    /// The mapped layer surface that currently captures all keyboard input:
    /// a Top/Overlay layer with Exclusive interactivity (a quickshell
    pub fn exclusive_keyboard_layer(&self) -> Option<WlSurface> {
        for layer in self.layer_shell_state.layer_surfaces().rev() {
            let exclusive = layer.with_cached_state(|data| {
                data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && (data.layer == WlrLayer::Top || data.layer == WlrLayer::Overlay)
            });
            if !exclusive {
                continue;
            }
            let mapped = self.space.outputs().find_map(|o| {
                let map = layer_map_for_output(o);
                map.layers()
                    .find(|l| l.layer_surface() == &layer)
                    .map(|l| l.wl_surface().clone())
            });
            if mapped.is_some() {
                return mapped;
            }
        }
        None
    }

    /// Keyboard-focusable layer surface under `pos`, if any: Overlay/Top
    /// when `upper` is set, Bottom/Background otherwise. Used to resolve
    pub fn focusable_layer_under(
        &self,
        pos: Point<f64, Logical>,
        upper: bool,
    ) -> Option<WlSurface> {
        let output = self.space.outputs().find(|o| {
            self.space
                .output_geometry(o)
                .map(|g| g.to_f64().contains(pos))
                .unwrap_or(false)
        })?;
        let output_geo = self.space.output_geometry(output)?;
        let map = layer_map_for_output(output);
        let local = pos - output_geo.loc.to_f64();
        let tiers: [WlrLayer; 2] = if upper {
            [WlrLayer::Overlay, WlrLayer::Top]
        } else {
            [WlrLayer::Bottom, WlrLayer::Background]
        };
        let mut hit = None;
        for tier in tiers {
            hit = map.layers_on(tier).rev().find_map(|layer| {
                if !layer.can_receive_keyboard_focus() {
                    return None;
                }
                let layer_loc = map.layer_geometry(layer)?.loc;
                layer
                    .surface_under(local - layer_loc.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, _)| surface)
            });
            if hit.is_some() {
                break;
            }
        }
        hit
    }

    /// True when any mapped Overlay/Top layer covers `pos`, regardless of
    /// keyboard interactivity. Windows underneath must not steal focus.
    pub fn upper_layer_at(&self, pos: Point<f64, Logical>) -> bool {
        let Some(output) = self.space.outputs().find(|o| {
            self.space
                .output_geometry(o)
                .map(|g| g.to_f64().contains(pos))
                .unwrap_or(false)
        }) else {
            return false;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            return false;
        };
        let map = layer_map_for_output(output);
        let local = pos - output_geo.loc.to_f64();
        map.layer_under(WlrLayer::Overlay, local)
            .or_else(|| map.layer_under(WlrLayer::Top, local))
            .is_some()
    }

    /// Compact diagnostic of the layer stack covering `pos` for click logs.
    pub fn debug_layers_at(&self, pos: Point<f64, Logical>) -> String {
        let mut out = String::new();
        for output in self.space.outputs() {
            let Some(geo) = self.space.output_geometry(output) else {
                continue;
            };
            if !geo.to_f64().contains(pos) {
                continue;
            }
            let map = layer_map_for_output(output);
            let local = pos - geo.loc.to_f64();
            for layer in map.layers() {
                let lgeo = map.layer_geometry(layer);
                let hit = lgeo.map(|g| g.to_f64().contains(local)).unwrap_or(false);
                out.push_str(&format!(
                    " [{} {:?} {:?} hit={hit}]",
                    layer.namespace(),
                    layer.layer(),
                    lgeo.map(|g| (g.loc.x, g.loc.y, g.size.w, g.size.h)),
                ));
            }
        }
        if out.is_empty() {
            out.push_str(" [no layers]");
        }
        out
    }

    /// Window whose output-local rect contains `pos`.
    ///
    pub(crate) fn window_at(&self, pos: Point<f64, Logical>) -> Option<Window> {
        let is_dragged = |w: &Window| {
            w.toplevel()
                .map(|t| self.dragging_window.as_ref() == Some(&t.wl_surface().id()))
                .unwrap_or(false)
        };

        if let Some((window, _)) = self.space.element_under(pos) {
            if !is_dragged(window) && self.window_owned_at(window, pos) {
                return Some(window.clone());
            }
        }

        let px = pos.x as i32;
        let py = pos.y as i32;
        self.space.elements().find_map(|window| {
            if is_dragged(window) || !self.window_owned_at(window, pos) {
                return None;
            }
            let loc = self.space.element_location(window)?;
            let geo = self
                .space
                .element_geometry(window)
                .map(|g| g.size)
                .unwrap_or_else(|| window.geometry().size);
            if px >= loc.x && px < loc.x + geo.w && py >= loc.y && py < loc.y + geo.h {
                Some(window.clone())
            } else {
                None
            }
        })
    }

    /// Reset the animation origin of every tracked window to its current
    /// on-screen position, and restart its timer.
    fn rebase_animations(&mut self) {
        let config = self.config_watcher.config.clone();
        let duration_ms = if config.animation_enabled {
            config.animation_duration
        } else {
            0.0
        };
        let bezier = CubicBezier::new(
            config.animation_bezier.0,
            config.animation_bezier.1,
            config.animation_bezier.2,
            config.animation_bezier.3,
        );

        let windows: Vec<Window> = self.space.elements().cloned().collect();
        for window in &windows {
            let id = window.toplevel().unwrap().wl_surface().id();
            if let Some(anim) = self.animations.get_mut(&id) {
                let anim_duration = if anim.is_closing {
                    duration_ms * 0.4
                } else {
                    duration_ms
                };
                let elapsed = anim.start_time.elapsed().as_secs_f64() * 1000.0;
                let t = if anim_duration <= 0.0 {
                    1.0
                } else {
                    (elapsed / anim_duration).min(1.0)
                };
                let eased = bezier.ease(t);

                let current_loc: Point<f64, Logical> = Point::from((
                    anim.start_loc.x + (anim.target_loc.x as f64 - anim.start_loc.x) * eased,
                    anim.start_loc.y + (anim.target_loc.y as f64 - anim.start_loc.y) * eased,
                ));
                let current_size: Size<f64, Logical> = Size::from((
                    anim.start_size.w + (anim.target_size.w as f64 - anim.start_size.w) * eased,
                    anim.start_size.h + (anim.target_size.h as f64 - anim.start_size.h) * eased,
                ));

                anim.start_time = std::time::Instant::now();
                anim.start_loc = current_loc;
                anim.start_size = current_size;
            }
        }
    }

    /// Incorporate newly mapped windows without changing existing column order.
    ///
    fn reconcile_column_order(&mut self) {
        let fallback_output = self
            .output_under(Some(self.cursor_position))
            .map(|output| output.name());

        let mut known = self.all_windows.clone();

        for window in self.space.elements() {
            let Some(toplevel) = window.toplevel() else {
                continue;
            };

            let id = toplevel.wl_surface().id();

            if !known.iter().any(|candidate| {
                candidate
                    .toplevel()
                    .map(|t| t.wl_surface().id() == id)
                    .unwrap_or(false)
            }) {
                known.push(window.clone());
            }
        }

        let mut live = std::collections::HashSet::new();

        for window in &known {
            let Some(toplevel) = window.toplevel() else {
                continue;
            };

            if !toplevel.wl_surface().is_alive() {
                continue;
            }

            let id = toplevel.wl_surface().id();
            live.insert(id.clone());

            if !self.column_order.contains(&id) {
                self.column_order.push(id.clone());
                self.column_starts.insert(id.clone());
            }

            let here = self
                .window_output
                .get(&id)
                .cloned()
                .or_else(|| fallback_output.clone());
            let ws = here
                .map(|name| self.active_workspace_on(&name))
                .unwrap_or(0);
            self.window_workspace
                .entry(id.clone())
                .or_insert(ws);

            if let Some(name) = &fallback_output {
                self.window_output.entry(id).or_insert_with(|| name.clone());
            }
        }

        self.column_order.retain(|id| live.contains(id));
        self.column_starts.retain(|id| live.contains(id));
        self.column_rows.retain(|id, _| live.contains(id));
        self.window_output.retain(|id, _| live.contains(id));

        let available: Vec<String> = self.space.outputs().map(|output| output.name()).collect();

        if let Some(fallback) = fallback_output {
            for name in self.window_output.values_mut() {
                if !available.contains(name) {
                    *name = fallback.clone();
                }
            }
        }
    }

    /// Mapped windows on one output, in persistent column order.
    pub(crate) fn ordered_windows_on(&self, output: &smithay::output::Output) -> Vec<Window> {
        let name = output.name();
        let active_here = self.active_workspace_on(&name);
        let mapped: Vec<Window> = self.space.elements().cloned().collect();

        self.column_order
            .iter()
            .filter_map(|id| {
                if self.window_output.get(id) != Some(&name) {
                    return None;
                }
                if self.window_workspace.get(id) != Some(&active_here) {
                    return None;
                }
                if self.floating_windows.contains_key(id) {
                    return None;
                }
                if self.scratchpad_id.as_ref().is_some_and(|s| s == id)
                    && !self.scratchpad_visible
                {
                    return None;
                }

                mapped
                    .iter()
                    .find(|window| {
                        window
                            .toplevel()
                            .map(|t| t.wl_surface().id() == *id)
                            .unwrap_or(false)
                    })
                    .cloned()
            })
            .collect()
    }

    /// Split this output/workspace's flat window list into scroll columns:
    /// every window in `column_starts` (plus always the first) begins a new
    fn columns_of(&self, windows: &[Window]) -> Vec<Vec<Window>> {
        let mut columns: Vec<Vec<Window>> = Vec::new();

        for window in windows {
            let id = window.toplevel().map(|t| t.wl_surface().id());
            let starts = id
                .as_ref()
                .is_some_and(|id| self.column_starts.contains(id));

            if starts || columns.is_empty() {
                columns.push(Vec::new());
            }

            columns.last_mut().unwrap().push(window.clone());
        }

        columns
    }

    /// First window id of the scroll column holding `id` (its own output).
    /// Column widths and remembered rows are keyed by this anchor.
    pub(crate) fn column_anchor_for(&self, id: &ObjectId) -> Option<ObjectId> {
        let name = self.window_output.get(id)?.clone();
        let output = self.space.outputs().find(|o| o.name() == name)?;
        let windows = self.ordered_windows_on(output);

        for column in self.columns_of(&windows) {
            if column
                .iter()
                .any(|w| w.toplevel().is_some_and(|t| t.wl_surface().id() == *id))
            {
                return column
                    .first()
                    .and_then(|w| w.toplevel().map(|t| t.wl_surface().id()));
            }
        }

        None
    }

    /// (column, row) of the focused window within `columns`, if it is there.
    fn focused_column_position(&self, columns: &[Vec<Window>]) -> Option<(usize, usize)> {
        let focus = self.current_focus_surface()?;

        columns.iter().enumerate().find_map(|(col, column)| {
            column
                .iter()
                .position(|w| {
                    Some(w.toplevel().unwrap().wl_surface().clone()) == Some(focus.clone())
                })
                .map(|row| (col, row))
        })
    }

    /// Row to land on when paging onto `column`: its remembered row.
    fn landing_row(&self, column: &[Window]) -> usize {
        column
            .first()
            .and_then(|w| w.toplevel().map(|t| t.wl_surface().id()))
            .and_then(|id| self.column_rows.get(&id).copied())
            .unwrap_or(0)
            .min(column.len().saturating_sub(1))
    }

    fn input_output(&self) -> Option<smithay::output::Output> {
        self.output_under(Some(self.cursor_position))
    }

    /// Stable order used by column navigation.
    fn input_columns(&self) -> Vec<Window> {
        self.input_output()
            .map(|output| self.ordered_windows_on(&output))
            .unwrap_or_default()
    }

    pub(crate) fn remember_column(&mut self, output: &smithay::output::Output, index: usize) {
        let active_here = self.active_workspace_on(&output.name());
        self.output_columns
            .insert((active_here, output.name()), index);

        self.scroll_active_idx = index;
        self.workspace_column.insert(active_here, index);
    }

    /// Rendering uses stacking order, but only windows owned by this output.
    ///
    pub(crate) fn render_windows_on(&self, output: &smithay::output::Output) -> Vec<Window> {
        let name = output.name();

        let dragged_is_here = self.dragging_window.is_some()
            && self
                .output_under(Some(self.cursor_position))
                .map(|o| o.name() == name)
                .unwrap_or(false);

        self.space
            .elements()
            .filter(|window| {
                window
                    .toplevel()
                    .map(|toplevel| {
                        let id = &toplevel.wl_surface().id();
                        if self.scratchpad_id.as_ref().is_some_and(|s| s == id)
                            && !self.scratchpad_visible
                        {
                            return false;
                        }
                        self.window_output.get(id) == Some(&name)
                            || (dragged_is_here
                                && self.dragging_window.as_ref() == Some(id))
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    }

    /// Look up (or default) the canvas camera for one output.
    pub fn canvas_camera_for(&self, output_name: &str) -> CanvasCamera {
        self.canvas_camera
            .get(output_name)
            .copied()
            .unwrap_or_default()
    }

    /// Map a screen point to Space coordinates for WINDOW hit-testing in
    /// canvas mode (inverse camera). Identity outside canvas mode and for
    pub fn canvas_pick_pos(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        if self.overview_active {
            return self.overview_pick_pos(pos);
        }

        if self.config_watcher.config.layout != "canvas" {
            return pos;
        }
        let Some(output) = self.output_under(Some(pos)) else {
            return pos;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return pos;
        };
        let cam = self.canvas_camera_for(&output.name());
        let zoom = cam.zoom.max(0.05);
        let lx = pos.x - geo.loc.x as f64;
        let ly = pos.y - geo.loc.y as f64;
        Point::from((
            geo.loc.x as f64 + (lx - cam.offset_x) / zoom,
            geo.loc.y as f64 + (ly - cam.offset_y) / zoom,
        ))
    }

    /// Owner-output zoom for one window (grab deltas arrive in screen pixels
    /// and must be divided by this in canvas mode); 1 outside canvas mode.
    pub fn canvas_zoom_for_window(&self, window: &Window) -> f64 {
        if self.config_watcher.config.layout != "canvas" {
            return 1.0;
        }
        window
            .toplevel()
            .map(|t| t.wl_surface().id())
            .and_then(|id| self.window_output.get(&id))
            .map(|name| self.canvas_camera_for(name).zoom)
            .unwrap_or(1.0)
            .max(0.05)
    }

    /// Canvas-mode layout: each window keeps a free-floating rect in canvas
    /// coordinates (zoom 1, relative to its output origin); the camera (pan
    fn canvas_layout_for(
        &mut self,
        output: &smithay::output::Output,
        output_geo: Rectangle<i32, Logical>,
        windows: &[Window],
    ) -> Vec<(Window, Rectangle<i32, Logical>)> {
        let name = output.name();
        let cam = self.canvas_camera_for(&name);
        let zoom = cam.zoom.max(0.05);

        let mut out = Vec::with_capacity(windows.len());
        for (index, window) in windows.iter().enumerate() {
            let Some(toplevel) = window.toplevel() else {
                continue;
            };
            let id = toplevel.wl_surface().id();
            let world = match self.canvas_rects.get(&id).copied() {
                Some(rect) => rect,
                None => {
                    let view_w = (output_geo.size.w as f64 / zoom).round() as i32;
                    let view_h = (output_geo.size.h as f64 / zoom).round() as i32;
                    let ww = view_w.min(900).max(200);
                    let wh = view_h.min(620).max(150);
                    let cascade = (index as i32 % 8) * 36;
                    let cx = ((output_geo.size.w as f64 / 2.0 - cam.offset_x) / zoom).round()
                        as i32
                        - ww / 2
                        + cascade;
                    let cy = ((output_geo.size.h as f64 / 2.0 - cam.offset_y) / zoom).round()
                        as i32
                        - wh / 2
                        + cascade;
                    let rect = Rectangle::new((cx, cy).into(), (ww, wh).into());
                    self.canvas_rects.insert(id.clone(), rect);
                    rect
                }
            };
            out.push((
                window.clone(),
                Rectangle::new(
                    (output_geo.loc.x + world.loc.x, output_geo.loc.y + world.loc.y).into(),
                    (world.size.w.max(1), world.size.h.max(1)).into(),
                ),
            ));
        }
        out
    }

    fn bump_canvas_view(&mut self) {
        self.canvas_view_serial = self.canvas_view_serial.wrapping_add(1);
    }

    /// Zoom the canvas on `output` by `factor`, keeping the canvas point under
    /// `cursor` (output-local logical pixels) pinned.
    pub fn canvas_zoom_at(
        &mut self,
        output: &smithay::output::Output,
        cursor: Point<f64, Logical>,
        factor: f64,
    ) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let name = output.name();
        let mut cam = self.canvas_camera_for(&name);
        let old = cam.zoom.max(0.05);
        let new = (old * factor).clamp(0.15, 4.0);
        if (new - old).abs() < f64::EPSILON {
            return;
        }
        let k = new / old;
        cam.offset_x = cursor.x - (cursor.x - cam.offset_x) * k;
        cam.offset_y = cursor.y - (cursor.y - cam.offset_y) * k;
        cam.zoom = new;
        self.canvas_camera.insert(name, cam);
        self.bump_canvas_view();
        self.repin_windows(output);
        self.canvas_snap = true;
        self.arrange_windows();
        self.canvas_snap = false;
    }

    /// Zoom the canvas under the cursor by `factor` (Mod+scroll, keybinds).
    pub fn canvas_zoom_by(&mut self, factor: f64) {
        let Some(output) = self.output_under(Some(self.cursor_position)) else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };
        let cursor: Point<f64, Logical> = Point::from((
            self.cursor_position.x - geo.loc.x as f64,
            self.cursor_position.y - geo.loc.y as f64,
        ));
        self.canvas_zoom_at(&output, cursor, factor);
    }

    /// Pan the canvas under the cursor by screen pixels.
    pub fn canvas_pan_by(&mut self, dx: f64, dy: f64) {
        let Some(output) = self.output_under(Some(self.cursor_position)) else {
            return;
        };
        let name = output.name();
        let mut cam = self.canvas_camera_for(&name);
        cam.offset_x = (cam.offset_x + dx).clamp(-20000.0, 20000.0);
        cam.offset_y = (cam.offset_y + dy).clamp(-20000.0, 20000.0);
        self.canvas_camera.insert(name, cam);
        self.bump_canvas_view();
        self.repin_windows(&output);
        self.canvas_snap = true;
        self.arrange_windows();
        self.canvas_snap = false;
    }

    /// Reset one output's canvas view (zoom 1, no pan).
    pub fn canvas_reset_view(&mut self, output: &smithay::output::Output) {
        self.canvas_camera
            .insert(output.name(), CanvasCamera::default());
        self.bump_canvas_view();
        self.repin_windows(output);
        self.arrange_windows();
    }

    /// Store a window's current on-screen geometry back into canvas
    /// coordinates (drop / resize landing in canvas mode).
    pub fn canvas_center_focused(&mut self) {
        if self.config_watcher.config.layout != "canvas" {
            return;
        }

        let Some(surface) = self.current_focus_surface() else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == &surface)
            .cloned()
        else {
            return;
        };

        let (Some(loc), Some(geo)) = (
            self.space.element_location(&window),
            self.space.element_geometry(&window),
        ) else {
            return;
        };
        let id = window.toplevel().unwrap().wl_surface().id();
        let output_name = self.window_output.get(&id).cloned().or_else(|| {
            self.output_under(Some(self.cursor_position))
                .map(|o| o.name())
        });
        let Some(output_name) = output_name else {
            return;
        };
        let Some(output) = self
            .space
            .outputs()
            .find(|o| o.name() == output_name)
            .cloned()
        else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(&output) else {
            return;
        };

        let mut cam = self.canvas_camera_for(&output_name);
        let zoom = cam.zoom.max(0.05);
        let center_x = (loc.x - output_geo.loc.x) as f64 + geo.size.w as f64 / 2.0;
        let center_y = (loc.y - output_geo.loc.y) as f64 + geo.size.h as f64 / 2.0;
        cam.offset_x = output_geo.size.w as f64 / 2.0 - center_x * zoom;
        cam.offset_y = output_geo.size.h as f64 / 2.0 - center_y * zoom;
        self.canvas_camera.insert(output_name, cam);
        self.bump_canvas_view();
        self.repin_windows(&output);
        self.canvas_snap = true;
        self.arrange_windows();
        self.canvas_snap = false;
    }

    /// Zoom the view to fit every window on the cursor's output (drift
    /// `zoom-to-fit`). Canvas mode only; zooms out at most to 1:1.
    pub fn canvas_zoom_to_fit(&mut self) {
        if self.config_watcher.config.layout != "canvas" {
            return;
        }

        let Some(output) = self.output_under(Some(self.cursor_position)) else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };

        let name = output.name();
        let (mut min_x, mut min_y) = (i32::MAX, i32::MAX);
        let (mut max_x, mut max_y) = (i32::MIN, i32::MIN);
        let mut any = false;

        for window in self.space.elements() {
            let same_output = window.toplevel().is_some_and(|t| {
                self.window_output
                    .get(&t.wl_surface().id())
                    .map(String::as_str)
                    == Some(name.as_str())
            });
            if !same_output {
                continue;
            }
            let (Some(loc), Some(window_geo)) = (
                self.space.element_location(window),
                self.space.element_geometry(window),
            ) else {
                continue;
            };
            any = true;
            min_x = min_x.min(loc.x);
            min_y = min_y.min(loc.y);
            max_x = max_x.max(loc.x + window_geo.size.w);
            max_y = max_y.max(loc.y + window_geo.size.h);
        }

        if !any {
            return;
        }

        let bounds_w = (max_x - min_x).max(1) as f64;
        let bounds_h = (max_y - min_y).max(1) as f64;
        let zoom = (geo.size.w as f64 / bounds_w)
            .min(geo.size.h as f64 / bounds_h)
            .min(1.0)
            .max(0.15);

        let mut cam = self.canvas_camera_for(&name);
        cam.zoom = zoom;
        cam.offset_x =
            geo.size.w as f64 / 2.0 - ((min_x - geo.loc.x) as f64 + bounds_w / 2.0) * zoom;
        cam.offset_y =
            geo.size.h as f64 / 2.0 - ((min_y - geo.loc.y) as f64 + bounds_h / 2.0) * zoom;
        self.canvas_camera.insert(name, cam);
        self.bump_canvas_view();
        self.repin_windows(&output);
        self.canvas_snap = true;
        self.arrange_windows();
        self.canvas_snap = false;
    }

    /// Nudge the focused window across the canvas by canvas pixels (drift
    /// `nudge-window`). Canvas mode only; nudging unpins a pinned window.
    pub fn canvas_nudge_focused(&mut self, dx: i32, dy: i32) {
        if self.config_watcher.config.layout != "canvas" {
            return;
        }

        let Some(surface) = self.current_focus_surface() else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == &surface)
            .cloned()
        else {
            return;
        };
        let id = window.toplevel().unwrap().wl_surface().id();

        self.canvas_pinned.remove(&id);

        if let Some(rect) = self.floating_windows.get_mut(&id) {
            rect.loc.x += dx;
            rect.loc.y += dy;
        } else if let Some(rect) = self.canvas_rects.get_mut(&id) {
            rect.loc.x += dx;
            rect.loc.y += dy;
        } else {
            return;
        }

        self.canvas_snap = true;
        self.arrange_windows();
        self.canvas_snap = false;
    }

    /// Reset the cursor output's canvas view (drift `home`). Canvas only.
    pub fn canvas_home(&mut self) {
        if self.config_watcher.config.layout != "canvas" {
            return;
        }

        let Some(output) = self.output_under(Some(self.cursor_position)) else {
            return;
        };
        self.canvas_reset_view(&output);
    }

    /// Pin/unpin the focused window to its current screen spot (drift
    /// `toggle-pin-to-screen`): while pinned, pan/zoom slide the canvas
    pub fn toggle_pin_focused(&mut self) {
        if self.config_watcher.config.layout != "canvas" {
            return;
        }

        let Some(surface) = self.current_focus_surface() else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == &surface)
            .cloned()
        else {
            return;
        };
        let id = window.toplevel().unwrap().wl_surface().id();

        if self.canvas_pinned.remove(&id).is_some() {
            return;
        }

        let (Some(loc), Some(geo)) = (
            self.space.element_location(&window),
            self.space.element_geometry(&window),
        ) else {
            return;
        };
        let output_name = self.window_output.get(&id).cloned().or_else(|| {
            self.output_under(Some(self.cursor_position))
                .map(|o| o.name())
        });
        let Some(output) = output_name
            .and_then(|name| self.space.outputs().find(|o| o.name() == name).cloned())
        else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(&output) else {
            return;
        };

        let cam = self.canvas_camera_for(&output.name());
        let screen = cam.to_screen(output_geo.loc, Rectangle::new(loc, geo.size));
        self.canvas_pinned.insert(id, screen);
    }

    /// Re-anchor pinned windows of `output` after a camera change so their
    /// screen rects stay put while the canvas slides underneath.
    fn repin_windows(&mut self, output: &smithay::output::Output) {
        if self.canvas_pinned.is_empty() {
            return;
        }

        let Some(geo) = self.space.output_geometry(output) else {
            return;
        };
        let cam = self.canvas_camera_for(&output.name());

        let pinned: Vec<(ObjectId, Rectangle<i32, Logical>)> =
            self.canvas_pinned.iter().map(|(id, rect)| (id.clone(), *rect)).collect();

        for (id, screen) in pinned {
            if self.window_output.get(&id).map(String::as_str) != Some(output.name().as_str()) {
                continue;
            }
            let world = cam.to_world(geo.loc, screen);
            if let Some(rect) = self.floating_windows.get_mut(&id) {
                *rect = world;
            } else {
                self.canvas_rects.insert(
                    id,
                    Rectangle::new(
                        (world.loc.x - geo.loc.x, world.loc.y - geo.loc.y).into(),
                        world.size,
                    ),
                );
            }
        }
    }

    pub fn canvas_save_rect(&mut self, window: &Window, id: &ObjectId) {
        let Some(loc) = self.space.element_location(window) else {
            return;
        };
        let Some(geo) = self.space.element_geometry(window) else {
            return;
        };
        self.canvas_save_rect_at(
            window,
            id,
            Rectangle::new(loc, geo.size),
        );
    }

    /// Persist a free-resize/move rect into canvas coordinates immediately
    /// (used mid-grab so LEFT/TOP origin updates stick before commit).
    pub fn canvas_save_rect_at(
        &mut self,
        _window: &Window,
        id: &ObjectId,
        space_rect: Rectangle<i32, Logical>,
    ) {
        let output_name = self.window_output.get(id).cloned().or_else(|| {
            self.output_under(Some(self.cursor_position))
                .map(|o| o.name())
        });
        let Some(output_name) = output_name else {
            return;
        };
        let origin = self
            .space
            .outputs()
            .find(|o| o.name() == output_name)
            .and_then(|o| self.space.output_geometry(o))
            .map(|g| g.loc)
            .unwrap_or_default();
        let cx = space_rect.loc.x - origin.x;
        let cy = space_rect.loc.y - origin.y;
        self.canvas_rects.insert(
            id.clone(),
            Rectangle::new(
                (cx, cy).into(),
                (space_rect.size.w.max(1), space_rect.size.h.max(1)).into(),
            ),
        );

        self.canvas_pinned.remove(id);
    }

    /// Re-apply libinput device settings after a config reload. (XKB is
    /// startup-only; the cursor theme reloads lazily in the render loop.)
    fn reapply_input_config(&mut self) {
        let config = self.config_watcher.config.clone();
        if let Some(drm) = self.drm.as_mut() {
            for device in drm.pointer_devices.iter_mut() {
                crate::drm::apply_libinput_config(device, &config);
            }
        }
    }

    pub fn arrange_windows(&mut self) {
        if self.overview_active {
            return;
        }

        if self.config_watcher.check_and_reload() {
            self.reapply_input_config();
        }
        self.reconcile_column_order();

        let config = self.config_watcher.config.clone();
        self.rebase_animations();

        let outputs: Vec<_> = self.space.outputs().cloned().collect();

        let input_output_name = self.input_output().map(|output| output.name());

        let mut layouts = Vec::new();

        for output in outputs {
            let Some(output_geo) = self.space.output_geometry(&output) else {
                continue;
            };

            let windows = self.ordered_windows_on(&output);

            if windows.is_empty() {
                continue;
            }

            let outer = config
                .outer_gap
                .max(0)
                .min(((output_geo.size.w.min(output_geo.size.h) - 1) / 2).max(0));

            let screen_rect = Rectangle::new(
                (output_geo.loc.x + outer, output_geo.loc.y + outer).into(),
                (
                    (output_geo.size.w - 2 * outer).max(1),
                    (output_geo.size.h - 2 * outer).max(1),
                )
                    .into(),
            );

            let screen_rect = {
                let zone = layer_map_for_output(&output).non_exclusive_zone();
                let zone_global = Rectangle::new(output_geo.loc + zone.loc, zone.size);
                screen_rect.intersection(zone_global).unwrap_or(screen_rect)
            };

            if config.layout == "scroll" {
                let key = (
                    self.active_workspace_on(&output.name()),
                    output.name(),
                );

                let columns = self.columns_of(&windows);

                let remembered = self.output_columns.get(&key).copied().unwrap_or(0);

                let active = self
                    .focused_column_position(&columns)
                    .map(|(col, _)| col)
                    .unwrap_or(remembered)
                    .min(columns.len() - 1);

                self.output_columns.insert(key, active);

                let maximized_on_output: Option<ObjectId> = self
                    .maximized_windows
                    .iter()
                    .find(|mid| {
                        self.window_output.get(*mid).map(|n| *n == output.name()).unwrap_or(false)
                            && columns.iter().any(|column| {
                                column.iter().any(|w| {
                                    w.toplevel().is_some_and(|t| t.wl_surface().id() == **mid)
                                })
                            })
                    })
                    .cloned();

                let maximized_ref = maximized_on_output.as_ref().and_then(|mid| {
                    columns.iter().find_map(|column| {
                        if column
                            .iter()
                            .any(|w| w.toplevel().is_some_and(|t| t.wl_surface().id() == *mid))
                        {
                            column
                                .first()
                                .and_then(|w| w.toplevel().map(|t| t.wl_surface().id()))
                        } else {
                            None
                        }
                    })
                });
                let gaps = config.inner_gap.max(0);

                let representatives: Vec<Window> =
                    columns.iter().map(|column| column[0].clone()).collect();

                let mut origin = crate::layout::scroll_view_origin(
                    screen_rect,
                    gaps,
                    active,
                    maximized_ref.as_ref(),
                    &representatives,
                    &self.scroll_widths,
                );

                if let Some((rid, rtarget)) = &self.tile_resize {
                    let active_is_resized = columns.get(active).is_some_and(|column| {
                        column.iter().any(|w| {
                            w.toplevel().map(|t| t.wl_surface().id()).as_ref() == Some(rid)
                        })
                    });
                    if active_is_resized {
                        origin = Point::from((rtarget.loc.x, screen_rect.loc.y));
                    }
                }

                if input_output_name.as_ref() == Some(&output.name()) {
                    self.scroll_active_idx = active;
                    self.scroll_view_origin = origin;
                }

                let column_layouts = crate::layout::layout_scroll(
                    &representatives,
                    screen_rect,
                    config.inner_gap.max(0),
                    active,
                    origin,
                    maximized_ref.as_ref(),
                    &self.scroll_widths,
                );

                let focused = self.focused_column_position(&columns);

                for (index, column) in columns.iter().enumerate() {
                    let column_rect = column_layouts[index].1;
                    let anchor = column[0].toplevel().map(|t| t.wl_surface().id());
                    let focused_row = match focused {
                        Some((focus_col, row)) if focus_col == index => row,
                        _ => anchor
                            .as_ref()
                            .and_then(|id| self.column_rows.get(id).copied())
                            .unwrap_or(0),
                    };

                    if let (Some(id), Some((focus_col, row))) = (anchor, focused) {
                        if focus_col == index {
                            self.column_rows.insert(id, row);
                        }
                    }

                    let rows = crate::layout::layout_stack_rows(
                        column_rect,
                        config.inner_gap.max(0),
                        column.len(),
                        focused_row,
                    );

                    layouts.extend(column.iter().cloned().zip(rows));
                }
            } else if config.layout == "canvas" {
                layouts.extend(self.canvas_layout_for(&output, output_geo, &windows));
            } else {
                layouts.extend(self.dwindle_layout_for(
                    &output.name(),
                    &windows,
                    screen_rect,
                    config.inner_gap.max(0),
                ));
            }
        }

        for (window, target_rect) in layouts {
            let Some(toplevel) = window.toplevel() else {
                continue;
            };

            let id = toplevel.wl_surface().id();

            if self.dragging_window.as_ref() == Some(&id) {
                continue;
            }

            let target_rect = if config.layout != "scroll"
                && self.maximized_windows.contains(&id)
            {
                self.window_output
                    .get(&id)
                    .and_then(|name| {
                        self.space.outputs().find(|o| o.name() == *name)
                    })
                    .and_then(|o| self.space.output_geometry(o).map(|geo| (o.name(), geo)))
                    .map(|(name, geo)| {
                        let outer = config
                            .outer_gap
                            .max(0)
                            .min(((geo.size.w.min(geo.size.h) - 1) / 2).max(0));
                        let screen = Rectangle::new(
                            (geo.loc.x + outer, geo.loc.y + outer).into(),
                            (
                                (geo.size.w - 2 * outer).max(1),
                                (geo.size.h - 2 * outer).max(1),
                            )
                                .into(),
                        );
                        if config.layout == "canvas" {
                            self.canvas_camera_for(&name).to_world(geo.loc, screen)
                        } else {
                            screen
                        }
                    })
                    .unwrap_or(target_rect)
            } else {
                target_rect
            };

            if self.pending_close.contains(&id) {
                continue;
            }

            let size_changed = self
                .animations
                .get(&id)
                .map(|animation| animation.target_size != target_rect.size)
                .unwrap_or(true);

            if size_changed {
                toplevel.with_pending_state(|pending| {
                    pending.size = Some(target_rect.size);
                });
                toplevel.send_configure();
            }

            let entry = self
                .animations
                .entry(id)
                .or_insert_with(|| WindowAnimationState {
                    start_time: std::time::Instant::now(),
                    start_loc: target_rect.loc.to_f64(),
                    start_size: target_rect.size.to_f64(),
                    start_alpha: 1.0,
                    target_loc: target_rect.loc,
                    target_size: target_rect.size,
                    target_alpha: 1.0,
                    is_closing: false,
                });

            entry.target_loc = target_rect.loc;
            entry.target_size = target_rect.size;

            if !config.animation_enabled || (config.layout == "canvas" && self.canvas_snap) {
                entry.start_loc = target_rect.loc.to_f64();
                entry.start_size = target_rect.size.to_f64();

                if self.dragging_window.is_none() {
                    let off = window
                        .toplevel()
                        .map(|t| self.ws_pan_offset_for(&t.wl_surface().id()))
                        .unwrap_or_default();
                    self.space.map_element(
                        window.clone(),
                        target_rect.loc + off,
                        false,
                    );
                }
            }
        }
    }

    /// Build the persistent dwindle tile layout for one output.
    ///
    pub fn dwindle_layout_for(
        &mut self,
        output: &str,
        windows: &[Window],
        screen_rect: Rectangle<i32, Logical>,
        gaps: i32,
    ) -> Vec<(Window, Rectangle<i32, Logical>)> {
        if windows.is_empty() {
            self.dwindle_roots.remove(output);
            return Vec::new();
        }

        let mouse = Point::from((self.cursor_position.x as i32, self.cursor_position.y as i32));

        let live_ids: Vec<ObjectId> = windows
            .iter()
            .map(|w| w.toplevel().unwrap().wl_surface().id())
            .collect();

        let mut root = self.dwindle_roots.remove(output);
        while let Some(r) = root.as_ref() {
            let missing = r
                .leaves()
                .into_iter()
                .map(|(id, _)| id)
                .find(|id| !live_ids.contains(id));
            match missing {
                Some(id) => {
                    if crate::layout::dwindle_remove(&mut root, &id, gaps) {
                        break;
                    }
                }
                None => break,
            }
        }

        let mut root = match root {
            Some(root) if !root.leaves().is_empty() => root,
            _ => {
                let Some(first) = windows.first() else {
                    return Vec::new();
                };
                let id = first.toplevel().unwrap().wl_surface().id();
                crate::layout::dwindle_new(id, screen_rect)
            }
        };

        let split_ratio = self.config_watcher.config.dwindle_split_ratio;
        let split_direction = self.config_watcher.config.dwindle_split_direction.clone();
        for w in windows.iter().skip(1) {
            let id = w.toplevel().unwrap().wl_surface().id();
            if !root.contains(&id) {
                crate::layout::dwindle_insert(
                    &mut root,
                    id.clone(),
                    mouse,
                    gaps,
                    split_ratio,
                    &split_direction,
                );
            }
        }

        root.set_rect(screen_rect);
        crate::layout::dwindle_recompute(&mut root, gaps);

        if let Some((rid, target)) = &self.tile_resize {
            if root.contains(rid) {
                crate::layout::dwindle_resize_to(&mut root, rid, *target);
                root.set_rect(screen_rect);
                crate::layout::dwindle_recompute(&mut root, gaps);
            }
        }

        let leaves = root.leaves();
        self.dwindle_roots.insert(output.to_string(), root);

        leaves
            .into_iter()
            .filter_map(|(id, rect)| {
                windows
                    .iter()
                    .find(|w| w.toplevel().unwrap().wl_surface().id() == id)
                    .map(|w| (w.clone(), rect))
            })
            .collect()
    }

    /// Focus the remaining window whose current rect center is nearest to the
    /// center of `target` (used to hand focus to the window that just moved
    pub fn focus_closest_window_to(&mut self, target: Option<Rectangle<i32, Logical>>) {
        let Some(target) = target else {
            return;
        };
        let target_center = (target.loc.x + target.size.w / 2, target.loc.y + target.size.h / 2);

        let windows: Vec<Window> = self.space.elements().cloned().collect();
        if windows.is_empty() {
            let keyboard = self.seat.get_keyboard().unwrap();
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Option::<WlSurface>::None, serial);
            return;
        }

        let mut best: Option<(Window, i64)> = None;
        for window in windows {
            let Some(loc) = self.space.element_location(&window) else {
                continue;
            };
            let geo = self
                .space
                .element_geometry(&window)
                .map(|geometry| geometry.size)
                .unwrap_or_else(|| window.geometry().size);
            let center = (loc.x + geo.w / 2, loc.y + geo.h / 2);
            let distance =
                (center.0 - target_center.0).abs() as i64 + (center.1 - target_center.1).abs() as i64;
            if best
                .as_ref()
                .map(|(_, best_distance)| distance < *best_distance)
                .unwrap_or(true)
            {
                best = Some((window, distance));
            }
        }

        if let Some((window, _)) = best {
            self.focus_window(&window);
        }
    }

    pub fn scroll_active(&mut self, dx: isize) {
        self.reconcile_column_order();

        if dx == 0 {
            return;
        }

        let Some(output) = self.input_output() else {
            return;
        };

        let windows = self.ordered_windows_on(&output);

        if windows.is_empty() {
            return;
        }

        let columns = self.columns_of(&windows);

        let remembered = self
            .output_columns
            .get(&(
                self.active_workspace_on(&output.name()),
                output.name(),
            ))
            .copied()
            .unwrap_or(0);

        let current = self
            .focused_column_position(&columns)
            .map(|(col, _)| col)
            .unwrap_or(remembered)
            .min(columns.len() - 1);

        let next = if dx < 0 {
            current.saturating_sub(dx.unsigned_abs())
        } else {
            current.saturating_add(dx as usize).min(columns.len() - 1)
        };

        if next == current {
            return;
        }

        let row = self.landing_row(&columns[next]);

        if let Some(anchor) = columns[next][0].toplevel().map(|t| t.wl_surface().id()) {
            self.column_rows.insert(anchor, row);
        }

        self.remember_column(&output, next);
        self.focus_window(&columns[next][row]);
        self.arrange_windows();
    }

    /// Move focus vertically within the focused scroll column (`dy > 0` moves
    /// down): plain Super+scroll, or Super+j/k. Saturates at the stack ends;
    pub fn scroll_stack(&mut self, dy: isize) {
        self.reconcile_column_order();

        if dy == 0 {
            return;
        }

        let Some(output) = self.input_output() else {
            return;
        };

        let windows = self.ordered_windows_on(&output);

        if windows.is_empty() {
            self.switch_workspace(dy.signum());
            return;
        }

        let columns = self.columns_of(&windows);

        let Some((col, row)) = self.focused_column_position(&columns) else {
            return;
        };

        let next = if dy < 0 {
            row.saturating_sub(dy.unsigned_abs())
        } else {
            row.saturating_add(dy as usize).min(columns[col].len() - 1)
        };

        if next == row {
            self.switch_workspace(dy.signum());
            return;
        }

        if let Some(anchor) = columns[col][0].toplevel().map(|t| t.wl_surface().id()) {
            self.column_rows.insert(anchor, next);
        }

        self.focus_window(&columns[col][next]);
        self.arrange_windows();
    }

    /// Active workspace of one output (0 when never switched there).
    pub fn active_workspace_on(&self, output_name: &str) -> usize {
        self.active_workspaces
            .get(output_name)
            .copied()
            .unwrap_or(0)
    }

    /// Highest workspace id used on one output (at least its active one).
    fn max_workspace_on(&self, output_name: &str) -> usize {
        let mut max_workspace = self.active_workspace_on(output_name);
        for (id, ws) in self.window_workspace.iter() {
            if self.window_output.get(id).map(String::as_str) == Some(output_name) {
                max_workspace = max_workspace.max(*ws);
            }
        }
        max_workspace
    }

    /// Workspace `output_name` was on when the overview opened (Escape target).
    pub fn overview_entry_for(&self, output_name: &str) -> usize {
        self.overview_entry
            .get(output_name)
            .copied()
            .unwrap_or_else(|| self.active_workspace_on(output_name))
    }

    /// Switch to the previous/next workspace (niri-style `focus-workspace-up/down`).
    ///
    pub fn switch_workspace(&mut self, direction: isize) {
        self.reconcile_column_order();

        let Some(output) = self.input_output() else {
            return;
        };
        let name = output.name();
        let old = self.active_workspace_on(&name);
        let max_workspace = self.max_workspace_on(&name);

        let next = if direction < 0 {
            old.saturating_sub(direction.unsigned_abs())
        } else {
            old.saturating_add(direction as usize)
                .min(max_workspace.saturating_add(1))
        };

        if next == old {
            return;
        }

        self.activate_workspace(next);
    }

    /// Show workspace `n` directly (Super+1..9, overview jumps) on the monitor
    /// under the pointer. Allows one past the last used workspace, creating
    pub fn goto_workspace(&mut self, n: usize) {
        self.reconcile_column_order();

        let Some(output) = self.input_output() else {
            return;
        };
        let name = output.name();
        let old = self.active_workspace_on(&name);
        let max_workspace = self.max_workspace_on(&name);

        let next = n.min(max_workspace.saturating_add(1));

        if next == old {
            return;
        }

        self.activate_workspace(next);
    }

    /// Move the monitor under the pointer to `next`: unmap its other
    /// workspaces, map `next`, restore its focus and re-arrange. Every other
    fn activate_workspace(&mut self, next: usize) {
        if let Some(output) = self.input_output() {
            let windows = self.ordered_windows_on(&output);

            if self.config_watcher.config.layout == "scroll" {
                let columns = self.columns_of(&windows);

                if let Some((col, _)) = self.focused_column_position(&columns) {
                    self.remember_column(&output, col);
                }
            } else if let Some(index) = self.focused_window_index(&windows) {
                self.remember_column(&output, index);
            }
        }

        let Some(output) = self.input_output() else {
            return;
        };
        let name = output.name();
        let old_workspace = self.active_workspace_on(&name);
        self.active_workspaces.insert(name.clone(), next);

        if let Some(drag_id) = self.dragging_window.clone() {
            let on_this = self
                .window_output
                .get(&drag_id)
                .map(String::as_str)
                == Some(name.as_str())
                || self
                    .output_under(Some(self.cursor_position))
                    .is_some_and(|o| o.name() == name);
            if on_this {
                self.window_workspace.insert(drag_id, next);
            }
        }

        let pan = !self.overview_active
            && next != old_workspace
            && self.config_watcher.config.animation_enabled
            && !matches!(
                self.overview_transition,
                Some(OverviewTransition::Exiting { .. })
            );
        if let Some(prev) = self.ws_pan.as_ref() {
            if prev.output != name {
                let stale: Vec<Window> = self
                    .space
                    .elements()
                    .filter(|w| {
                        w.toplevel().is_some_and(|t| {
                            let id = &t.wl_surface().id();
                            self.window_workspace.get(id) == Some(&prev.old)
                                && self.window_output.get(id).map(String::as_str)
                                    == Some(prev.output.as_str())
                        })
                    })
                    .cloned()
                    .collect();
                for window in stale {
                    self.space.unmap_elem(&window);
                }
            }
        }
        self.ws_pan = if pan {
            Some(WsPan {
                output: name.clone(),
                old: old_workspace,
                next,
                start: std::time::Instant::now(),
            })
        } else {
            None
        };

        let mapped: Vec<Window> = self.space.elements().cloned().collect();

        for window in mapped {
            let id = window.toplevel().unwrap().wl_surface().id();

            if self.window_output.get(&id).map(String::as_str) != Some(name.as_str()) {
                continue;
            }

            let is_dragged = self.dragging_window.as_ref() == Some(&id);
            let keep = is_dragged
                || (pan && self.window_workspace.get(&id) == Some(&old_workspace));
            if self.window_workspace.get(&id) != Some(&next) && !keep {
                self.space.unmap_elem(&window);
            }
        }

        for window in self.all_windows.clone() {
            let Some(toplevel) = window.toplevel() else {
                continue;
            };

            if !toplevel.wl_surface().is_alive() {
                continue;
            }

            let id = toplevel.wl_surface().id();

            if self.window_output.get(&id).map(String::as_str) == Some(name.as_str())
                && self.window_workspace.get(&id) == Some(&next)
                && self.space.element_location(&window).is_none()
            {
                let location = self
                    .animations
                    .get(&id)
                    .map(|animation| animation.target_loc)
                    .unwrap_or_default();

                self.space.map_element(window, location, false);
            }
        }

        self.focus_workspace_top();
        self.arrange_windows();

        if self.overview_active {
            self.apply_overview_mapping();
        }
    }

    /// Sorted workspaces that exist (used ones plus the active one, so the
    /// current desktop always has a cell even when empty), plus Niri's
    fn overview_workspaces_on(&self, output_name: &str) -> Vec<usize> {
        let mut workspaces: Vec<usize> = Vec::new();
        for (id, ws) in self.window_workspace.iter() {
            if self.window_output.get(id).map(String::as_str) == Some(output_name) {
                workspaces.push(*ws);
            }
        }
        workspaces.push(self.active_workspace_on(output_name));
        if let Some(max) = workspaces.iter().max() {
            workspaces.push(max.saturating_add(1));
        }
        workspaces.sort_unstable();
        workspaces.dedup();
        workspaces
    }

    /// Grid cells for the overview on one output: a single-column vertical
    /// strip (Niri-style) in workspace order, with the ACTIVE desktop pinned
    fn overview_cells(
        &self,
        output_name: &str,
        output_geo: Rectangle<i32, Logical>,
    ) -> Vec<(usize, Rectangle<i32, Logical>)> {
        const COLUMNS: usize = 1;
        const GAP: i32 = 64;

        let workspaces = self.overview_workspaces_on(output_name);
        let active_pos = workspaces
            .iter()
            .position(|ws| *ws == self.active_workspace_on(output_name))
            .unwrap_or(0);
        let (active_col, active_row) = (active_pos % COLUMNS, active_pos / COLUMNS);

        let (w, h) = (output_geo.size.w.max(1), output_geo.size.h.max(1));

        workspaces
            .into_iter()
            .enumerate()
            .map(|(index, ws)| {
                let col = index % COLUMNS;
                let row = index / COLUMNS;
                let x = output_geo.loc.x + (col as i32 - active_col as i32) * (w + GAP);
                let y = output_geo.loc.y + (row as i32 - active_row as i32) * (h + GAP);
                (ws, Rectangle::new((x, y).into(), (w, h).into()))
            })
            .collect()
    }

    /// Fitted camera for the overview on one output: the whole grid fits on
    /// screen, centered (slightly zoomed out even for a single desktop so the
    fn overview_fitted_camera_for(&self, output_name: &str) -> CanvasCamera {
        let Some(output) = self.space.outputs().find(|o| o.name() == output_name) else {
            return CanvasCamera::default();
        };
        let Some(geo) = self.space.output_geometry(output) else {
            return CanvasCamera::default();
        };

        let cells = self.overview_cells(output_name, geo);
        if cells.is_empty() {
            return CanvasCamera::default();
        }

        let (mut min_x, mut min_y) = (i32::MAX, i32::MAX);
        let (mut max_x, mut max_y) = (i32::MIN, i32::MIN);
        for (_, cell) in &cells {
            min_x = min_x.min(cell.loc.x - geo.loc.x);
            min_y = min_y.min(cell.loc.y - geo.loc.y);
            max_x = max_x.max(cell.loc.x - geo.loc.x + cell.size.w);
            max_y = max_y.max(cell.loc.y - geo.loc.y + cell.size.h);
        }

        let bounds_w = (max_x - min_x).max(1) as f64;
        let bounds_h = (max_y - min_y).max(1) as f64;
        let zoom = (geo.size.w as f64 / bounds_w)
            .min(geo.size.h as f64 / bounds_h)
            .min(0.85)
            .max(0.05);

        CanvasCamera {
            offset_x: geo.size.w as f64 / 2.0 - (min_x as f64 + bounds_w / 2.0) * zoom,
            offset_y: geo.size.h as f64 / 2.0 - (min_y as f64 + bounds_h / 2.0) * zoom,
            zoom,
        }
    }

    /// Eased 0..1 progress of a transition that started at `start`, on the
    /// shared animation curve (snaps to done when animations are off).
    fn anim_progress(&self, start: std::time::Instant) -> f64 {
        let config = &self.config_watcher.config;
        if !config.animation_enabled {
            return 1.0;
        }
        let duration_ms = config.animation_duration;
        if duration_ms <= 0.0 {
            return 1.0;
        }
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let bezier = CubicBezier::new(
            config.animation_bezier.0,
            config.animation_bezier.1,
            config.animation_bezier.2,
            config.animation_bezier.3,
        );
        bezier.ease((elapsed / duration_ms).min(1.0))
    }

    /// Eased 0..1 progress of the active workspace pan, if one is running.
    fn ws_pan_progress(&self) -> Option<f64> {
        self.ws_pan
            .as_ref()
            .map(|pan| self.anim_progress(pan.start))
    }

    /// Output height for pan math (fallback when the window's output is gone).
    fn output_height_for(&self, name: &str) -> i32 {
        self.space
            .outputs()
            .find(|o| o.name() == name)
            .and_then(|o| self.space.output_geometry(o))
            .map(|geo| geo.size.h.max(1))
            .unwrap_or(1080)
    }

    /// Screen-space pan offset for one window: while a workspace switch
    /// runs, the incoming desktop glides in from one screen off (in the
    fn ws_pan_offset_for(&self, id: &ObjectId) -> Point<i32, Logical> {
        let (Some(pan), Some(progress)) = (self.ws_pan.as_ref(), self.ws_pan_progress())
        else {
            return Point::default();
        };
        if self.window_output.get(id).map(String::as_str) != Some(pan.output.as_str()) {
            return Point::default();
        }
        let ws = self.window_workspace.get(id).copied();
        if ws != Some(pan.old) && ws != Some(pan.next) {
            return Point::default();
        }
        let h = self
            .window_output
            .get(id)
            .map(|name| self.output_height_for(name))
            .unwrap_or(1080);
        let sign = if pan.next > pan.old { 1.0 } else { -1.0 };
        let dy = if ws == Some(pan.next) {
            sign * h as f64 * (1.0 - progress)
        } else {
            -sign * h as f64 * progress
        };
        Point::from((0, dy.round() as i32))
    }

    /// Freeze every mapped window's animation at its target (the per-frame
    /// driver then holds tiles still): used when leaving the overview so the
    fn freeze_mapped_animations(&mut self) {
        let now = std::time::Instant::now();
        let ids: Vec<ObjectId> = self
            .space
            .elements()
            .filter_map(|w| w.toplevel().map(|t| t.wl_surface().id()))
            .collect();
        for id in ids {
            if let Some(entry) = self.animations.get_mut(&id) {
                if entry.is_closing {
                    continue;
                }
                entry.start_time = now;
                entry.start_loc = entry.target_loc.to_f64();
                entry.start_size = entry.target_size.to_f64();
            }
        }
    }

    /// True while the overview-closing zoom is settling (rendering still
    /// goes through the camera even though the overview is off).
    pub fn overview_closing(&self) -> bool {
        matches!(
            self.overview_transition,
            Some(OverviewTransition::Exiting { .. })
        )
    }

    /// Live overview camera: the fitted grid, plus the enter zoom (identity
    /// growing out to the fit - the desktops shrink away as one) and the
    pub fn overview_camera_for(&self, output_name: &str) -> CanvasCamera {
        let fitted = self.overview_fitted_camera_for(output_name);
        if let Some(OverviewTransition::Shifting { output, .. }) = &self.overview_transition {
            if output.as_str() != output_name {
                return fitted;
            }
        }
        match self.overview_transition {
            Some(OverviewTransition::Entering { start }) => {
                let t = self.anim_progress(start).clamp(0.0, 1.0);
                let from = CanvasCamera::default();
                CanvasCamera {
                    offset_x: from.offset_x + (fitted.offset_x - from.offset_x) * t,
                    offset_y: from.offset_y + (fitted.offset_y - from.offset_y) * t,
                    zoom: from.zoom + (fitted.zoom - from.zoom) * t,
                }
            }
            Some(OverviewTransition::Shifting { rows, start, .. }) => {
                let t = self.anim_progress(start).clamp(0.0, 1.0);
                let stride = self
                    .space
                    .outputs()
                    .find(|o| o.name() == output_name)
                    .and_then(|o| self.space.output_geometry(o))
                    .map(|geo| {
                        let cells = self.overview_cells(output_name, geo);
                        if cells.len() >= 2 {
                            (cells[1].1.loc.y - cells[0].1.loc.y).max(1) as f64
                        } else {
                            geo.size.h.max(1) as f64
                        }
                    })
                    .unwrap_or(1080.0);
                CanvasCamera {
                    offset_x: fitted.offset_x,
                    offset_y: fitted.offset_y
                        + rows as f64 * stride * fitted.zoom * (1.0 - t),
                    zoom: fitted.zoom,
                }
            }
            _ => fitted,
        }
    }

    /// Closing overview camera: the fitted grid growing back to the plain
    /// 1:1 tiled view (identity once the zoom settles).
    pub fn overview_closing_camera_for(&self, output_name: &str) -> CanvasCamera {
        let Some(OverviewTransition::Exiting { start }) = self.overview_transition else {
            return CanvasCamera::default();
        };
        let fitted = self.overview_fitted_camera_for(output_name);
        let t = self.anim_progress(start).clamp(0.0, 1.0);
        let to = CanvasCamera::default();
        CanvasCamera {
            offset_x: fitted.offset_x + (to.offset_x - fitted.offset_x) * t,
            offset_y: fitted.offset_y + (to.offset_y - fitted.offset_y) * t,
            zoom: fitted.zoom + (to.zoom - fitted.zoom) * t,
        }
    }

    /// Whatever camera the compositor is actually looking through right now
    /// (overview, closing zoom, or plain 1:1), for hit-testing.
    pub fn overview_effective_camera_for(&self, output_name: &str) -> CanvasCamera {
        if self.overview_active {
            self.overview_camera_for(output_name)
        } else {
            self.overview_closing_camera_for(output_name)
        }
    }

    /// Map a screen point to Space coordinates for WINDOW hit-testing in the
    /// overview (inverse fitted camera). Identity when inactive.
    pub fn overview_pick_pos(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        if !self.overview_active && !self.overview_closing() {
            return pos;
        }
        let Some(output) = self.output_under(Some(pos)) else {
            return pos;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return pos;
        };
        let cam = self.overview_effective_camera_for(&output.name());
        let zoom = cam.zoom.max(0.05);
        let lx = pos.x - geo.loc.x as f64;
        let ly = pos.y - geo.loc.y as f64;
        Point::from((
            geo.loc.x as f64 + (lx - cam.offset_x) / zoom,
            geo.loc.y as f64 + (ly - cam.offset_y) / zoom,
        ))
    }

    /// Windows of one workspace on one output, in column order (mapped or
    /// not): the overview shows live windows only - closing windows and the
    fn overview_windows(&self, workspace: usize, output_name: &str) -> Vec<Window> {
        let position: std::collections::HashMap<&ObjectId, usize> = self
            .column_order
            .iter()
            .enumerate()
            .map(|(index, id)| (id, index))
            .collect();

        let mut windows: Vec<Window> = self
            .all_windows
            .iter()
            .filter(|window| {
                let Some(toplevel) = window.toplevel() else {
                    return false;
                };
                if !toplevel.wl_surface().is_alive() {
                    return false;
                }
                let id = toplevel.wl_surface().id();
                if self.pending_close.contains(&id) {
                    return false;
                }
                if self.scratchpad_id.as_ref() == Some(&id) && !self.scratchpad_visible {
                    return false;
                }
                self.window_output.get(&id).map(String::as_str) == Some(output_name)
                    && self.window_workspace.get(&id) == Some(&workspace)
            })
            .cloned()
            .collect();

        windows.sort_by_key(|window| {
            window
                .toplevel()
                .map(|t| {
                    position
                        .get(&t.wl_surface().id())
                        .copied()
                        .unwrap_or(usize::MAX)
                })
                .unwrap_or(usize::MAX)
        });
        windows
    }

    /// Lay out one overview cell: canvas desktops keep their real free
    /// arrangement (shifted into the cell); every other layout renders its
    fn overview_cell_layout(
        &self,
        output_name: &str,
        output_geo: Rectangle<i32, Logical>,
        workspace: usize,
        cell: Rectangle<i32, Logical>,
        windows: &[Window],
    ) -> Vec<(Window, Rectangle<i32, Logical>)> {
        if windows.is_empty() {
            return Vec::new();
        }

        let (floating, tiled): (Vec<&Window>, Vec<&Window>) = windows
            .iter()
            .partition(|window| {
                window.toplevel().is_some_and(|t| {
                    self.floating_windows.contains_key(&t.wl_surface().id())
                })
            });

        let mut out = Vec::with_capacity(windows.len());

        for window in floating {
            let id = window.toplevel().unwrap().wl_surface().id();
            if let Some(rect) = self.floating_windows.get(&id) {
                out.push((
                    (*window).clone(),
                    Rectangle::new(
                        (
                            cell.loc.x + rect.loc.x - output_geo.loc.x,
                            cell.loc.y + rect.loc.y - output_geo.loc.y,
                        )
                            .into(),
                        rect.size,
                    ),
                ));
            }
        }

        if self.config_watcher.config.layout == "canvas" {
            for window in tiled {
                let id = window.toplevel().unwrap().wl_surface().id();
                let world = self.canvas_rects.get(&id).copied().unwrap_or_else(|| {
                    let loc = self
                        .space
                        .element_location(window)
                        .unwrap_or(output_geo.loc);
                    let size = self
                        .space
                        .element_geometry(window)
                        .map(|g| g.size)
                        .unwrap_or((900, 620).into());
                    Rectangle::new(
                        (loc.x - output_geo.loc.x, loc.y - output_geo.loc.y).into(),
                        size,
                    )
                });
                out.push((
                    (*window).clone(),
                    Rectangle::new(
                        (cell.loc.x + world.loc.x, cell.loc.y + world.loc.y).into(),
                        (world.size.w.max(1), world.size.h.max(1)).into(),
                    ),
                ));
            }

            return out;
        }

        let tiled: Vec<Window> = tiled.into_iter().cloned().collect();

        if tiled.is_empty() {
            return out;
        }

        let columns = self.columns_of(&tiled);
        let representatives: Vec<Window> =
            columns.iter().map(|column| column[0].clone()).collect();

        let gaps = self.config_watcher.config.inner_gap.max(0);
        let active_col = self
            .output_columns
            .get(&(workspace, output_name.to_string()))
            .copied()
            .unwrap_or(0)
            .min(columns.len() - 1);

        let origin = crate::layout::scroll_view_origin(
            cell,
            gaps,
            active_col,
            None,
            &representatives,
            &self.scroll_widths,
        );
        let column_layouts = crate::layout::layout_scroll(
            &representatives,
            cell,
            gaps,
            active_col,
            origin,
            None,
            &self.scroll_widths,
        );

        for (index, column) in columns.iter().enumerate() {
            let column_rect = column_layouts[index].1;
            let anchor = column[0].toplevel().map(|t| t.wl_surface().id());
            let focused_row = anchor
                .as_ref()
                .and_then(|id| self.column_rows.get(id).copied())
                .unwrap_or(0);
            let rows = crate::layout::layout_stack_rows(
                column_rect,
                gaps,
                column.len(),
                focused_row,
            );
            out.extend(column.iter().cloned().zip(rows));
        }

        out
    }

    /// Map every desktop's windows and pin them into their overview grid
    /// cells (snapped, no client reconfigures: scaling is render-time). Runs
    fn apply_overview_mapping(&mut self) {
        self.reconcile_column_order();

        let outputs: Vec<_> = self.space.outputs().cloned().collect();

        for output in outputs {
            let Some(geo) = self.space.output_geometry(&output) else {
                continue;
            };

            for (workspace, cell) in self.overview_cells(&output.name(), geo) {
                let windows = self.overview_windows(workspace, &output.name());
                let layouts =
                    self.overview_cell_layout(&output.name(), geo, workspace, cell, &windows);

                for (window, target_rect) in layouts {
                    let Some(toplevel) = window.toplevel() else {
                        continue;
                    };
                    let id = toplevel.wl_surface().id();

                    self.space
                        .map_element(window.clone(), target_rect.loc, false);
                    let entry = self
                        .animations
                        .entry(id.clone())
                        .or_insert_with(|| WindowAnimationState {
                            start_time: std::time::Instant::now(),
                            start_loc: target_rect.loc.to_f64(),
                            start_size: target_rect.size.to_f64(),
                            start_alpha: 1.0,
                            target_loc: target_rect.loc,
                            target_size: target_rect.size,
                            target_alpha: 1.0,
                            is_closing: false,
                        });
                    entry.start_time = std::time::Instant::now();
                    entry.start_loc = target_rect.loc.to_f64();
                    entry.start_size = target_rect.size.to_f64();
                    entry.target_loc = target_rect.loc;
                    entry.target_size = target_rect.size;
                }
            }
        }
    }

    /// Enter the all-desktops overview: freeze the layout, grid every
    /// desktop's windows into cells. No-op when already active.
    pub fn enter_overview(&mut self) {
        if self.overview_active {
            return;
        }

        self.reconcile_column_order();
        let entries: Vec<(String, usize)> = self
            .space
            .outputs()
            .map(|output| {
                let name = output.name();
                let ws = self.active_workspace_on(&name);
                (name, ws)
            })
            .collect();
        self.overview_entry = entries.into_iter().collect();
        self.ws_pan = None;
        self.overview_transition = if self.config_watcher.config.animation_enabled {
            Some(OverviewTransition::Entering {
                start: std::time::Instant::now(),
            })
        } else {
            None
        };
        self.overview_active = true;
        self.apply_overview_mapping();
    }

    /// Leave the overview, landing on `target` (default: keep the live
    /// selection). Always re-maps exactly the landed workspace and re-tiles.
    pub fn exit_overview(&mut self, target: Option<usize>) {
        if !self.overview_active {
            return;
        }

        let fallback = self
            .input_output()
            .map(|output| self.active_workspace_on(&output.name()))
            .unwrap_or(0);
        let workspace = target.unwrap_or(fallback);
        self.overview_transition = if self.config_watcher.config.animation_enabled {
            Some(OverviewTransition::Exiting {
                start: std::time::Instant::now(),
            })
        } else {
            None
        };
        self.overview_active = false;
        self.activate_workspace(workspace);
        self.freeze_mapped_animations();
    }

    /// Leave the overview, returning the monitor under the pointer to the
    /// desktop it was on when the overview opened (Escape).
    pub fn exit_overview_to_entry(&mut self) {
        if !self.overview_active {
            return;
        }
        let target = self
            .input_output()
            .map(|output| self.overview_entry_for(&output.name()));
        self.exit_overview(target);
    }

    /// Toggle the overview (Super+o): entering remembers the current desktop,
    /// leaving keeps the live selection.
    pub fn toggle_overview(&mut self) {
        if self.overview_active {
            self.exit_overview(None);
        } else {
            self.enter_overview();
        }
    }

    /// Move the live selection inside the overview grid (arrow keys): one
    /// cell per press, saturating at the grid edges. No-op outside it.
    pub fn overview_nav(&mut self, dx: isize, dy: isize) {
        if !self.overview_active {
            return;
        }

        let Some(output) = self.input_output() else {
            return;
        };
        let name = output.name();
        let workspaces = self.overview_workspaces_on(&name);
        let pos = workspaces
            .iter()
            .position(|ws| *ws == self.active_workspace_on(&name))
            .unwrap_or(0);

        let target = (pos as isize + dx + dy)
            .clamp(0, workspaces.len() as isize - 1) as usize;

        self.goto_workspace(workspaces[target]);

        let delta = target as isize - pos as isize;
        if delta != 0 && self.config_watcher.config.animation_enabled {
            let rows = match self.overview_transition {
                Some(OverviewTransition::Shifting { rows, .. }) => rows + delta as i32,
                _ => delta as i32,
            };
            let start = match self.overview_transition {
                Some(OverviewTransition::Shifting { start, .. }) => start,
                _ => std::time::Instant::now(),
            };
            self.overview_transition = Some(OverviewTransition::Shifting {
                output: name.clone(),
                rows,
                start,
            });
        }
    }

    /// Focus the topmost (first-listed) window of the active workspace.
    fn focus_workspace_top(&mut self) {
        self.reconcile_column_order();

        let Some(output) = self.input_output() else {
            return;
        };

        let windows = self.ordered_windows_on(&output);

        if windows.is_empty() {
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(
                    self,
                    Option::<WlSurface>::None,
                    smithay::utils::SERIAL_COUNTER.next_serial(),
                );
            }

            return;
        }

        if self.config_watcher.config.layout == "scroll" {
            let columns = self.columns_of(&windows);
            let col = self
                .output_columns
                .get(&(
                    self.active_workspace_on(&output.name()),
                    output.name(),
                ))
                .copied()
                .unwrap_or(0)
                .min(columns.len() - 1);
            let row = self.landing_row(&columns[col]);

            self.remember_column(&output, col);
            self.focus_window(&columns[col][row]);

            return;
        }

        let index = self
            .output_columns
            .get(&(
                self.active_workspace_on(&output.name()),
                output.name(),
            ))
            .copied()
            .unwrap_or(0)
            .min(windows.len() - 1);

        self.remember_column(&output, index);
        self.focus_window(&windows[index]);
    }

    /// Remove a window (e.g. after it is destroyed) from all bookkeeping.
    pub fn remove_window_bookkeeping(&mut self, id: &ObjectId) {
        self.window_workspace.remove(id);
        self.window_output.remove(id);
        self.column_order.retain(|candidate| candidate != id);
        self.column_starts.remove(id);
        self.column_rows.remove(id);
        self.floating_windows.remove(id);
        self.window_opacity.remove(id);
        self.window_rules_done.remove(id);
        self.maximized_windows.remove(id);
        self.scroll_widths.remove(id);
        self.canvas_rects.remove(id);
        self.canvas_pinned.remove(id);
        if let Some((rid, _)) = &self.tile_resize {
            if rid == id {
                self.tile_resize = None;
            }
        }

        self.all_windows.retain(|window| {
            window
                .toplevel()
                .map(|t| t.wl_surface().id() != *id)
                .unwrap_or(false)
        });
    }

    /// Index (within `space.elements()` order) of the currently focused window,
    /// if any.
    fn focused_window_index(&self, windows: &[Window]) -> Option<usize> {
        let focus = self.current_focus_surface();
        windows
            .iter()
            .position(|w| Some(w.toplevel().unwrap().wl_surface().clone()) == focus)
    }

    /// Focus (set keyboard focus) on the window matching a surface, if present.
    pub fn focus_window(&mut self, window: &Window) {
        let keyboard = self.seat.get_keyboard().unwrap();
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        keyboard.set_focus(
            self,
            Some(window.toplevel().unwrap().wl_surface().clone()),
            serial,
        );
    }

    /// Get the currently focused surface, if any.
    pub fn current_focus_surface(&self) -> Option<WlSurface> {
        self.seat.get_keyboard().unwrap().current_focus()
    }

    /// Focus the window at the given index in `space.elements()` order.
    pub fn focus_index(&mut self, index: usize) {
        self.reconcile_column_order();

        let Some(output) = self.input_output() else {
            return;
        };

        let windows = self.ordered_windows_on(&output);

        if let Some(window) = windows.get(index) {
            if self.config_watcher.config.layout == "scroll" {
                let columns = self.columns_of(&windows);

                if let Some(id) = window.toplevel().map(|t| t.wl_surface().id()) {
                    if let Some(col) = columns.iter().position(|column| {
                        column.iter().any(|w| {
                            w.toplevel().is_some_and(|t| t.wl_surface().id() == id)
                        })
                    }) {
                        self.remember_column(&output, col);
                    }
                }
            } else {
                self.remember_column(&output, index);
            }

            self.focus_window(window);

            if self.config_watcher.config.layout == "scroll" {
                self.arrange_windows();
            }
        }
    }

    /// Move the focused window to a new list position (0-based index within
    /// the current window ordering). Then re-arrange.
    pub fn move_focused_to(&mut self, target_index: usize) {
        self.reconcile_column_order();

        let windows = self.input_columns();

        if windows.is_empty() {
            return;
        }

        if self.config_watcher.config.layout == "scroll" {
            self.move_focused_column_to(target_index);
            return;
        }

        let Some(current) = self.focused_window_index(&windows) else {
            return;
        };

        let target = target_index.min(windows.len() - 1);

        if target == current {
            return;
        }

        let mut ids: Vec<ObjectId> = windows
            .iter()
            .map(|window| window.toplevel().unwrap().wl_surface().id())
            .collect();

        let moved = ids.remove(current);
        ids.insert(target, moved);

        let member_ids: std::collections::HashSet<ObjectId> = windows
            .iter()
            .map(|window| window.toplevel().unwrap().wl_surface().id())
            .collect();

        let mut replacement = ids.into_iter();

        for id in &mut self.column_order {
            if member_ids.contains(id) {
                *id = replacement.next().unwrap();
            }
        }

        if let Some(output) = self.input_output() {
            self.remember_column(&output, target);
        }

        self.focus_window(&windows[current]);
        self.arrange_windows();
    }

    /// Move the focused window's scroll column (all its stacked rows) to a
    /// new column position, keeping the focused window focused.
    fn move_focused_column_to(&mut self, target_column: usize) {
        self.reconcile_column_order();

        let windows = self.input_columns();

        if windows.is_empty() {
            return;
        }

        let columns = self.columns_of(&windows);

        let Some((current_col, row)) = self.focused_column_position(&columns) else {
            return;
        };

        let target = target_column.min(columns.len() - 1);

        if target == current_col {
            return;
        }

        let mut blocks: Vec<Vec<ObjectId>> = columns
            .iter()
            .map(|column| {
                column
                    .iter()
                    .map(|window| window.toplevel().unwrap().wl_surface().id())
                    .collect()
            })
            .collect();

        let block = blocks.remove(current_col);
        let insert_at = target.min(blocks.len());
        blocks.insert(insert_at, block);

        let ids: Vec<ObjectId> = blocks.into_iter().flatten().collect();

        let member_ids: std::collections::HashSet<ObjectId> =
            ids.iter().cloned().collect();

        let mut replacement = ids.into_iter();

        for id in &mut self.column_order {
            if member_ids.contains(id) {
                *id = replacement.next().unwrap();
            }
        }

        if let Some(output) = self.input_output() {
            self.remember_column(&output, insert_at);
        }

        self.focus_window(&columns[current_col][row]);
        self.arrange_windows();
    }

    /// Toggle a hidden scratchpad terminal. Spawns one on first use.
    pub fn toggle_scratchpad(&mut self) {
        if let Some(id) = self.scratchpad_id.clone() {
            let exists = self
                .space
                .elements()
                .any(|w| w.toplevel().unwrap().wl_surface().id() == id);
            if !exists {
                self.scratchpad_id = None;
            }
        }

        if self.scratchpad_id.is_none() {
            let socket = self.socket_name.clone();
            let terminal = self.config_watcher.config.terminal.clone();
            let mut parts = terminal.split_whitespace();
            let binary = parts.next().unwrap_or("foot");
            let args: Vec<String> = parts.map(str::to_string).collect();
            let spawned = crate::spawn_wayland_client(binary, &args, &socket);
            crate::life(format!("scratchpad: spawn `{terminal}` result={spawned}"));
            self.scratchpad_pending = true;
            self.scratchpad_visible = true;
            return;
        }

        let id = self.scratchpad_id.clone().unwrap();
        if self.scratchpad_visible {
            self.scratchpad_visible = false;
            crate::life(format!("scratchpad: hiding {id}"));
            self.arrange_windows();
            if let Some(next) = self.input_columns().first().cloned() {
                self.focus_window(&next);
            } else {
                let kb = self.seat.get_keyboard().unwrap();
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                kb.set_focus(self, Option::<WlSurface>::None, serial);
            }
        } else {
            self.scratchpad_visible = true;
            crate::life(format!("scratchpad: showing {id}"));
            self.arrange_windows();
            for w in self.space.elements().cloned().collect::<Vec<_>>() {
                if w.toplevel().unwrap().wl_surface().id() == id {
                    self.focus_window(&w);
                    break;
                }
            }
        }
    }

    /// Toggle maximize on the currently focused window (Super+F).
    ///
    pub fn is_maximized(&self, id: &ObjectId) -> bool {
        self.maximized_windows.contains(id)
    }

    pub fn toggle_fullscreen(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let Some(focused_surface) = keyboard.current_focus() else {
            return;
        };

        let focused_window = self
            .space
            .elements()
            .find(|w| {
                w.toplevel()
                    .map(|t| *t.wl_surface() == focused_surface)
                    .unwrap_or(false)
            })
            .cloned();

        let Some(window) = focused_window else {
            return;
        };

        let id = window.toplevel().unwrap().wl_surface().id();

        if self.maximized_windows.contains(&id) {
            tracing::info!("NovaWM: exiting maximize for {id}");
            self.maximized_windows.remove(&id);
            window.toplevel().unwrap().with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
            });
            window.toplevel().unwrap().send_configure();
        } else {
            tracing::info!("NovaWM: maximizing {id}");
            self.floating_windows.remove(&id);
            self.maximized_windows.insert(id);
            window.toplevel().unwrap().with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
            });
            window.toplevel().unwrap().send_configure();
        }

        self.arrange_windows();
    }

    /// Is this window currently floating (out of the tiling layout)?
    pub fn is_floating(&self, id: &ObjectId) -> bool {
        self.floating_windows.contains_key(id)
    }

    /// Toggle floating on the focused window (Super+v).
    ///
    pub fn toggle_float(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let Some(focused_surface) = keyboard.current_focus() else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| {
                w.toplevel()
                    .map(|t| *t.wl_surface() == focused_surface)
                    .unwrap_or(false)
            })
            .cloned()
        else {
            return;
        };
        let id = window.toplevel().unwrap().wl_surface().id();

        if self.floating_windows.remove(&id).is_some() {
            tracing::info!("NovaWM: unfloating {id}");
        } else {
            tracing::info!("NovaWM: floating {id}");
            self.float_window_internal(&window);
        }

        self.arrange_windows();
    }

    /// Mark `window` as floating (Hyprland-style Super+right-drag resize turns
    /// a tiled window into a floating one). No-op if already floating.
    pub fn float_window(&mut self, window: &Window) {
        let id = window.toplevel().unwrap().wl_surface().id();
        if !self.floating_windows.contains_key(&id) {
            self.float_window_internal(window);
        }
    }

    fn float_window_internal(&mut self, window: &Window) {
        let id = window.toplevel().unwrap().wl_surface().id();
        if self.maximized_windows.remove(&id) {
            window.toplevel().unwrap().with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
            });
            window.toplevel().unwrap().send_configure();
        }
        let loc = self.space.element_location(window).unwrap_or_default();
        let size = self
            .space
            .element_geometry(window)
            .map(|g| g.size)
            .unwrap_or_else(|| window.geometry().size);
        self.floating_windows.insert(id, Rectangle::new(loc, size));
        self.space.raise_element(window, true);
    }

    /// Read the current app_id / title for a toplevel (may still be empty on
    /// the first map; callers re-run on commit once properties land).
    pub fn window_identity(&self, window: &Window) -> (String, String) {
        let Some(toplevel) = window.toplevel() else {
            return (String::new(), String::new());
        };
        use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
        with_states(toplevel.wl_surface(), |states| {
            let data = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|d| d.lock().unwrap());
            match data {
                Some(attrs) => (
                    attrs.app_id.clone().unwrap_or_default(),
                    attrs.title.clone().unwrap_or_default(),
                ),
                None => (String::new(), String::new()),
            }
        })
    }

    /// Apply every matching `windowrule` from the live config to `window`.
    ///
    pub fn apply_window_rules(&mut self, window: &Window) {
        let (app_id, title) = self.window_identity(window);
        let id = match window.toplevel() {
            Some(t) => t.wl_surface().id(),
            None => return,
        };

        let rules = self.config_watcher.config.window_rules.clone();
        if rules.is_empty() {
            return;
        }

        let app_l = app_id.to_lowercase();
        let title_l = title.to_lowercase();
        let identity_ready = !app_id.is_empty() || !title.is_empty();

        let mut want_float = false;
        let mut want_center = false;
        let mut opacity: Option<f32> = None;
        let mut workspace: Option<u8> = None;
        let mut any_match = false;
        let mut needs_identity = false;

        for rule in &rules {
            let has_matcher = !rule.app_id.is_empty() || !rule.title.is_empty();
            if has_matcher {
                needs_identity = true;
            }
            let app_ok = rule.app_id.is_empty()
                || app_l.contains(&rule.app_id.to_lowercase());
            let title_ok = rule.title.is_empty()
                || title_l.contains(&rule.title.to_lowercase());
            if !app_ok || !title_ok {
                continue;
            }
            if has_matcher && !identity_ready {
                continue;
            }
            any_match = true;
            if rule.float {
                want_float = true;
            }
            if rule.center {
                want_center = true;
            }
            if let Some(o) = rule.opacity {
                opacity = Some(o);
            }
            if let Some(ws) = rule.workspace {
                workspace = Some(ws);
            }
        }

        if let Some(o) = opacity {
            self.window_opacity.insert(id.clone(), o.clamp(0.0, 1.0));
        }

        if self.window_rules_done.contains(&id) {
            return;
        }
        if !any_match {
            if needs_identity && !identity_ready {
                return;
            }
            if identity_ready || !needs_identity {
                self.window_rules_done.insert(id);
            }
            return;
        }

        if let Some(ws) = workspace {
            let idx = (ws as usize).saturating_sub(1);
            let prev = self.window_workspace.insert(id.clone(), idx);
            if prev != Some(idx) {
                self.goto_workspace(idx);
            }
        }

        if want_float && !self.floating_windows.contains_key(&id) {
            self.float_window_internal(window);
        }

        if want_center && self.floating_windows.contains_key(&id) {
            if let Some(output) = self
                .output_under(Some(self.cursor_position))
                .or_else(|| self.space.outputs().next().cloned())
            {
                if let Some(geo) = self.space.output_geometry(&output) {
                    let size = self
                        .floating_windows
                        .get(&id)
                        .map(|r| r.size)
                        .unwrap_or_else(|| window.geometry().size);
                    let loc = (
                        geo.loc.x + (geo.size.w - size.w).max(0) / 2,
                        geo.loc.y + (geo.size.h - size.h).max(0) / 2,
                    )
                        .into();
                    if let Some(rect) = self.floating_windows.get_mut(&id) {
                        rect.loc = loc;
                    }
                    self.space.map_element(window.clone(), loc, true);
                }
            }
        }

        self.window_rules_done.insert(id);
        crate::life(format!(
            "windowrule: app_id={app_id:?} title={title:?} float={want_float} ws={workspace:?} opacity={opacity:?}"
        ));
        if want_float || workspace.is_some() || want_center {
            self.arrange_windows();
        }
    }

    /// Snap the window being live-tile-resized to its freshly computed layout
    /// target, so the grabbed edge tracks the pointer on the next frame while
    pub fn pin_tile_resize(&mut self, id: &ObjectId) {
        if let Some(anim) = self.animations.get_mut(id) {
            anim.start_loc = anim.target_loc.to_f64();
            anim.start_size = anim.target_size.to_f64();
            anim.start_time = std::time::Instant::now();
        }
    }
    pub fn cycle_focus_index(&mut self, backwards: bool) -> usize {
        self.reconcile_column_order();

        let windows = self.input_columns();

        if windows.is_empty() {
            return 0;
        }

        let current = self.focused_window_index(&windows).unwrap_or(0);

        let next = if backwards {
            (current + windows.len() - 1) % windows.len()
        } else {
            (current + 1) % windows.len()
        };

        self.focus_index(next);
        next
    }

    /// Cycle focus to the next/previous window (Alt+Tab).
    pub fn cycle_focus(&mut self, backwards: bool) {
        self.cycle_focus_index(backwards);
    }

    /// Focus the window in a given direction relative to the focused window.
    /// Direction: "left", "right", "up", "down".
    pub fn focus_direction(&mut self, direction: &str) {
        self.reconcile_column_order();

        if self.config_watcher.config.layout == "scroll" {
            match direction {
                "left" => self.scroll_active(-1),
                "right" => self.scroll_active(1),
                "up" => self.scroll_stack(-1),
                "down" => self.scroll_stack(1),
                _ => {}
            }

            return;
        }

        let windows = self.input_columns();

        let Some(current) = self.focused_window_index(&windows) else {
            return;
        };

        let Some(current_rect) = self.space.element_geometry(&windows[current]) else {
            return;
        };

        let current_x = current_rect.loc.x as f64 + current_rect.size.w as f64 / 2.0;
        let current_y = current_rect.loc.y as f64 + current_rect.size.h as f64 / 2.0;

        let mut best: Option<(f64, usize)> = None;

        for (index, window) in windows.iter().enumerate() {
            if index == current {
                continue;
            }

            let Some(rect) = self.space.element_geometry(window) else {
                continue;
            };

            let dx = rect.loc.x as f64 + rect.size.w as f64 / 2.0 - current_x;
            let dy = rect.loc.y as f64 + rect.size.h as f64 / 2.0 - current_y;

            let matches = match direction {
                "left" => dx < 0.0 && dx.abs() >= dy.abs(),
                "right" => dx > 0.0 && dx.abs() >= dy.abs(),
                "up" => dy < 0.0 && dy.abs() >= dx.abs(),
                "down" => dy > 0.0 && dy.abs() >= dx.abs(),
                _ => false,
            };

            let distance = dx * dx + dy * dy;

            if matches
                && best
                    .map(|(previous, _)| distance < previous)
                    .unwrap_or(true)
            {
                best = Some((distance, index));
            }
        }

        if let Some((_, index)) = best {
            self.focus_index(index);
        }
    }

    /// Cycle the focused window's position in the layout (Alt+Shift as move).
    pub fn move_focus_dir(&mut self, backwards: bool) {
        self.reconcile_column_order();

        if self.config_watcher.config.layout == "scroll" {
            let windows = self.input_columns();
            let columns = self.columns_of(&windows);
            let Some((current_col, _)) = self.focused_column_position(&columns) else {
                return;
            };
            let target = if backwards {
                current_col.saturating_sub(1)
            } else {
                current_col.saturating_add(1).min(columns.len() - 1)
            };
            self.move_focused_to(target);
            return;
        }

        let windows = self.input_columns();

        let Some(current) = self.focused_window_index(&windows) else {
            return;
        };

        let target = if backwards {
            current.saturating_sub(1)
        } else {
            current.saturating_add(1).min(windows.len() - 1)
        };

        self.move_focused_to(target);
    }

    /// Move the focused window up/down within its scroll column: swap it with
    /// the row above/below. Saturates at the stack ends; no-op outside the
    pub fn move_row(&mut self, backwards: bool) {
        if self.config_watcher.config.layout != "scroll" {
            return;
        }

        self.reconcile_column_order();

        let windows = self.input_columns();

        if windows.is_empty() {
            return;
        }

        let columns = self.columns_of(&windows);

        let Some((col, row)) = self.focused_column_position(&columns) else {
            return;
        };

        let next = if backwards {
            row.saturating_sub(1)
        } else {
            row.saturating_add(1).min(columns[col].len() - 1)
        };

        if next == row {
            return;
        }

        let id_a = columns[col][row].toplevel().unwrap().wl_surface().id();
        let id_b = columns[col][next].toplevel().unwrap().wl_surface().id();

        if row.min(next) == 0 {
            self.column_starts.remove(&id_a);
            self.column_starts.remove(&id_b);
            let new_top = columns[col][row.max(next)]
                .toplevel()
                .unwrap()
                .wl_surface()
                .id();
            self.column_starts.insert(new_top.clone());
            self.column_rows.remove(&id_a);
            self.column_rows.remove(&id_b);
            self.column_rows.insert(new_top, next);
        }

        if let (Some(pos_a), Some(pos_b)) = (
            self.column_order.iter().position(|id| id == &id_a),
            self.column_order.iter().position(|id| id == &id_b),
        ) {
            self.column_order.swap(pos_a, pos_b);
        }

        self.focus_window(&columns[col][row]);
        self.arrange_windows();
    }

    /// Stack the focused window under its left neighbor: it leaves its own
    /// column and becomes a row of the previous column. No-op for the first
    pub fn consume_into_column(&mut self) {
        if self.config_watcher.config.layout != "scroll" {
            return;
        }

        self.reconcile_column_order();

        let windows = self.input_columns();

        let Some(current) = self.focused_window_index(&windows) else {
            return;
        };

        if current == 0 {
            return;
        }

        let id = windows[current].toplevel().unwrap().wl_surface().id();

        if !self.column_starts.contains(&id) {
            return;
        }

        self.column_starts.remove(&id);
        self.focus_window(&windows[current]);
        self.arrange_windows();
    }

    /// Lift the focused window out of its stack: it becomes its own column
    /// again (the rows below it form the column after that). No-op for
    pub fn expel_from_column(&mut self) {
        if self.config_watcher.config.layout != "scroll" {
            return;
        }

        self.reconcile_column_order();

        let windows = self.input_columns();

        let Some(current) = self.focused_window_index(&windows) else {
            return;
        };

        let id = windows[current].toplevel().unwrap().wl_surface().id();

        if self.column_starts.contains(&id) {
            return;
        }

        self.column_starts.insert(id);

        if let Some(next) = windows.get(current + 1) {
            let next_id = next.toplevel().unwrap().wl_surface().id();
            self.column_starts.insert(next_id);
        }

        self.focus_window(&windows[current]);
        self.arrange_windows();
    }

    /// Begin the shrink-to-center close animation on the focused window.
    pub fn start_close_focused(&mut self) {
        let Some(surface) = self.current_focus_surface() else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == &surface)
            .cloned()
        else {
            return;
        };
        self.start_close(&window);
    }

    /// Start the close (shrink-fade) animation for `window` and mark it for
    /// destruction; the client is told to close once the animation finishes.
    pub fn start_close(&mut self, window: &smithay::desktop::Window) {
        let id = window.toplevel().unwrap().wl_surface().id();
        if self.pending_close.contains(&id) {
            return;
        }
        let loc = self.space.element_location(window).unwrap_or_default();
        let geo = window.geometry();
        let center_x = loc.x + geo.size.w / 2;
        let center_y = loc.y + geo.size.h / 2;
        self.last_closed_rect = Some(Rectangle::new(loc, geo.size));
        self.animations.insert(
            id.clone(),
            WindowAnimationState {
                start_time: std::time::Instant::now(),
                start_loc: Point::from((loc.x as f64, loc.y as f64)),
                start_size: Size::from((geo.size.w as f64, geo.size.h as f64)),
                start_alpha: 1.0,
                target_loc: Point::from((center_x, center_y)),
                target_size: Size::from((0, 0)),
                target_alpha: 0.0,
                is_closing: true,
            },
        );
        self.pending_close.insert(id);
    }

    pub fn update_animations(&mut self) {
        let config = self.config_watcher.config.clone();
        let duration_ms = if config.animation_enabled {
            config.animation_duration
        } else {
            0.0
        };

        let dead: Vec<Window> = self
            .all_windows
            .iter()
            .filter(|w| !w.toplevel().unwrap().wl_surface().is_alive())
            .cloned()
            .collect();
        let mut reflow = false;
        for w in &dead {
            let id = w.toplevel().unwrap().wl_surface().id();
            if self
                .space
                .elements()
                .any(|x| x.toplevel().unwrap().wl_surface().id() == id)
            {
                let loc = self.space.element_location(w).unwrap_or_default();
                let geo = w.geometry();
                self.last_closed_rect = Some(Rectangle::new(loc, geo.size));
            }
            self.pending_close.remove(&id);
            self.remove_window_bookkeeping(&id);
            self.space.unmap_elem(w);
            let gaps = config.inner_gap.max(0);
            let names: Vec<String> = self.dwindle_roots.keys().cloned().collect();
            for name in names {
                let placeholder = self.dwindle_roots.remove(&name);
                let mut placeholder = match placeholder {
                    Some(root) => Some(root),
                    None => continue,
                };
                crate::layout::dwindle_remove(&mut placeholder, &id, gaps);
                if let Some(root) = placeholder {
                    if !root.leaves().is_empty() {
                        self.dwindle_roots.insert(name, root);
                    }
                }
            }
            reflow = true;
        }

        if reflow {
            self.arrange_windows();
            let target = self.last_closed_rect.take();
            self.focus_closest_window_to(target);
            self.arrange_windows();
        }

        let bezier = CubicBezier::new(
            config.animation_bezier.0,
            config.animation_bezier.1,
            config.animation_bezier.2,
            config.animation_bezier.3,
        );

        let alive_ids: std::collections::HashSet<ObjectId> = self
            .space
            .elements()
            .map(|w| w.toplevel().unwrap().wl_surface().id())
            .collect();

        self.animations.retain(|id, _| alive_ids.contains(id));
        self.pending_close.retain(|id| alive_ids.contains(id));

        let windows: Vec<Window> = self.space.elements().cloned().collect();
        let mut to_close: Vec<Window> = Vec::new();
        for window in windows {
            let id = window.toplevel().unwrap().wl_surface().id();
            if self.dragging_window.as_ref() == Some(&id) {
                continue;
            }
            if self.floating_windows.contains_key(&id)
                && !self.animations.get(&id).is_some_and(|a| a.is_closing)
            {
                continue;
            }
            let pan_off = self.ws_pan_offset_for(&id);
            if let Some(anim) = self.animations.get_mut(&id) {
                let anim_duration = if anim.is_closing {
                    duration_ms * 0.4
                } else {
                    duration_ms
                };
                let elapsed = anim.start_time.elapsed().as_secs_f64() * 1000.0;
                let t = if anim_duration <= 0.0 {
                    1.0
                } else {
                    (elapsed / anim_duration).min(1.0)
                };
                let eased = bezier.ease(t);

                let current_loc: Point<f64, Logical> = Point::from((
                    anim.start_loc.x + (anim.target_loc.x as f64 - anim.start_loc.x) * eased,
                    anim.start_loc.y + (anim.target_loc.y as f64 - anim.start_loc.y) * eased,
                ));
                let current_size: Size<f64, Logical> = Size::from((
                    anim.start_size.w + (anim.target_size.w as f64 - anim.start_size.w) * eased,
                    anim.start_size.h + (anim.target_size.h as f64 - anim.start_size.h) * eased,
                ));

                let map_loc = Point::from((current_loc.x as i32, current_loc.y as i32));
                let map_loc = map_loc + pan_off;
                self.space.map_element(window.clone(), map_loc, false);

                let _ = current_size;

                if t >= 1.0 && anim.is_closing {
                    to_close.push(window);
                }
            }
        }

        if self.ws_pan.is_some() {
            let done = !config.animation_enabled
                || self.ws_pan_progress().is_some_and(|p| p >= 1.0);
            if done {
                if let Some(pan) = self.ws_pan.as_ref() {
                    let gone: Vec<Window> = self
                        .space
                        .elements()
                        .filter(|w| {
                            w.toplevel().is_some_and(|t| {
                                let id = &t.wl_surface().id();
                                self.window_workspace.get(id) == Some(&pan.old)
                                    && self.window_output.get(id).map(String::as_str)
                                        == Some(pan.output.as_str())
                            })
                        })
                        .cloned()
                        .collect();
                    for window in gone {
                        self.space.unmap_elem(&window);
                    }
                }
                self.ws_pan = None;
            }
        }

        if let Some(transition) = self.overview_transition.as_ref() {
            let start = match transition {
                OverviewTransition::Entering { start } => *start,
                OverviewTransition::Exiting { start } => *start,
                OverviewTransition::Shifting { start, .. } => *start,
            };
            if self.anim_progress(start) >= 1.0 {
                self.overview_transition = None;
            }
        }

        let floats: Vec<(Window, Point<i32, Logical>)> = self
            .space
            .elements()
            .filter(|w| {
                let id = w.toplevel().unwrap().wl_surface().id();
                self.dragging_window.as_ref() != Some(&id)
                    && self.floating_windows.contains_key(&id)
                    && !self.animations.get(&id).is_some_and(|a| a.is_closing)
            })
            .filter_map(|w| self.space.element_location(w).map(|l| (w.clone(), l)))
            .collect();
        for (window, location) in floats {
            let id = window.toplevel().map(|t| t.wl_surface().id());
            let base = id
                .as_ref()
                .map(|id| {
                    if self.ws_pan.is_some() {
                        self.floating_windows
                            .get(id)
                            .map(|rect| rect.loc)
                            .unwrap_or(location)
                    } else {
                        location
                    }
                })
                .unwrap_or(location);
            let off = id
                .as_ref()
                .map(|id| self.ws_pan_offset_for(id))
                .unwrap_or_default();
            self.space.map_element(window, base + off, false);
        }

        if let Some(dragged_id) = &self.dragging_window {
            let dragged: Vec<Window> = self
                .space
                .elements()
                .filter(|w| w.toplevel().unwrap().wl_surface().id() == *dragged_id)
                .cloned()
                .collect();
            if let Some(dragged) = dragged.first() {
                if let Some(location) = self.space.element_location(dragged) {
                    self.space.map_element(dragged.clone(), location, true);
                }
            }
        }

        for window in to_close {
            let id = window.toplevel().unwrap().wl_surface().id();
            self.animations.remove(&id);
            self.pending_close.remove(&id);
            window.toplevel().unwrap().send_close();
        }
    }
}

/// Data associated with a wayland client that connects to Smallvil.
/// One instance of this type per client.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        crate::life(format!(
            "wayland client {client_id:?} disconnected ({reason:?})"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cam() -> CanvasCamera {
        CanvasCamera {
            offset_x: 100.0,
            offset_y: -50.0,
            zoom: 2.0,
        }
    }

    #[test]
    fn canvas_camera_maps_world_to_screen() {
        let cam = test_cam();
        let origin = Point::<i32, Logical>::from((1920, 0));
        let world = Rectangle::new((1930, 20).into(), (300, 200).into());
        let screen = cam.to_screen(origin, world);
        assert_eq!(screen.loc, Point::from((2040, -10)));
        assert_eq!(screen.size.w, 600);
        assert_eq!(screen.size.h, 400);
    }

    #[test]
    fn canvas_camera_round_trips() {
        let cam = test_cam();
        let origin = Point::<i32, Logical>::from((1920, 0));
        let world = Rectangle::new((1930, 20).into(), (300, 200).into());
        assert_eq!(cam.to_world(origin, cam.to_screen(origin, world)), world);
        let local = Rectangle::new((10, 20).into(), (300, 200).into());
        let screen = cam.to_screen(Point::default(), local);
        assert_eq!(screen.loc, Point::from((120, -10)));
        assert_eq!(cam.to_world(Point::default(), screen), local);
    }

    #[test]
    fn canvas_camera_scales_physical_rects() {
        let phys = Rectangle::new((10, 20).into(), (300, 200).into());
        let scaled = test_cam().scale_physical(2.0, phys);
        assert_eq!(scaled.loc, Point::from((220, -60)));
        assert_eq!(scaled.size.w, 600);
        assert_eq!(scaled.size.h, 400);
        assert_eq!(CanvasCamera::default().scale_physical(1.0, phys), phys);
    }
}
