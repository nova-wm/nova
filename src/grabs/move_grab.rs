//! Move grab is the state of a composer during which the client window is being dragged around.
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
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    reexports::wayland_server::Resource,
    utils::{Logical, Point},
};

pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<Smallvil>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
}

impl PointerGrab<Smallvil> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Smallvil,
        handle: &mut PointerInnerHandle<'_, Smallvil>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let delta = event.location - self.start_data.location;
        let zoom = data.canvas_zoom_for_window(&self.window);
        let init = self.initial_window_location.to_f64();
        let new_location = Point::from((init.x + delta.x / zoom, init.y + delta.y / zoom));
        data.space
            .map_element(self.window.clone(), new_location.to_i32_round(), true);
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

        const BTN_LEFT: u32 = 0x110;

        if !handle.current_pressed().contains(&BTN_LEFT) {
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

        let canvas_mode = data.config_watcher.config.layout == "canvas";

        if !data.is_floating(&id) && !canvas_mode {
            if let Some(output) = data.output_under(Some(data.cursor_position)) {
                let dest = output.name();
                let source = data.window_output.get(&id).cloned();

                if let Some(target) = data.window_at(data.cursor_position).filter(|w| {
                    w.toplevel()
                        .map(|t| t.wl_surface().id() != id)
                        .unwrap_or(false)
                }) {
                    let tid = target.toplevel().unwrap().wl_surface().id();
                    data.window_output
                        .insert(tid.clone(), source.unwrap_or_else(|| dest.clone()));
                    if let (Some(i), Some(j)) = (
                        data.column_order.iter().position(|x| x == &id),
                        data.column_order.iter().position(|x| x == &tid),
                    ) {
                        data.column_order.swap(i, j);
                    }
                }

                let ws = data.active_workspace_on(&dest);
                data.window_output.insert(id.clone(), dest);
                data.window_workspace.insert(id.clone(), ws);
            }
        } else if canvas_mode && !data.is_floating(&id) {
            if let Some(output) = data.output_under(Some(data.cursor_position)) {
                let dest = output.name();
                let ws = data.active_workspace_on(&dest);
                data.window_output.insert(id.clone(), dest);
                data.window_workspace.insert(id.clone(), ws);
            }
            data.canvas_save_rect(&self.window, &id);
        } else {
            if let Some(entry) = data.floating_windows.get_mut(&id) {
                if let Some(loc) = data.space.element_location(&self.window) {
                    entry.loc = loc;
                }
            }
            if let Some(output) = data.output_under(Some(data.cursor_position)) {
                let dest = output.name();
                let ws = data.active_workspace_on(&dest);
                data.window_output.insert(id.clone(), dest);
                data.window_workspace.insert(id.clone(), ws);
            }
        }

        if data.dragging_window.as_ref() == Some(&id) {
            data.dragging_window = None;
        }

        let grab_loc = data.space.element_location(&self.window);
        if let Some(grab_loc) = grab_loc {
            if let Some(entry) = data.animations.get_mut(&id) {
                entry.start_loc = grab_loc.to_f64();
                entry.start_time = std::time::Instant::now();
            } else {
                data.animations.insert(
                    id.clone(),
                    crate::state::WindowAnimationState {
                        start_time: std::time::Instant::now(),
                        start_loc: grab_loc.to_f64(),
                        start_size: (0.0, 0.0).into(),
                        start_alpha: 1.0,
                        target_loc: grab_loc,
                        target_size: (0, 0).into(),
                        target_alpha: 1.0,
                        is_closing: false,
                    },
                );
            }
        }

        if data.is_floating(&id) {
            return;
        }

        data.arrange_windows();
    }
}
