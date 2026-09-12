use std::time::Duration;

use smithay::{
    backend::{
        renderer::{
            Color32F, Frame as _, ImportMem, Renderer, Texture,
            element::{self, Element, Id, Kind, RenderElement},
            gles::GlesRenderer,
            utils::CommitCounter,
        },
        winit::{self, WinitEvent},
    },
    desktop::{layer_map_for_output, space::space_render_elements},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::Smallvil;
use smithay::reexports::wayland_server::Resource;

pub fn init_winit(
    event_loop: &mut EventLoop<Smallvil>,
    state: &mut Smallvil,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut backend, winit) = winit::init::<GlesRenderer>()?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<Smallvil>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));

    let bg_path = state.config_watcher.config.background.clone();
    let wallpaper_rgba = if std::path::Path::new(&bg_path).exists() {
        match image::open(&bg_path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                crate::border::note_wallpaper_colors(&rgba);
                Some(rgba)
            }
            Err(e) => {
                tracing::warn!("Failed to open {}: {:?}", bg_path, e);
                None
            }
        }
    } else {
        tracing::warn!("Background {} not found", bg_path);
        None
    };
    let mut wallpaper_rgba = wallpaper_rgba;
    let mut wallpaper_texture: Option<smithay::backend::renderer::gles::GlesTexture> = None;

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| {
            match event {
                WinitEvent::Resized { size, .. } => {
                    output.change_current_state(
                        Some(Mode {
                            size,
                            refresh: 60_000,
                        }),
                        None,
                        None,
                        None,
                    );
                    layer_map_for_output(&output).arrange();
                }
                WinitEvent::Input(event) => state.process_input_event(event),
                WinitEvent::Redraw => {
                    state.update_animations();

                    let size = backend.window_size();
                    let damage = Rectangle::from_size(size);

                    {
                        let (renderer, mut framebuffer) = backend.bind().unwrap();

                        if wallpaper_texture.is_none() {
                            if let Some(rgba) = wallpaper_rgba.take() {
                                let format = smithay::backend::allocator::Fourcc::Abgr8888;
                                let tex_size =
                                    Size::from((rgba.width() as i32, rgba.height() as i32));
                                match renderer.import_memory(&rgba, format, tex_size, false) {
                                    Ok(tex) => wallpaper_texture = Some(tex),
                                    Err(e) => {
                                        tracing::warn!("Failed to import wallpaper texture: {:?}", e)
                                    }
                                }
                            }
                        }

                        let output_scale = output.current_scale().fractional_scale();
                        let scale = Scale::from(output_scale);
                        let space_elements =
                            space_render_elements(renderer, [&state.space], &output, 1.0).unwrap();

                        let canvas_cam = (state.config_watcher.config.layout == "canvas")
                            .then(|| state.canvas_camera_for(&output.name()));
                        let output_origin = state
                            .space
                            .output_geometry(&output)
                            .map(|g| g.loc)
                            .unwrap_or_default();

                        let mut frame = renderer
                            .render(&mut framebuffer, size, Transform::Flipped180)
                            .unwrap();

                        if let Some(ref tex) = wallpaper_texture {
                            let tex_size = tex.size();
                            let src =
                                smithay::utils::Rectangle::<f64, smithay::utils::Buffer>::from_size(
                                    tex_size.to_f64(),
                                );
                            let dst = Rectangle::<i32, Physical>::from_size(size);
                            frame.clear([0.0, 0.0, 0.0, 0.0].into(), &[dst]).unwrap();
                            let _ = frame.render_texture_from_to(
                                tex,
                                src,
                                dst,
                                &[dst],
                                &[],
                                Transform::Normal,
                                1.0,
                                None,
                                &[],
                            );
                        } else {
                            frame.clear([0.1, 0.1, 0.1, 1.0].into(), &[damage]).unwrap();
                        }

                        let focus_surface =
                            state.seat.get_keyboard().and_then(|k| k.current_focus());

                        let border_width = state.config_watcher.config.border_width;
                        let rounding = state.config_watcher.config.rounding;
                        let active_arr = state.config_watcher.config.active_border_color;
                        let inactive_color: Color32F =
                            state.config_watcher.config.inactive_border_color.into();
                        let effect = state.config_watcher.config.border_effect.clone();
                        let effect_t = state.start_time.elapsed().as_secs_f32()
                            * state
                                .config_watcher
                                .config
                                .border_effect_speed
                                .max(0.0);
                        let gradient = state.config_watcher.config.border_gradient_color;
                        if border_width > 0 {
                            for window in state.space.elements().cloned().collect::<Vec<_>>() {
                                let geo = state.space.element_geometry(&window);
                                let loc = state.space.element_location(&window);
                                if let (Some(geo), Some(loc)) = (geo, loc) {
                                    let focused = focus_surface.as_ref()
                                        == Some(window.toplevel().unwrap().wl_surface());
                                    let rect = Rectangle::new(loc, geo.size);
                                    let (rect, bw, rd) = match canvas_cam {
                                        Some(cam) => (
                                            cam.to_screen(output_origin, rect),
                                            ((border_width as f64 * cam.zoom).round() as i32)
                                                .max(1),
                                            ((rounding as f64 * cam.zoom).round() as i32).max(0),
                                        ),
                                        None => (rect, border_width, rounding),
                                    };
                                    draw_border_rect(
                                        &mut frame,
                                        &damage,
                                        rect,
                                        scale,
                                        bw,
                                        rd,
                                        active_arr,
                                        inactive_color,
                                        focused,
                                        &effect,
                                        effect_t,
                                        gradient,
                                    );
                                }
                            }
                        }

                        for elem in &space_elements {
                            let elem_geometry = elem.geometry(scale);
                            let elem_geometry = match canvas_cam {
                                Some(cam) => cam.scale_physical(output_scale, elem_geometry),
                                None => elem_geometry,
                            };
                            let elem_src = elem.src();

                            if !damage.overlaps(elem_geometry) {
                                continue;
                            }

                            let elem_damage: Vec<Rectangle<i32, Physical>> = damage
                                .intersection(elem_geometry)
                                .into_iter()
                                .map(|mut d| {
                                    d.loc -= elem_geometry.loc;
                                    d
                                })
                                .collect();

                            if elem_damage.is_empty() {
                                continue;
                            }

                            let _ = elem.draw(
                                &mut frame,
                                elem_src,
                                elem_geometry,
                                &elem_damage,
                                &[],
                                None,
                            );
                        }

                        let focus_opacity = state.config_watcher.config.focus_opacity;
                        let unfocused_opacity = state.config_watcher.config.opacity;
                        let any_rule_opacity = !state.window_opacity.is_empty();
                        if unfocused_opacity < 1.0 || focus_opacity < 1.0 || any_rule_opacity {
                            for window in state.space.elements().cloned().collect::<Vec<_>>() {
                                let geo = state.space.element_geometry(&window);
                                if let Some(geo) = geo {
                                    let focused = focus_surface.as_ref()
                                        == Some(window.toplevel().unwrap().wl_surface());
                                    let id = window.toplevel().unwrap().wl_surface().id();
                                    let opacity = state
                                        .window_opacity
                                        .get(&id)
                                        .copied()
                                        .unwrap_or(if focused {
                                            focus_opacity
                                        } else {
                                            unfocused_opacity
                                        });
                                    let rect = Rectangle::new(
                                        state.space.element_location(&window).unwrap_or_default(),
                                        geo.size,
                                    );
                                    let rect = match canvas_cam {
                                        Some(cam) => cam.to_screen(output_origin, rect),
                                        None => rect,
                                    };
                                    if opacity < 1.0 && opacity > 0.0 {
                                        let dim_alpha = 1.0 - opacity;
                                        draw_dim_overlay(
                                            &mut frame, &damage, rect, scale, dim_alpha,
                                        );
                                    }
                                }
                            }
                        }

                        if state.alt_tab_active {
                            draw_alt_tab_overlay(&mut frame, &damage, &state, scale);
                        }
                    }
                    backend.submit(Some(&[damage])).unwrap();

                    state.space.elements().for_each(|window| {
                        window.send_frame(
                            &output,
                            state.start_time.elapsed(),
                            Some(Duration::ZERO),
                            |_, _| Some(output.clone()),
                        )
                    });

                    for output in state.space.outputs().cloned().collect::<Vec<_>>() {
                        for layer in layer_map_for_output(&output).layers() {
                            layer.send_frame(
                                &output,
                                state.start_time.elapsed(),
                                Some(Duration::ZERO),
                                |_, _| Some(output.clone()),
                            );
                        }
                    }

                    state.space.refresh();
                    state.popups.cleanup();
                    for output in state.space.outputs().cloned().collect::<Vec<_>>() {
                        layer_map_for_output(&output).cleanup();
                    }
                    let _ = state.display_handle.flush_clients();

                    backend.window().request_redraw();
                }
                WinitEvent::CloseRequested => {
                    state.loop_signal.stop();
                }
                _ => (),
            };
        })?;

    Ok(())
}

