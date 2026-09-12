//! Resize grab is the state of a composer during which the client window is being resized.
//!

use crate::Smallvil;
use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData,
        MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    reexports::wayland_server::Resource,
    utils::{Logical, Point, Rectangle, Size},
    wayland::{compositor, shell::xdg::SurfaceCachedState},
};
use std::cell::RefCell;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ResizeEdge: u32 {
        const TOP          = 0b0001;
        const BOTTOM       = 0b0010;
        const LEFT         = 0b0100;
        const RIGHT        = 0b1000;

        const TOP_LEFT     = Self::TOP.bits() | Self::LEFT.bits();
        const BOTTOM_LEFT  = Self::BOTTOM.bits() | Self::LEFT.bits();

        const TOP_RIGHT    = Self::TOP.bits() | Self::RIGHT.bits();
        const BOTTOM_RIGHT = Self::BOTTOM.bits() | Self::RIGHT.bits();
    }
}

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    #[inline]
    fn from(x: xdg_toplevel::ResizeEdge) -> Self {
        Self::from_bits(x as u32).unwrap()
    }
}

pub struct ResizeSurfaceGrab {
    start_data: PointerGrabStartData<Smallvil>,
    window: Window,

    edges: ResizeEdge,

    initial_rect: Rectangle<i32, Logical>,
    last_window_size: Size<i32, Logical>,
}

impl ResizeSurfaceGrab {
    pub fn start(
        start_data: PointerGrabStartData<Smallvil>,
        window: Window,
        edges: ResizeEdge,
        initial_window_rect: Rectangle<i32, Logical>,
    ) -> Self {
        let initial_rect = initial_window_rect;

        ResizeSurfaceState::with(window.toplevel().unwrap().wl_surface(), |state| {
            *state = ResizeSurfaceState::Resizing {
                edges,
                initial_rect,
            };
        });

        Self {
            start_data,
            window,
            edges,
            initial_rect,
            last_window_size: initial_rect.size,
        }
    }
}

impl PointerGrab<Smallvil> for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let mut delta = event.location - self.start_data.location;
        let zoom = data.canvas_zoom_for_window(&self.window);
        delta.x /= zoom;
        delta.y /= zoom;

        let (min_size, max_size) =
            compositor::with_states(self.window.toplevel().unwrap().wl_surface(), |states| {
                let mut guard = states.cached_state.get::<SurfaceCachedState>();
                let data = guard.current();
                (data.min_size, data.max_size)
            });

        let min_width = min_size.w.max(1);
        let min_height = min_size.h.max(1);

        let max_width = if max_size.w == 0 {
            i32::MAX
        } else {
            max_size.w
        };
        let max_height = if max_size.h == 0 {
            i32::MAX
        } else {
            max_size.h
        };

        let mut rect = self.initial_rect;

        if self.edges.intersects(ResizeEdge::RIGHT) {
            let new_w = (self.initial_rect.size.w as f64 + delta.x) as i32;
            rect.size.w = new_w.max(min_width).min(max_width);
        } else if self.edges.intersects(ResizeEdge::LEFT) {
            let right = (self.initial_rect.loc.x + self.initial_rect.size.w) as i64;
            let desired_w = (self.initial_rect.size.w as f64 - delta.x) as i32;
            let clamped_w = desired_w.max(min_width).min(max_width);
            let new_x = (right - clamped_w as i64) as i32;
            rect.loc.x = new_x;
            rect.size.w = clamped_w;
        }

        if self.edges.intersects(ResizeEdge::BOTTOM) {
            let new_h = (self.initial_rect.size.h as f64 + delta.y) as i32;
            rect.size.h = new_h.max(min_height).min(max_height);
        } else if self.edges.intersects(ResizeEdge::TOP) {
            let bottom = (self.initial_rect.loc.y + self.initial_rect.size.h) as i64;
            let desired_h = (self.initial_rect.size.h as f64 - delta.y) as i32;
            let clamped_h = desired_h.max(min_height).min(max_height);
            let new_y = (bottom - clamped_h as i64) as i32;
            rect.loc.y = new_y;
            rect.size.h = clamped_h;
        }

        self.last_window_size = rect.size;

        let xdg = self.window.toplevel().unwrap();
        xdg.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
            state.size = Some(self.last_window_size);
        });

        xdg.send_pending_configure();

        if self.edges.intersects(ResizeEdge::TOP_LEFT) {
            data.space
                .map_element(self.window.clone(), rect.loc, false);
        }

        let id = xdg.wl_surface().id();
        if let Some(entry) = data.floating_windows.get_mut(&id) {
            *entry = rect;
        }
        if data.config_watcher.config.layout == "canvas" && !data.is_floating(&id) {
            data.canvas_save_rect_at(&self.window, &id, rect);
        }
        if let Some(anim) = data.animations.get_mut(&id) {
            anim.start_loc = rect.loc.to_f64();
            anim.target_loc = rect.loc;
            anim.start_size = rect.size.to_f64();
            anim.target_size = rect.size;
            anim.start_time = std::time::Instant::now();
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);

        if !handle
            .current_pressed()
            .contains(&self.start_data.button)
        {
            handle.unset_grab(self, data, event.serial, event.time, true);

            let xdg = self.window.toplevel().unwrap();
            xdg.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Resizing);
                state.size = Some(self.last_window_size);
            });

            xdg.send_pending_configure();

            ResizeSurfaceState::with(xdg.wl_surface(), |state| {
                *state = ResizeSurfaceState::WaitingForLastCommit {
                    edges: self.edges,
                    initial_rect: self.initial_rect,
                };
            });
        }
    }

    fn axis(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut Smallvil, handle: &mut PointerInnerHandle<'_, Smallvil>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event)
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event)
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event)
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event)
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event)
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event)
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event)
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event)
    }

    fn start_data(&self) -> &PointerGrabStartData<Smallvil> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut Smallvil) {
        let id = self.window.toplevel().unwrap().wl_surface().id();
        if let Some(entry) = data.floating_windows.get_mut(&id) {
            if let Some(loc) = data.space.element_location(&self.window) {
                entry.loc = loc;
            }
        }
        if data.config_watcher.config.layout == "canvas" && !data.is_floating(&id) {
            data.canvas_save_rect(&self.window, &id);
        }
    }
}

