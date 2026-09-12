use std::time::{Duration, Instant};

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    reexports::wayland_server::Resource,
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
};

use crate::grabs::MoveSurfaceGrab;
use crate::state::Smallvil;

#[derive(Debug, Clone)]
enum NovaInputAction {
    SpawnTerminal,
    CloseFocused,
    Focus(String),
    MoveFocus(bool),
    CycleFocus(bool),
    ToggleScratchpad,
    Fullscreen,
    ToggleFloat,
    CanvasZoomIn,
    CanvasZoomOut,
    CanvasZoomReset,
    MoveRow(bool),
    ConsumeIntoColumn,
    ExpelFromColumn,
    ToggleOverview,
    Workspace(usize),
    CanvasCenterWindow,
    CanvasZoomToFit,
    CanvasNudge(i32, i32),
    CanvasPan(f64, f64),
    TogglePin,
    CanvasHome,
    OverviewNav(isize, isize),
    OverviewConfirm,
    OverviewCancel,
    OverviewIgnore,
    ScreenshotConfirm,
    ScreenshotCancel,
    RecordToggle,
    RecordDigit(u8),
    RecordMove(i8),
    RecordEnter,
    RecordCancel,
    /// Run an arbitrary shell command (`spawn <cmdline>`).
    Spawn(String),
    /// Show the welcome window (`welcome`).
    Welcome,
}

impl Smallvil {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        self.notify_idle_activity();

        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time(&event);
                let key_code = event.key_code();
                let key_state = event.state();

                tracing::debug!(?key_code, ?key_state, "raw keyboard event");

                let Some(keyboard) = self.seat.get_keyboard() else {
                    crate::life(format!(
                        "INPUT ERROR: keyboard event received but \
                         seat has no keyboard: code={key_code:?}"
                    ));
                    return;
                };

                if let Some(layer_surface) = self.exclusive_keyboard_layer() {
                    keyboard.set_focus(self, Some(layer_surface), serial);
                    keyboard.input::<(), _>(self, key_code, key_state, serial, time, |_, _, _| {
                        FilterResult::Forward
                    });
                    return;
                }

                if self.is_session_locked() {
                    let focus = self
                        .output_under(Some(self.cursor_position))
                        .and_then(|o| self.lock_surface_for(&o))
                        .map(|s| s.wl_surface().clone())
                        .or_else(|| {
                            self.lock_surfaces
                                .values()
                                .next()
                                .map(|s| s.wl_surface().clone())
                        });
                    keyboard.set_focus(self, focus, serial);
                    keyboard.input::<(), _>(self, key_code, key_state, serial, time, |_, _, _| {
                        FilterResult::Forward
                    });
                    return;
                }

