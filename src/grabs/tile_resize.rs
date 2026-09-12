//! Tile resize grab for the live tile resize (Super+right-drag on a *tiled*
//! window).

use crate::Smallvil;
use crate::grabs::resize_grab::ResizeEdge;
use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData,
        MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    reexports::wayland_server::Resource,
    utils::{Logical, Point, Rectangle},
};

pub struct TileResizeGrab {
    start_data: PointerGrabStartData<Smallvil>,
    window: Window,
    edges: ResizeEdge,
    initial_rect: Rectangle<i32, Logical>,
}

impl TileResizeGrab {
    pub fn start(
        start_data: PointerGrabStartData<Smallvil>,
        window: Window,
        edges: ResizeEdge,
        initial_rect: Rectangle<i32, Logical>,
    ) -> Self {
        Self {
            start_data,
            window,
            edges,
            initial_rect,
        }
    }

    /// Lowest tile size an edge may shrink a column/tile to. Scroll columns
    /// keep the scroll minimum width; dwindle tiles get the generic tile floor.
    fn min_tile(&self, data: &Smallvil) -> i32 {
        if data.config_watcher.config.layout == "scroll" {
            200
        } else {
            80
        }
    }
}

impl PointerGrab<Smallvil> for TileResizeGrab {
    fn motion(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let min = self.min_tile(data);
        let delta = event.location - self.start_data.location;

        let mut rect = self.initial_rect;

        if self.edges.intersects(ResizeEdge::RIGHT) {
            rect.size.w = (self.initial_rect.size.w as f64 + delta.x) as i32;
            rect.size.w = rect.size.w.max(min);
        } else if self.edges.intersects(ResizeEdge::LEFT) {
            let right = (self.initial_rect.loc.x + self.initial_rect.size.w) as i64;
            let new_x = (self.initial_rect.loc.x as i64 + delta.x as i64)
                .min(right - min as i64);
            rect.loc.x = new_x as i32;
            rect.size.w = (right - new_x) as i32;
        }

        if self.edges.intersects(ResizeEdge::BOTTOM) {
            rect.size.h = (self.initial_rect.size.h as f64 + delta.y) as i32;
            rect.size.h = rect.size.h.max(min);
        } else if self.edges.intersects(ResizeEdge::TOP) {
            let bottom = (self.initial_rect.loc.y + self.initial_rect.size.h) as i64;
            let new_y = (self.initial_rect.loc.y as i64 + delta.y as i64)
                .min(bottom - min as i64);
            rect.loc.y = new_y as i32;
            rect.size.h = (bottom - new_y) as i32;
        }

        let id = self.window.toplevel().unwrap().wl_surface().id();
        data.tile_resize = Some((id.clone(), rect));

        if data.config_watcher.config.layout == "scroll" {
            let anchor = data.column_anchor_for(&id).unwrap_or_else(|| id.clone());
            data.scroll_widths.insert(anchor, rect.size.w);
        }

        data.arrange_windows();
        data.pin_tile_resize(&id);
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

        if let Some((rid, _)) = &data.tile_resize {
            if rid == &id {
                data.tile_resize = None;
            }
        }
        data.arrange_windows();
    }
}