/// Draw a window border by rendering the rounded border ring (outer rounded
/// rect minus the window rectangle) slightly larger than the window geometry.
fn draw_border_rect(
    frame: &mut smithay::backend::renderer::gles::GlesFrame,
    damage: &Rectangle<i32, Physical>,
    window_rect: Rectangle<i32, Logical>,
    scale: Scale<f64>,
    border_width: i32,
    rounding: i32,
    active: [f32; 4],
    inactive: Color32F,
    focused: bool,
    effect: &str,
    effect_t: f32,
    gradient: [f32; 4],
) {
    use element::solid::SolidColorRenderElement as SCRE;

    let bw = border_width.max(1);

    let rects = crate::border::border_ring_rects(window_rect, bw, rounding);

    let center = (
        window_rect.loc.x as f32 + window_rect.size.w as f32 / 2.0,
        window_rect.loc.y as f32 + window_rect.size.h as f32 / 2.0,
    );
    let animated = focused && crate::border::is_animated_border_effect(effect);

    let src_size = Size::from((bw as f64 * 10.0, bw as f64 * 10.0));
    let src = Rectangle::from_size(src_size.to_f64());

    for rect in rects {
        let color: Color32F = if animated {
            let angle = crate::border::ring_angle(
                (
                    rect.loc.x as f32 + rect.size.w as f32 / 2.0,
                    rect.loc.y as f32 + rect.size.h as f32 / 2.0,
                ),
                center,
            );
            crate::border::border_effect_color(active, effect, effect_t, angle, gradient).into()
        } else if focused {
            active.into()
        } else {
            inactive
        };
        let rect = rect.to_physical_precise_round(scale);

        if !damage.overlaps(rect) {
            continue;
        }
        let local_damage: Vec<Rectangle<i32, Physical>> = damage
            .intersection(rect)
            .into_iter()
            .map(|mut d| {
                d.loc -= rect.loc;
                d
            })
            .collect();
        if local_damage.is_empty() {
            continue;
        }
        let elem = SCRE::new(
            Id::new(),
            rect,
            CommitCounter::default(),
            color,
            Kind::Unspecified,
        );
        let _ = <SCRE as RenderElement<GlesRenderer>>::draw(
            &elem,
            frame,
            src,
            rect,
            &local_damage,
            &[],
            None,
        );
    }
}