                let action = keyboard.input::<NovaInputAction, _>(
                    self,
                    key_code,
                    key_state,
                    serial,
                    time,
                    |state, modifiers, handle| {
                        if key_state != KeyState::Pressed {
                            return FilterResult::Forward;
                        }

                        let sym = handle.raw_latin_sym_or_raw_current_sym();

                        if state.screenshot_select.is_some() {
                            use xkbcommon::xkb::{keysym_from_name, KEYSYM_NO_FLAGS};

                            if sym == Some(keysym_from_name("Escape", KEYSYM_NO_FLAGS)) {
                                return FilterResult::Intercept(
                                    NovaInputAction::ScreenshotCancel,
                                );
                            }

                            let confirm = sym
                                == Some(keysym_from_name("Return", KEYSYM_NO_FLAGS))
                                || sym == Some(keysym_from_name("KP_Enter", KEYSYM_NO_FLAGS))
                                || sym == Some(keysym_from_name("space", KEYSYM_NO_FLAGS));
                            if confirm {
                                return FilterResult::Intercept(
                                    NovaInputAction::ScreenshotConfirm,
                                );
                            }

                            return FilterResult::Intercept(NovaInputAction::OverviewIgnore);
                        }

                        if state.record_picker.is_some() {
                            use xkbcommon::xkb::{keysym_from_name, KEYSYM_NO_FLAGS};

                            if sym == Some(keysym_from_name("Escape", KEYSYM_NO_FLAGS)) {
                                return FilterResult::Intercept(
                                    NovaInputAction::RecordCancel,
                                );
                            }

                            for digit in 1u8..=9u8 {
                                let name = digit.to_string();
                                let keypad = format!("KP_{digit}");
                                let main = keysym_from_name(name.as_str(), KEYSYM_NO_FLAGS);
                                let pad = keysym_from_name(keypad.as_str(), KEYSYM_NO_FLAGS);
                                if sym == Some(main) || sym == Some(pad) {
                                    return FilterResult::Intercept(
                                        NovaInputAction::RecordDigit(digit),
                                    );
                                }
                            }

                            let left = sym == Some(keysym_from_name("Left", KEYSYM_NO_FLAGS))
                                || sym == Some(keysym_from_name("KP_Left", KEYSYM_NO_FLAGS));
                            let right = sym == Some(keysym_from_name("Right", KEYSYM_NO_FLAGS))
                                || sym == Some(keysym_from_name("KP_Right", KEYSYM_NO_FLAGS));
                            let up = sym == Some(keysym_from_name("Up", KEYSYM_NO_FLAGS))
                                || sym == Some(keysym_from_name("KP_Up", KEYSYM_NO_FLAGS));
                            let down = sym == Some(keysym_from_name("Down", KEYSYM_NO_FLAGS))
                                || sym == Some(keysym_from_name("KP_Down", KEYSYM_NO_FLAGS));
                            if left || up {
                                return FilterResult::Intercept(NovaInputAction::RecordMove(-1));
                            }
                            if right || down {
                                return FilterResult::Intercept(NovaInputAction::RecordMove(1));
                            }

                            let confirm = sym
                                == Some(keysym_from_name("Return", KEYSYM_NO_FLAGS))
                                || sym == Some(keysym_from_name("KP_Enter", KEYSYM_NO_FLAGS))
                                || sym == Some(keysym_from_name("space", KEYSYM_NO_FLAGS));
                            if confirm {
                                return FilterResult::Intercept(NovaInputAction::RecordEnter);
                            }

                            return FilterResult::Intercept(NovaInputAction::OverviewIgnore);
                        }

                        if state.overview_active {
                            use xkbcommon::xkb::{keysym_from_name, KEYSYM_NO_FLAGS};

                            let nav = [
                                ("Left", (-1isize, 0isize)),
                                ("Right", (1isize, 0isize)),
                                ("Up", (0isize, -1isize)),
                                ("Down", (0isize, 1isize)),
                            ];
                            for (name, (dx, dy)) in nav {
                                if sym == Some(keysym_from_name(name, KEYSYM_NO_FLAGS)) {
                                    return FilterResult::Intercept(
                                        NovaInputAction::OverviewNav(dx, dy),
                                    );
                                }
                            }

                            let confirm = sym
                                == Some(keysym_from_name("Return", KEYSYM_NO_FLAGS))
                                || sym == Some(keysym_from_name("KP_Enter", KEYSYM_NO_FLAGS));
                            if confirm {
                                return FilterResult::Intercept(
                                    NovaInputAction::OverviewConfirm,
                                );
                            }

                            if sym == Some(keysym_from_name("Escape", KEYSYM_NO_FLAGS)) {
                                return FilterResult::Intercept(
                                    NovaInputAction::OverviewCancel,
                                );
                            }
                        }

                        let config = state.config_watcher.config.clone();

                        for bind in &config.keybindings {
                            if bind.is_mouse() {
                                continue;
                            }
                            let Some(bind_sym) = bind.key_sym else {
                                continue;
                            };
                            if sym != Some(bind_sym) {
                                continue;
                            }

                            let state_bits = (
                                modifiers.shift,
                                modifiers.ctrl,
                                modifiers.alt,
                                modifiers.logo,
                            );
                            if bind_modifier_bits(&bind.modifiers, &config.modifier)
                                != state_bits
                            {
                                continue;
                            }

                            if is_canvas_action(&bind.action) && config.layout != "canvas" {
                                continue;
                            }

                            tracing::debug!(key = ?bind.key, action = ?bind.action, args = ?bind.args, "compositor shortcut");

                            if let Some(action) =
                                parse_keybind_action(&bind.action, &bind.args)
                            {
                                return FilterResult::Intercept(action);
                            }
                        }

                        if state.overview_active {
                            return FilterResult::Intercept(NovaInputAction::OverviewIgnore);
                        }

                        if modifiers.shift || modifiers.ctrl || modifiers.alt || modifiers.logo
                        {
                            tracing::debug!(
                                "keybind: no match for sym={sym:?} mods=shift:{} ctrl:{} alt:{} logo:{}",
                                modifiers.shift,
                                modifiers.ctrl,
                                modifiers.alt,
                                modifiers.logo,
                            );
                        }

                        FilterResult::Forward
                    },
                );

                let modifiers = keyboard.modifier_state();

                if !modifiers.alt && !modifiers.logo {
                    self.alt_tab_active = false;
                }

