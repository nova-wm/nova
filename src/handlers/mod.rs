pub(crate) mod capture;
mod compositor;
mod layer_shell;
mod xdg_shell;

use crate::Smallvil;


use smithay::input::dnd::{DnDGrab, DndGrabHandler, GrabType, Source};
use smithay::input::pointer::Focus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Serial;
use smithay::desktop::{WindowSurfaceType, layer_map_for_output};
use smithay::wayland::background_effect::{Capability, ExtBackgroundEffectHandler};
use smithay::wayland::compositor::with_states;
use smithay::wayland::fractional_scale::{FractionalScaleHandler, with_fractional_scale};
use smithay::wayland::idle_inhibit::IdleInhibitHandler;
use smithay::wayland::idle_notify::IdleNotifierHandler;
use smithay::wayland::output::OutputHandler;
use smithay::wayland::pointer_constraints::PointerConstraintsHandler;
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::utils::{Logical, Size};
use smithay::wayland::selection::{SelectionHandler, SelectionTarget};
use smithay::wayland::selection::data_device::{
    DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus,
};

impl SeatHandler for Smallvil {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Smallvil> {
        &mut self.seat_state
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

impl PointerConstraintsHandler for Smallvil {}


impl FractionalScaleHandler for Smallvil {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let output = self
            .space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .map(|toplevel| toplevel.wl_surface() == &surface)
                    .unwrap_or(false)
            })
            .and_then(|window| self.space.outputs_for_element(window).first().cloned())
            .or_else(|| {
                self.space
                    .outputs()
                    .find(|o| {
                        layer_map_for_output(o)
                            .layer_for_surface(&surface, WindowSurfaceType::TOPLEVEL)
                            .is_some()
                    })
                    .cloned()
            })
            .or_else(|| self.output_under(Some(self.cursor_position)))
            .or_else(|| self.space.outputs().next().cloned());

        if let Some(output) = output {
            let scale = output.current_scale().fractional_scale();
            with_states(&surface, |states| {
                with_fractional_scale(states, |fractional| {
                    fractional.set_preferred_scale(scale);
                });
            });
        }
    }
}


impl IdleInhibitHandler for Smallvil {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.insert(surface.id());
        self.sync_idle_inhibition();
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.remove(&surface.id());
        self.sync_idle_inhibition();
    }
}


impl IdleNotifierHandler for Smallvil {
    fn idle_notifier_state(&mut self) -> &mut smithay::wayland::idle_notify::IdleNotifierState<Self> {
        &mut self.idle_notifier_state
    }
}


impl SessionLockHandler for Smallvil {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        self.session_locked = true;
        self.lock_surfaces.clear();
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Option::<WlSurface>::None, serial);
        }
        confirmation.lock();
        crate::life("session-lock: locked");
    }

    fn unlock(&mut self) {
        self.session_locked = false;
        self.lock_surfaces.clear();
        crate::life("session-lock: unlocked");
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        use smithay::output::Output;
        let out = Output::from_resource(&output);
        let size: Size<u32, Logical> = out
            .as_ref()
            .and_then(|o| self.space.output_geometry(o))
            .map(|g| Size::from((g.size.w.max(1) as u32, g.size.h.max(1) as u32)))
            .unwrap_or_else(|| Size::from((1920u32, 1080u32)));

        surface.with_pending_state(|state| {
            state.size = Some(size);
        });
        surface.send_configure();

        let name = out
            .map(|o| o.name())
            .unwrap_or_else(|| "unknown".to_string());
        crate::life(format!(
            "session-lock: surface on {name} {}x{}",
            size.w, size.h
        ));
        self.lock_surfaces.insert(name, surface);
    }
}


impl ExtBackgroundEffectHandler for Smallvil {
    fn capabilities(&self) -> Capability {
        if self.config_watcher.config.blur {
            Capability::Blur
        } else {
            Capability::empty()
        }
    }
}


impl SelectionHandler for Smallvil {
    type SelectionUserData = ();

    fn send_selection(
        &mut self,
        _type_: SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
        _seat: Seat<Self>,
        _user_data: &(),
    ) {
        if mime_type != "image/png" {
            return;
        }
        let Some(png) = self.clipboard_png.clone() else {
            return;
        };
        let mut file = std::fs::File::from(fd);
        if let Err(error) = std::io::Write::write_all(&mut file, &png) {
            crate::life(format!("screenshot: clipboard serve failed: {error}"));
        }
    }
}

impl DataDeviceHandler for Smallvil {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl DndGrabHandler for Smallvil {}
impl WaylandDndGrabHandler for Smallvil {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        _icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        match type_ {
            GrabType::Pointer => {
                let ptr = seat.get_pointer().unwrap();
                let start_data = ptr.grab_start_data().unwrap();

                let grab = DnDGrab::new_pointer(&self.display_handle, start_data, source, seat);
                ptr.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => {
                source.cancel();
            }
        }
    }
}


impl OutputHandler for Smallvil {}

smithay::delegate_dispatch2!(Smallvil);