/// Draw a semi-transparent black overlay over a window to simulate
/// reduced opacity for unfocused windows.
fn draw_dim_overlay(
    frame: &mut smithay::backend::renderer::gles::GlesFrame,
    damage: &Rectangle<i32, Physical>,
    window_rect: Rectangle<i32, Logical>,
    scale: Scale<f64>,
    alpha: f32,
) {
    use element::solid::SolidColorRenderElement as SCRE;

    let rect = window_rect.to_f64().to_physical_precise_round(scale);
    if !damage.overlaps(rect) {
        return;
    }
    let local_damage: Vec<Rectangle<i32, Physical>> = damage
        .intersection(rect)
        .into_iter()
        .map(|mut d| {
            d.loc -= rect.loc;
            d
        })
        .collect();
    if local_damage.is_empty() {
        return;
    }
    let src = Rectangle::from_size(Size::from((10.0, 10.0)));
    let elem = SCRE::new(
        Id::new(),
        rect,
        CommitCounter::default(),
        [0.0f32, 0.0, 0.0, alpha.clamp(0.0, 1.0)],
        Kind::Unspecified,
    );
    let _ = <SCRE as RenderElement<GlesRenderer>>::draw(
        &elem,
        frame,
        src,
        rect,
        &local_damage,
        &[],
        None,
    );
}

/// Draw a simple centered alt-tab overlay box with the focused window index.
fn draw_alt_tab_overlay(
    frame: &mut smithay::backend::renderer::gles::GlesFrame,
    damage: &Rectangle<i32, Physical>,
    state: &Smallvil,
    scale: Scale<f64>,
) {
    use element::solid::SolidColorRenderElement as SCRE;

    let n = state.space.elements().count();
    if n == 0 {
        return;
    }
    let idx = state.alt_tab_index.min(n - 1);

    let box_w = (n as i32 * 120).min(800);
    let box_h = 90;
    let center: Point<i32, Physical> = Point::from((damage.size.w / 2, damage.size.h / 3));
    let box_rect = Rectangle::new(
        Point::from((center.x - box_w / 2, center.y - box_h / 2)),
        Size::from((box_w, box_h)),
    )
    .to_physical_precise_round(scale);

    let live_damage: Vec<Rectangle<i32, Physical>> = damage
        .intersection(box_rect)
        .into_iter()
        .map(|mut d| {
            d.loc -= box_rect.loc;
            d
        })
        .collect();
    if live_damage.is_empty() {
        return;
    }
    let src = Rectangle::from_size(Size::from((10.0, 10.0)));
    let bg = SCRE::new(
        Id::new(),
        box_rect,
        CommitCounter::default(),
        [0.05f32, 0.05, 0.08, 0.7],
        Kind::Unspecified,
    );
    let _ = <SCRE as RenderElement<GlesRenderer>>::draw(
        &bg,
        frame,
        src,
        box_rect,
        &live_damage,
        &[],
        None,
    );

    let slot_w = 100;
    let slot_h = 60;
    let slot_x = box_rect.loc.x + 10 + idx as i32 * 110;
    let slot_rect = Rectangle::new(
        Point::from((slot_x, box_rect.loc.y + (box_h - slot_h) / 2)),
        Size::from((slot_w, slot_h)),
    )
    .to_physical_precise_round(scale);
    let slot_damage: Vec<Rectangle<i32, Physical>> = live_damage
        .iter()
        .cloned()
        .filter(|d| slot_rect.overlaps(*d))
        .collect();
    let sel = SCRE::new(
        Id::new(),
        slot_rect,
        CommitCounter::default(),
        [0.0f32, 0.5, 1.0, 0.9],
        Kind::Unspecified,
    );
    let _ = <SCRE as RenderElement<GlesRenderer>>::draw(
        &sel,
        frame,
        src,
        slot_rect,
        &slot_damage,
        &[],
        None,
    );
}