                if let Some(action) = action {
                    self.nova_execute_input_action(action);
                }
            }

            InputEvent::PointerMotion { event, .. } => {
                let delta = event.delta();

                let requested: Point<f64, Logical> = (
                    self.cursor_position.x + delta.x,
                    self.cursor_position.y + delta.y,
                )
                    .into();

                self.cursor_position = self.nova_clamp_pointer_to_outputs(requested);

                let motion = MotionEvent {
                    location: self.cursor_position,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: Event::time(&event),
                };

                self.nova_dispatch_pointer_motion(motion);
            }

            InputEvent::PointerMotionAbsolute { event, .. } => {
                let output = self
                    .output_under(Some(self.cursor_position))
                    .or_else(|| self.space.outputs().next().cloned());

                let Some(output) = output else {
                    return;
                };

                let Some(geometry) = self.space.output_geometry(&output) else {
                    return;
                };

                let requested = event.position_transformed(geometry.size) + geometry.loc.to_f64();

                self.cursor_position = self.nova_clamp_pointer_to_outputs(requested);

                let motion = MotionEvent {
                    location: self.cursor_position,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: Event::time(&event),
                };

                self.nova_dispatch_pointer_motion(motion);
            }

            InputEvent::PointerButton { event, .. } => {
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };

                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();

                if self.is_session_locked() {
                    let lock_focus: Option<WlSurface> = self
                        .output_under(Some(self.cursor_position))
                        .and_then(|o| self.lock_surface_for(&o))
                        .map(|s| s.wl_surface().clone())
                        .or_else(|| {
                            self.lock_surfaces
                                .values()
                                .next()
                                .map(|s| s.wl_surface().clone())
                        });
                    pointer.button(
                        self,
                        &ButtonEvent {
                            button,
                            state: button_state,
                            serial,
                            time: Event::time(&event),
                        },
                    );
                    pointer.frame(self);
                    if button_state == ButtonState::Pressed {
                        if let Some(keyboard) = self.seat.get_keyboard() {
                            keyboard.set_focus(self, lock_focus, serial);
                        }
                    }
                    return;
                }

                if self.screenshot_select.is_some() {
                    self.screenshot_button(button, button_state);
                    return;
                }

                if self.record_picker.is_some() {
                    self.record_button(button, button_state);
                    return;
                }

                if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
                    let click_hit = self.surface_under(self.cursor_position);
                    crate::life(format!(
                        "CLICK pos=({:.0},{:.0}) hit={} stack:{}",
                        self.cursor_position.x,
                        self.cursor_position.y,
                        click_hit
                            .map(|(s, p)| format!(
                                "{:?}@({:.0},{:.0})",
                                s.id(),
                                p.x,
                                p.y
                            ))
                            .unwrap_or_else(|| "none".to_string()),
                        self.debug_layers_at(self.cursor_position),
                    ));

                    let upper_layer =
                        self.focusable_layer_under(self.cursor_position, true);

                    let pick = self.canvas_pick_pos(self.cursor_position);
                    let under = self.space.element_under(pick)
                        .filter(|(w, _)| {
                            !self.is_hidden_scratchpad(w)
                                && self.window_owned_at(w, self.cursor_position)
                        })
                        .map(|(w, p)| (w.clone(), p))
                        .or_else(|| {
                            let cx = pick.x as i32;
                            let cy = pick.y as i32;
                            self.space.elements().find_map(|window| {
                                if self.is_hidden_scratchpad(window)
                                    || !self.window_owned_at(window, self.cursor_position)
                                {
                                    return None;
                                }
                                let loc = self.space.element_location(window)?;
                                let geo = self.space.element_geometry(window)
                                    .map(|g| g.size)
                                    .unwrap_or_else(|| window.geometry().size);
                                if cx >= loc.x && cx < loc.x + geo.w && cy >= loc.y && cy < loc.y + geo.h {
                                    Some((window.clone(), (cx - loc.x, cy - loc.y).into()))
                                } else {
                                    None
                                }
                            })
                        });

                    if self.overview_active {
                        if button_state == ButtonState::Pressed && button == 272 {
                            if let Some((window, _)) = &under {
                                if let Some(id) =
                                    window.toplevel().map(|t| t.wl_surface().id())
                                {
                                    if let Some(ws) =
                                        self.window_workspace.get(&id).copied()
                                    {
                                        self.exit_overview(Some(ws));
                                    }
                                }
                            }
                        }

                        return;
                    }

                    let (shift, ctrl, alt, logo) = self
                        .seat
                        .get_keyboard()
                        .map(|k| {
                            let m = k.modifier_state();
                            (m.shift, m.ctrl, m.alt, m.logo)
                        })
                        .unwrap_or_default();
                    let config = self.config_watcher.config.clone();
                    let drag_binding = if upper_layer.is_some() {
                        None
                    } else {
                        config
                            .keybindings
                            .iter()
                            .find(|b| b.is_mouse() && b.mouse_button == Some(button))
                    };

                    if let Some(bind) = drag_binding {
                        if bind_modifier_bits(&bind.modifiers, &config.modifier)
                            == (shift, ctrl, alt, logo)
                        {
                            tracing::debug!(
                                "DRAG: bindm match button={button} modifiers=({shift},{ctrl},{alt},{logo}) under={}",
                                under.is_some(),
                            );
                            if let Some((window, loc)) = under {
                                let window = window.clone();
                                let initial_window_location =
                                    self.space.element_location(&window).unwrap_or_default();

                                use smithay::input::pointer::Focus;

                                let focus_data = window
                                    .toplevel()
                                    .map(|t| (t.wl_surface().clone(), loc.to_f64()));

                                if bind.action == "resize_window" {
                                    use crate::grabs::resize_grab::{
                                        ResizeEdge, ResizeSurfaceGrab,
                                    };
                                    use crate::grabs::tile_resize::TileResizeGrab;

                                    let id = window.toplevel().unwrap().wl_surface().id();

                                    let grabbed_rect = self
                                        .space
                                        .element_geometry(&window)
                                        .unwrap_or_else(|| {
                                            Rectangle::new(
                                                self.space
                                                    .element_location(&window)
                                                    .unwrap_or_default(),
                                                window.geometry().size,
                                            )
                                        });

                                    let cx = grabbed_rect.loc.x + grabbed_rect.size.w / 2;
                                    let cy = grabbed_rect.loc.y + grabbed_rect.size.h / 2;
                                    let cursor =
                                        self.canvas_pick_pos(self.cursor_position);

                                    let mut edges = ResizeEdge::empty();
                                    if cursor.x >= cx as f64 {
                                        edges |= ResizeEdge::RIGHT;
                                    } else {
                                        edges |= ResizeEdge::LEFT;
                                    }
                                    if self.config_watcher.config.layout != "scroll" {
                                        if cursor.y >= cy as f64 {
                                            edges |= ResizeEdge::BOTTOM;
                                        } else {
                                            edges |= ResizeEdge::TOP;
                                        }
                                    }

                                    let start_data = smithay::input::pointer::GrabStartData {
                                        focus: focus_data,
                                        button,
                                        location: self.cursor_position,
                                    };

                                    if self.is_floating(&id)
                                        || self.config_watcher.config.layout == "canvas"
                                    {
                                        let grab = ResizeSurfaceGrab::start(
                                            start_data,
                                            window.clone(),
                                            edges,
                                            grabbed_rect,
                                        );
                                        tracing::debug!(
                                            "RESIZE: free grab {} edges={edges:?}",
                                            window.id(),
                                        );
                                        pointer.set_grab(self, grab, serial, Focus::Clear);
                                        return;
                                    }

                                    self.focus_window(&window);
                                    let grab = TileResizeGrab::start(
                                        start_data,
                                        window.clone(),
                                        edges,
                                        grabbed_rect,
                                    );
                                    tracing::debug!(
                                        "RESIZE: tiled grab {} edges={edges:?}",
                                        window.id(),
                                    );
                                    pointer.set_grab(self, grab, serial, Focus::Clear);
                                    return;
                                }

                                let grab = MoveSurfaceGrab {
                                    start_data: smithay::input::pointer::GrabStartData {
                                        focus: focus_data,
                                        button,
                                        location: self.cursor_position,
                                    },
                                    window: window.clone(),
                                    initial_window_location,
                                };
                                tracing::debug!(
                                    "DRAG: grabbing {} button={button}",
                                    grab.window.id(),
                                );
                                self.dragging_window =
                                    Some(grab.window.toplevel().unwrap().wl_surface().id());
                                pointer.set_grab(self, grab, serial, Focus::Clear);
                                return;
                            } else if self.config_watcher.config.layout == "canvas"
                                && bind.action == "move_window"
                            {
                                if let Some(output) =
                                    self.output_under(Some(self.cursor_position))
                                {
                                    self.canvas_pan = Some((
                                        output.name(),
                                        self.cursor_position,
                                        button,
                                    ));
                                }
                            } else {
                                tracing::debug!("DRAG: bindm matched but nothing under cursor");
                            }
                        } else {
                            tracing::debug!(
                                "DRAG: bindm matched but modifiers mismatch ({shift},{ctrl},{alt},{logo}) vs target {:?}",
                                bind_modifier_bits(&bind.modifiers, &config.modifier),
                            );
                        }
                    } else {
                        tracing::debug!(
                            "DRAG: no bindm for button={button} (state={:?})",
                            button_state,
                        );
                    }

                    let window = under.map(|(window, _)| window.clone());

                    if let Some(layer_surface) = upper_layer {
                        if let Some(keyboard) = self.seat.get_keyboard() {
                            keyboard.set_focus(self, Some(layer_surface), serial);
                        }
                    } else if let Some(window) = window {
                        let clicked_is_floating = window
                            .toplevel()
                            .map(|t| self.is_floating(&t.wl_surface().id()))
                            .unwrap_or(false);
                        if clicked_is_floating || self.floating_windows.is_empty() {
                            self.space.raise_element(&window, true);
                        }

                        if let Some(keyboard) = self.seat.get_keyboard() {
                            let focus = window
                                .toplevel()
                                .map(|toplevel| toplevel.wl_surface().clone());

                            keyboard.set_focus(self, focus, serial);
                        }
                    } else if let Some(layer_surface) =
                        self.focusable_layer_under(self.cursor_position, false)
                    {
                        if let Some(keyboard) = self.seat.get_keyboard() {
                            keyboard.set_focus(self, Some(layer_surface), serial);
                        }
                    } else if let Some(keyboard) = self.seat.get_keyboard() {
                        keyboard.set_focus(self, Option::<WlSurface>::None, serial);
                    }
                }

                if button_state == ButtonState::Released
                    && self
                        .canvas_pan
                        .as_ref()
                        .is_some_and(|(_, _, pan_button)| *pan_button == button)
                {
                    self.canvas_pan = None;
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: Event::time(&event),
                    },
                );

                pointer.frame(self);
            }

            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();

                let horizontal_v120 = event.amount_v120(Axis::Horizontal);

                let vertical_v120 = event.amount_v120(Axis::Vertical);

                let horizontal = event
                    .amount(Axis::Horizontal)
                    .unwrap_or_else(|| horizontal_v120.unwrap_or(0.0) * 15.0 / 120.0);

                let vertical = event
                    .amount(Axis::Vertical)
                    .unwrap_or_else(|| vertical_v120.unwrap_or(0.0) * 15.0 / 120.0);

                let (mod_down, shift_down, ctrl_down) = self
                    .seat
                    .get_keyboard()
                    .map(|keyboard| {
                        let modifiers = keyboard.modifier_state();

                        (
                            modifiers.alt || modifiers.logo,
                            modifiers.shift,
                            modifiers.ctrl,
                        )
                    })
                    .unwrap_or((false, false, false));

                if mod_down {
                    if self.config_watcher.config.layout == "canvas" {
                        let horizontal_gesture = horizontal.abs() > vertical.abs();
                        if shift_down && !horizontal_gesture {
                            let now = Instant::now();
                            let cooldown = Duration::from_millis(150);
                            let ready = self
                                .last_ws_time
                                .map(|last| last.elapsed() >= cooldown)
                                .unwrap_or(true);
                            if vertical != 0.0 && ready {
                                self.last_ws_time = Some(now);
                                self.switch_workspace(-(vertical.signum() as isize));
                            }
                            return;
                        }
                        if horizontal_gesture {
                            if horizontal != 0.0 {
                                self.canvas_pan_by(-horizontal, 0.0);
                            }
                            return;
                        }
                        let notches = vertical_v120.unwrap_or(0.0) / 120.0;
                        let smooth = if notches == 0.0 { -vertical / 50.0 } else { 0.0 };
                        let power = (-notches + smooth).clamp(-3.0, 3.0);
                        if power != 0.0 {
                            self.canvas_zoom_by(1.15f64.powf(power));
                        }
                        return;
                    }

                    let now = Instant::now();
                    let cooldown = Duration::from_millis(150);

                    let horizontal_gesture = horizontal.abs() > vertical.abs();

                    let scroll_layout = self.config_watcher.config.layout == "scroll";

                    let step = if horizontal_gesture { horizontal } else { vertical };

                    let page_columns = if scroll_layout {
                        !(ctrl_down && !horizontal_gesture)
                    } else {
                        shift_down || horizontal_gesture
                    };

                    if page_columns {
                        let ready = self
                            .last_col_time
                            .map(|last| last.elapsed() >= cooldown)
                            .unwrap_or(true);

                        if step != 0.0 && ready {
                            self.last_col_time = Some(now);

                            tracing::debug!(
                                "keybind: Mod+scroll column \
                                 shift={shift_down} step={step}"
                            );

                            if scroll_layout {
                                if !horizontal_gesture && !shift_down {
                                    self.scroll_stack(step.signum() as isize);
                                } else {
                                    self.scroll_active(step.signum() as isize);
                                }
                            } else {
                                self.focus_direction(if step < 0.0 { "left" } else { "right" });
                            }
                        }
                    } else {
                        let ready = self
                            .last_ws_time
                            .map(|last| last.elapsed() >= cooldown)
                            .unwrap_or(true);

                        if step != 0.0 && ready {
                            self.last_ws_time = Some(now);

                            tracing::debug!(
                                "keybind: Mod+scroll workspace \
                                 step={step}"
                            );

                            self.switch_workspace(-(step.signum() as isize));
                        }
                    }

                    return;
                }

                let mut frame = AxisFrame::new(Event::time(&event)).source(source);

                if horizontal != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal);
                }

                if let Some(value) = horizontal_v120 {
                    if value != 0.0 {
                        frame = frame.v120(Axis::Horizontal, value as i32);
                    }
                }

                if vertical != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical);
                }

                if let Some(value) = vertical_v120 {
                    if value != 0.0 {
                        frame = frame.v120(Axis::Vertical, value as i32);
                    }
                }

                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }

                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
            }

            _ => {}
        }
    }

    fn nova_execute_input_action(&mut self, action: NovaInputAction) {
        tracing::debug!("keybind: {action:?}");

        let now = Instant::now();
        let debounce = Duration::from_millis(150);

        match action {
            NovaInputAction::SpawnTerminal => {
                let ready = self
                    .last_spawn_time
                    .map(|last| last.elapsed() >= debounce)
                    .unwrap_or(true);

                if !ready {
                    return;
                }

                self.last_spawn_time = Some(now);

                let socket = self.socket_name.clone();

                let terminal = self.config_watcher.config.terminal.clone();
                let mut parts = terminal.split_whitespace();
                let binary = parts.next().unwrap_or("foot");
                let args: Vec<String> = parts.map(str::to_string).collect();

                let spawned = crate::spawn_wayland_client(binary, &args, &socket);

                crate::life(format!(
                    "keybind: terminal `{terminal}` spawn result={spawned}"
                ));
            }

            NovaInputAction::Spawn(command) => {
                let ready = self
                    .last_spawn_time
                    .map(|last| last.elapsed() >= debounce)
                    .unwrap_or(true);

                if !ready {
                    return;
                }

                self.last_spawn_time = Some(now);

                let socket = self.socket_name.clone();
                let spawned = crate::spawn_shell(&command, &socket);

                crate::life(format!(
                    "keybind: spawn `{command}` result={spawned}"
                ));
            }

            NovaInputAction::Welcome => {
                crate::show_welcome(self);
            }

            NovaInputAction::CloseFocused => {
                let ready = self
                    .last_close_time
                    .map(|last| last.elapsed() >= debounce)
                    .unwrap_or(true);

                if ready {
                    self.last_close_time = Some(now);
                    self.start_close_focused();
                }
            }

            NovaInputAction::Focus(direction) => {
                self.focus_direction(&direction);
            }

            NovaInputAction::MoveFocus(backwards) => {
                self.move_focus_dir(backwards);
            }

            NovaInputAction::CycleFocus(backwards) => {
                self.alt_tab_active = true;

                self.alt_tab_index = self.cycle_focus_index(backwards);
            }

            NovaInputAction::ToggleScratchpad => {
                tracing::debug!(
                    "keybind: toggle_scratchpad fired (visible={} id_set={})",
                    self.scratchpad_visible,
                    self.scratchpad_id.is_some(),
                );
                self.toggle_scratchpad();
            }

            NovaInputAction::Fullscreen => {
                self.toggle_fullscreen();
            }

            NovaInputAction::ToggleFloat => {
                self.toggle_float();
            }

            NovaInputAction::CanvasZoomIn => {
                if self.config_watcher.config.layout == "canvas" {
                    self.canvas_zoom_by(1.2);
                }
            }

            NovaInputAction::CanvasZoomOut => {
                if self.config_watcher.config.layout == "canvas" {
                    self.canvas_zoom_by(1.0 / 1.2);
                }
            }

            NovaInputAction::CanvasZoomReset => {
                if self.config_watcher.config.layout == "canvas" {
                    if let Some(output) = self.output_under(Some(self.cursor_position)) {
                        self.canvas_reset_view(&output);
                    }
                }
            }

            NovaInputAction::MoveRow(backwards) => {
                self.move_row(backwards);
            }

            NovaInputAction::ConsumeIntoColumn => {
                self.consume_into_column();
            }

            NovaInputAction::ExpelFromColumn => {
                self.expel_from_column();
            }

            NovaInputAction::ToggleOverview => {
                self.toggle_overview();
            }

            NovaInputAction::Workspace(n) => {
                let target = n.saturating_sub(1);
                if self.overview_active {
                    self.exit_overview(Some(target));
                } else {
                    self.goto_workspace(target);
                }
            }

            NovaInputAction::CanvasCenterWindow => {
                self.canvas_center_focused();
            }

            NovaInputAction::CanvasZoomToFit => {
                self.canvas_zoom_to_fit();
            }

            NovaInputAction::CanvasNudge(dx, dy) => {
                self.canvas_nudge_focused(dx, dy);
            }

            NovaInputAction::CanvasPan(dx, dy) => {
                if self.config_watcher.config.layout == "canvas" {
                    self.canvas_pan_by(dx, dy);
                }
            }

            NovaInputAction::TogglePin => {
                self.toggle_pin_focused();
            }

            NovaInputAction::CanvasHome => {
                self.canvas_home();
            }

            NovaInputAction::OverviewNav(dx, dy) => {
                self.overview_nav(dx, dy);
            }

            NovaInputAction::OverviewConfirm => {
                self.exit_overview(None);
            }

            NovaInputAction::OverviewCancel => {
                self.exit_overview_to_entry();
            }

            NovaInputAction::OverviewIgnore => {}

            NovaInputAction::ScreenshotConfirm => {
                self.screenshot_confirm();
            }

            NovaInputAction::ScreenshotCancel => {
                self.screenshot_cancel();
            }

            NovaInputAction::RecordToggle => {
                self.record_toggle();
            }

            NovaInputAction::RecordDigit(digit) => {
                self.record_digit(digit);
            }

            NovaInputAction::RecordMove(direction) => {
                self.record_move(direction);
            }

            NovaInputAction::RecordEnter => {
                self.record_enter();
            }

            NovaInputAction::RecordCancel => {
                self.record_cancel();
            }
        }
    }

    fn nova_dispatch_pointer_motion(&mut self, motion: MotionEvent) {
        if self.screenshot_motion() {
            return;
        }

        if self.record_motion() {
            return;
        }

        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };

        let position = motion.location;
        let serial = motion.serial;

        if !self.is_session_locked() {
        if let Some((name, last, pan_button)) = self.canvas_pan.clone() {
            let dx = position.x - last.x;
            let dy = position.y - last.y;
            if dx != 0.0 || dy != 0.0 {
                self.canvas_pan_by(dx, dy);
                self.canvas_pan = Some((name, position, pan_button));
            }
        }
        }

        let exclusive_layer = self.exclusive_keyboard_layer().is_some();
        let over_layer = self.upper_layer_at(position);
        if self.config_watcher.config.focus_follows_mouse
            && !self.is_session_locked()
            && !pointer.is_grabbed()
            && self.canvas_pan.is_none()
            && !exclusive_layer
            && !over_layer
        {
            let window = self
                .space
                .element_under(self.canvas_pick_pos(position))
                .filter(|(window, _)| {
                    !self.is_hidden_scratchpad(window)
                        && self.window_owned_at(window, position)
                })
                .map(|(window, _)| window.clone());

            if let Some(window) = window {
                if let Some(keyboard) = self.seat.get_keyboard() {
                    let focus = window
                        .toplevel()
                        .map(|toplevel| toplevel.wl_surface().clone());

                    if keyboard.current_focus().as_ref() != focus.as_ref() {
                        keyboard.set_focus(self, focus, serial);
                    }
                }
            }
        }

        let under = self.surface_under(position);

        pointer.motion(self, under, &motion);
        pointer.frame(self);
    }

    fn nova_clamp_pointer_to_outputs(&self, requested: Point<f64, Logical>) -> Point<f64, Logical> {
        let eps = 0.01;
        let current_output = self.space.outputs().find(|output| {
            self.space
                .output_geometry(output)
                .map(|geometry| {
                    let g = geometry.to_f64();
                    self.cursor_position.x >= g.loc.x - eps
                        && self.cursor_position.x < g.loc.x + g.size.w + eps
                        && self.cursor_position.y >= g.loc.y - eps
                        && self.cursor_position.y < g.loc.y + g.size.h + eps
                })
                .unwrap_or(false)
        });

        if let Some(output) = current_output {
            if let Some(geometry) = self.space.output_geometry(output) {
                let left = geometry.loc.x as f64;
                let top = geometry.loc.y as f64;
                let right = left + geometry.size.w as f64 - 0.001;
                let bottom = top + geometry.size.h as f64 - 0.001;

                if requested.x >= left
                    && requested.x <= right
                    && requested.y >= top
                    && requested.y <= bottom
                {
                    return requested;
                }
            }
        }

        for output in self.space.outputs() {
            let Some(geometry) = self.space.output_geometry(output) else {
                continue;
            };

            if geometry.to_f64().contains(requested) {
                return requested;
            }
        }

        let mut nearest: Option<(f64, Point<f64, Logical>)> = None;

        for output in self.space.outputs() {
            let Some(geometry) = self.space.output_geometry(output) else {
                continue;
            };

            if geometry.size.w <= 0 || geometry.size.h <= 0 {
                continue;
            }

            let left = geometry.loc.x as f64;
            let top = geometry.loc.y as f64;
            let right = left + geometry.size.w as f64 - 0.001;
            let bottom = top + geometry.size.h as f64 - 0.001;

            let clamped: Point<f64, Logical> = (
                requested.x.clamp(left, right),
                requested.y.clamp(top, bottom),
            )
                .into();

            let dx = requested.x - clamped.x;
            let dy = requested.y - clamped.y;
            let distance = dx * dx + dy * dy;

            if nearest
                .as_ref()
                .map(|(best, _)| distance < *best)
                .unwrap_or(true)
            {
                nearest = Some((distance, clamped));
            }
        }

        nearest.map(|(_, position)| position).unwrap_or(requested)
    }
}