/// State of the resize operation.
///
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
enum ResizeSurfaceState {
    #[default]
    Idle,
    Resizing {
        edges: ResizeEdge,
        /// The initial window size and location.
        initial_rect: Rectangle<i32, Logical>,
    },
    /// Resize is done, we are now waiting for last commit, to do the final move
    WaitingForLastCommit {
        edges: ResizeEdge,
        /// The initial window size and location.
        initial_rect: Rectangle<i32, Logical>,
    },
}

impl ResizeSurfaceState {
    fn with<F, T>(surface: &WlSurface, cb: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        compositor::with_states(surface, |states| {
            states.data_map.insert_if_missing(RefCell::<Self>::default);
            let state = states.data_map.get::<RefCell<Self>>().unwrap();

            cb(&mut state.borrow_mut())
        })
    }

    fn commit(&mut self) -> Option<(ResizeEdge, Rectangle<i32, Logical>)> {
        match *self {
            Self::Resizing {
                edges,
                initial_rect,
            } => Some((edges, initial_rect)),
            Self::WaitingForLastCommit {
                edges,
                initial_rect,
            } => {
                *self = Self::Idle;

                Some((edges, initial_rect))
            }
            Self::Idle => None,
        }
    }
}

/// Should be called on `WlSurface::commit`
pub fn handle_commit(data: &mut Smallvil, surface: &WlSurface) -> Option<()> {
    let window = data
        .space
        .elements()
        .find(|w| w.toplevel().unwrap().wl_surface() == surface)
        .cloned()?;

    let mut window_loc = data.space.element_location(&window)?;
    let geometry = window.geometry();

    let new_loc: Point<Option<i32>, Logical> =
        ResizeSurfaceState::with(surface, |state| {
            match state.commit() {
                None => Default::default(),
                Some((edges, initial_rect)) => {
                    if !edges.intersects(ResizeEdge::TOP_LEFT)
                        || (geometry.size.w == initial_rect.size.w
                            && geometry.size.h == initial_rect.size.h)
                    {
                        return Default::default();
                    }
                    let new_x = edges.intersects(ResizeEdge::LEFT).then_some(
                        initial_rect.loc.x + (initial_rect.size.w - geometry.size.w),
                    );
                    let new_y = edges.intersects(ResizeEdge::TOP).then_some(
                        initial_rect.loc.y + (initial_rect.size.h - geometry.size.h),
                    );
                    (new_x, new_y).into()
                }
            }
        });

    if let Some(new_x) = new_loc.x {
        window_loc.x = new_x;
    }
    if let Some(new_y) = new_loc.y {
        window_loc.y = new_y;
    }

    let moved = new_loc.x.is_some() || new_loc.y.is_some();
    if moved {
        data.space
            .map_element(window.clone(), window_loc, false);
    }

    let id = window.toplevel().unwrap().wl_surface().id();
    let size = data
        .space
        .element_geometry(&window)
        .map(|g| g.size)
        .unwrap_or(geometry.size);
    let rect = Rectangle::new(window_loc, size);

    if data.floating_windows.contains_key(&id) {
        data.floating_windows.insert(id.clone(), rect);
    }

    if moved {
        if data.config_watcher.config.layout == "canvas" && !data.is_floating(&id) {
            data.canvas_save_rect_at(&window, &id, rect);
        }
        if let Some(anim) = data.animations.get_mut(&id) {
            anim.start_loc = rect.loc.to_f64();
            anim.target_loc = rect.loc;
            anim.start_size = rect.size.to_f64();
            anim.target_size = rect.size;
            anim.start_time = std::time::Instant::now();
        }
    }

    Some(())
}