/// Compute the (shift, ctrl, alt, logo) modifier bits requested by a binding's
/// modifier tokens. `"mod"` expands to the top-level `modifier` config value.
fn bind_modifier_bits(modifiers: &[String], configured: &str) -> (bool, bool, bool, bool) {
    let mut bits = (false, false, false, false);

    for token in modifiers {
        match token.as_str() {
            "shift" => bits.0 = true,
            "ctrl" | "control" => bits.1 = true,
            "alt" => bits.2 = true,
            "super" | "logo" | "win" | "meta" => bits.3 = true,
            "mod" | "MOD" => {
                for part in configured.split('+') {
                    match part.trim() {
                        "shift" => bits.0 = true,
                        "ctrl" | "control" => bits.1 = true,
                        "alt" => bits.2 = true,
                        "super" | "logo" | "win" | "meta" => bits.3 = true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    bits
}

/// Canvas-only actions only make sense while the canvas layout is active.
fn is_canvas_action(action: &str) -> bool {
    action == "toggle_pin" || action.starts_with("canvas_") || action.starts_with("canvas ")
}

/// Convert a config action string (plus args) into a keybind action.
fn parse_keybind_action(action: &str, args: &[String]) -> Option<NovaInputAction> {
    match action {
        "spawn_terminal" => Some(NovaInputAction::SpawnTerminal),
        "close_focused" => Some(NovaInputAction::CloseFocused),
        "focus" => args
            .first()
            .map(|direction| NovaInputAction::Focus(direction.clone())),
        "move_list" => Some(NovaInputAction::MoveFocus(
            args.first().is_some_and(|a| a == "previous"),
        )),
        "cycle_focus" => Some(NovaInputAction::CycleFocus(
            args.first().is_some_and(|a| a == "previous"),
        )),
        "move_row" => Some(NovaInputAction::MoveRow(
            args.first().is_some_and(|a| a == "previous"),
        )),
        "consume_into_column" => Some(NovaInputAction::ConsumeIntoColumn),
        "expel_from_column" => Some(NovaInputAction::ExpelFromColumn),
        "toggle_overview" => Some(NovaInputAction::ToggleOverview),
        "toggle_record" => Some(NovaInputAction::RecordToggle),
        "workspace" => args
            .first()
            .and_then(|n| n.parse::<usize>().ok())
            .map(NovaInputAction::Workspace),
        "canvas_center_window" => Some(NovaInputAction::CanvasCenterWindow),
        "canvas_zoom_to_fit" => Some(NovaInputAction::CanvasZoomToFit),
        "canvas_nudge" => args.first().map(|direction| {
            let (dx, dy) = match direction.as_str() {
                "left" => (-50, 0),
                "right" => (50, 0),
                "up" => (0, -50),
                _ => (0, 50),
            };
            NovaInputAction::CanvasNudge(dx, dy)
        }),
        "canvas_pan" => args.first().map(|direction| {
            let (dx, dy) = match direction.as_str() {
                "left" => (-120.0, 0.0),
                "right" => (120.0, 0.0),
                "up" => (0.0, -120.0),
                _ => (0.0, 120.0),
            };
            NovaInputAction::CanvasPan(dx, dy)
        }),
        "toggle_pin" => Some(NovaInputAction::TogglePin),
        "canvas_home" => Some(NovaInputAction::CanvasHome),
        "toggle_scratchpad" => Some(NovaInputAction::ToggleScratchpad),
        "fullscreen" => Some(NovaInputAction::Fullscreen),
        "toggle_float" => Some(NovaInputAction::ToggleFloat),
        "canvas_zoom_in" => Some(NovaInputAction::CanvasZoomIn),
        "canvas_zoom_out" => Some(NovaInputAction::CanvasZoomOut),
        "canvas_zoom_reset" => Some(NovaInputAction::CanvasZoomReset),
        "spawn" => {
            let command = args.join(" ");
            if command.trim().is_empty() {
                None
            } else {
                Some(NovaInputAction::Spawn(command))
            }
        }
        "welcome" => Some(NovaInputAction::Welcome),
        "move_window" | "resize_window" => None,
        _ => None,
    }
}
