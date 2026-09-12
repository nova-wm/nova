//! DRM backend with startup discovery of all available GPUs and outputs.
//!

use std::{
    collections::HashMap,
    io,
    ops::Deref,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        SwapBuffersError,
        allocator::{
            Fourcc,
            format::FormatSet,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            CreateDrmNodeError, DrmAccessError, DrmDevice, DrmDeviceFd, DrmError, DrmEvent,
            DrmEventMetadata, DrmNode, NodeType,
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{self, EGLContext, EGLDevice, EGLDisplay, context::ContextPriority},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            Bind as _,
            Color32F,
            ExportMem as _,
            Frame as _,
            Renderer as _,
            element::{
                Element as _,
                Kind,
                Wrap,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::{GlesRenderer, GlesTexProgram, GlesTexture, UniformName, UniformType},
            multigpu::{GpuManager, MultiRenderer, gbm::GbmGlesBackend},
            utils::draw_render_elements,
        },
        session::{
            Event as SessionEvent, Session,
            libseat::{self, LibSeatSession},
        },
        udev::{UdevBackend, all_gpus, primary_gpu},
    },
    desktop::{
        layer_map_for_output,
        space::{SpaceRenderElements, space_render_elements},
    },
    output::{Mode, Output, PhysicalProperties},
    reexports::{
        calloop::{
            EventLoop, LoopHandle, RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{ModeTypeFlags, connector, crtc},
        input::{DeviceCapability, Libinput},
        rustix::fs::OFlags,
        wayland_server::{Display, backend::GlobalId},
    },
    utils::{Buffer, Clock, DeviceFd, Logical, Monotonic, Physical, Point, Rectangle, Scale, Time},
    wayland::{
        selection::data_device::set_data_device_selection,
        shell::wlr_layer::Layer as WlrLayer,
    },
};

use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

use crate::Smallvil;
use smithay::reexports::wayland_server::Resource;

#[path = "drm_cursor.rs"]
mod drm_cursor;

use drm_cursor::{CursorSprite, NovaDrmElement, RoundedBorderElement, RoundedSurfaceElement};

const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
];

static FRAME_EPOCH: OnceLock<Instant> = OnceLock::new();

type UdevRenderer<'a> = MultiRenderer<
    'a,
    'a,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

type SurfaceRenderElement<'a> =
    SpaceRenderElements<UdevRenderer<'a>, WaylandSurfaceRenderElement<UdevRenderer<'a>>>;

#[derive(Debug, PartialEq, Clone)]
pub struct UdevOutputId {
    pub device_id: DrmNode,
    pub crtc: crtc::Handle,
}

pub struct DrmSurface {
    pub output: Output,

    pub drm_output: DrmOutput<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<()>,
        DrmDeviceFd,
    >,

    pub global: GlobalId,
    pub device_id: DrmNode,
    pub render_node: Option<DrmNode>,
}

pub struct DrmDeviceData {
    pub surfaces: HashMap<crtc::Handle, DrmSurface>,

    pub drm_output_manager: DrmOutputManager<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<()>,
        DrmDeviceFd,
    >,

    pub drm_scanner: DrmScanner,
    pub render_node: Option<DrmNode>,
    pub registration_token: RegistrationToken,
}

pub struct DrmState {
    pub session: LibSeatSession,
    pub handle: LoopHandle<'static, Smallvil>,
    pub clock: Clock<Monotonic>,
    pub primary_gpu: DrmNode,

    pub gpus: GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,

    pub backends: HashMap<DrmNode, DrmDeviceData>,
    pub keyboards: Vec<smithay::reexports::input::Device>,
    pub pointer_devices: Vec<smithay::reexports::input::Device>,
    pub paused: bool,
    pub cursor_sprite: CursorSprite,
    /// Compiled rounded-corner fragment shader, cached per render device.
    /// A GLES program is only valid on the GL context that created it, and
    pub rounded_program: HashMap<DrmNode, GlesTexProgram>,
    /// Compiled rounded-border annulus fragment shader, cached per render
    /// device (same per-GPU-context constraint as `rounded_program`).
    pub border_program: HashMap<DrmNode, GlesTexProgram>,
    /// A cached 1x1 white texture used as a dummy sampler for the border
    /// shader, per render device.
    pub stub_texture: HashMap<DrmNode, smithay::backend::renderer::gles::GlesTexture>,
    /// Monotonically increasing counter used as CommitCounter for cursor and
    /// border elements so damage tracking sees each frame as changed.
    pub frame_counter: usize,
    /// Decoded wallpaper pixels (from `background` in the config), loaded once.
    /// Blurring happens at load time; the sharp/blurred choice is read from the
    pub wallpaper_source: Option<image::RgbaImage>,
    pub wallpaper_blurred: Option<image::RgbaImage>,
    pub wallpaper_tried: bool,
    /// Imported wallpaper GL textures per render device:
    /// `[0]` = sharp, `[1]` = CPU-blurred.
    pub wallpaper_textures:
        HashMap<DrmNode, [Option<smithay::backend::renderer::gles::GlesTexture>; 2]>,
    /// Pre-generated animated film-grain frames (gray-noise textures) per
    /// render device and output size. The drawn frame cycles in
    pub grain_frames: HashMap<
        (DrmNode, (u32, u32)),
        Vec<smithay::backend::renderer::gles::GlesTexture>,
    >,
    pub cursor_theme_key: Option<(String, i32)>,
    pub cursor_theme_pixels: Option<drm_cursor::CursorThemeImage>,
    pub cursor_theme_textures: HashMap<DrmNode, GlesTexture>,
    pub cursor_theme_id: smithay::backend::renderer::element::Id,
    /// Offscreen capture textures per render device and output size:
    /// `novactl screenshot` re-composites the output's elements into these,
    pub screenshot_textures: HashMap<(DrmNode, (u32, u32)), GlesTexture>,
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceAddError {
    #[error("Failed to open device using libseat: {0}")]
    DeviceOpen(libseat::Error),

    #[error("Failed to initialize DRM device: {0}")]
    DrmDevice(DrmError),

    #[error("Failed to initialize GBM device: {0}")]
    GbmDevice(std::io::Error),

    #[error("Failed to access DRM node: {0}")]
    DrmNode(CreateDrmNodeError),

    #[error("Failed to initialize GPU rendering: {0}")]
    AddNode(egl::Error),

    #[error("No usable hardware renderer")]
    NoRenderNode,

    #[error("Primary GPU is missing")]
    PrimaryGpuMissing,
}

fn output_frame_duration(output: &Output) -> Duration {
    output
        .current_mode()
        .filter(|mode| mode.refresh > 0)
        .map(|mode| Duration::from_secs_f64(1_000.0 / mode.refresh as f64))
        .unwrap_or(Duration::from_millis(16))
}

/// Generate (once) and import the animated film-grain frames for the given
/// render device and output size. Each frame is a full-resolution gray-noise
fn parse_accel_profile(name: &str) -> Option<smithay::reexports::input::AccelProfile> {
    use smithay::reexports::input::AccelProfile;
    match name.trim().to_ascii_lowercase().as_str() {
        "flat" => Some(AccelProfile::Flat),
        "adaptive" => Some(AccelProfile::Adaptive),
        _ => None,
    }
}

/// Parse a click-method name from the config (`button-areas` /
/// `clickfinger`). Unknown names leave the device default untouched.
fn parse_click_method(name: &str) -> Option<smithay::reexports::input::ClickMethod> {
    use smithay::reexports::input::ClickMethod;
    match name.trim().to_ascii_lowercase().as_str() {
        "button-areas" | "button_areas" | "buttonareas" => Some(ClickMethod::ButtonAreas),
        "clickfinger" | "click-finger" | "click_finger" => Some(ClickMethod::Clickfinger),
        _ => None,
    }
}

/// Apply the `input.touchpad` / `input.mouse` config blocks to a libinput
/// pointer device. Touchpads are detected by tap finger count; every setter
pub(crate) fn apply_libinput_config(
    device: &mut smithay::reexports::input::Device,
    config: &crate::config::Config,
) {
    let name = format!("{:?}", device.name());
    if device.config_tap_finger_count() > 0 {
        let _ = device.config_tap_set_enabled(config.touchpad_tap);
        if device.config_scroll_has_natural_scroll() {
            let _ = device.config_scroll_set_natural_scroll_enabled(config.touchpad_natural_scroll);
        }
        if device.config_accel_is_available() {
            if let Some(profile) = parse_accel_profile(&config.touchpad_accel_profile) {
                if device.config_accel_profiles().contains(&profile) {
                    let _ = device.config_accel_set_profile(profile);
                }
            }
            let _ = device.config_accel_set_speed(config.touchpad_accel_speed.clamp(-1.0, 1.0));
        }
        if let Some(method) = parse_click_method(&config.touchpad_click_method) {
            if device.config_click_methods().contains(&method) {
                let _ = device.config_click_set_method(method);
            }
        }
        let _ = device.config_dwt_set_enabled(config.touchpad_dwt);
        crate::life(format!(
            "libinput: touchpad {name}: tap={} natural={} profile={:?} speed={} click={:?} dwt={}",
            config.touchpad_tap,
            config.touchpad_natural_scroll,
            config.touchpad_accel_profile,
            config.touchpad_accel_speed,
            config.touchpad_click_method,
            config.touchpad_dwt,
        ));
    } else {
        if device.config_scroll_has_natural_scroll() {
            let _ = device.config_scroll_set_natural_scroll_enabled(config.mouse_natural_scroll);
        }
        if device.config_accel_is_available() {
            if let Some(profile) = parse_accel_profile(&config.mouse_accel_profile) {
                if device.config_accel_profiles().contains(&profile) {
                    let _ = device.config_accel_set_profile(profile);
                }
            }
            let _ = device.config_accel_set_speed(config.mouse_accel_speed.clamp(-1.0, 1.0));
        }
        crate::life(format!(
            "libinput: pointer {name}: natural={} profile={:?} speed={}",
            config.mouse_natural_scroll,
            config.mouse_accel_profile,
            config.mouse_accel_speed,
        ));
    }
}

/// Import the cursor-theme image for a render node (once per node;
/// mirrors the wallpaper cache). Returns the texture plus (width, height,
fn ensure_cursor_texture(
    textures: &mut HashMap<DrmNode, GlesTexture>,
    pixels: Option<&drm_cursor::CursorThemeImage>,
    render_node: &DrmNode,
    gles: &mut GlesRenderer,
) -> Option<(GlesTexture, (u32, u32, u32, u32))> {
    let pixels = pixels?;
    let (w, h) = pixels.rgba.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    if let Some(texture) = textures.get(render_node).cloned() {
        return Some((texture, (w, h, pixels.xhot, pixels.yhot)));
    }
    let texture = smithay::backend::renderer::ImportMem::import_memory(
        gles,
        pixels.rgba.as_raw(),
        Fourcc::Abgr8888,
        smithay::utils::Size::from((w as i32, h as i32)),
        false,
    )
    .ok()?;
    textures.insert(render_node.clone(), texture.clone());
    Some((texture, (w, h, pixels.xhot, pixels.yhot)))
}

fn ensure_grain_frames(
    grain_frames: &mut HashMap<
        (DrmNode, (u32, u32)),
        Vec<smithay::backend::renderer::gles::GlesTexture>,
    >,
    render_node: &DrmNode,
    gles: &mut GlesRenderer,
    size: (u32, u32),
) {
    if grain_frames.contains_key(&(render_node.clone(), size)) {
        return;
    }
    if size.0 == 0 || size.1 == 0 {
        return;
    }
    let generated = generate_grain_frames(size.0, size.1, 4);
    let mut frames = Vec::with_capacity(generated.len());
    for frame in generated {
        let tex_size = smithay::utils::Size::from((size.0 as i32, size.1 as i32));
        match smithay::backend::renderer::ImportMem::import_memory(
            gles,
            frame.as_raw(),
            Fourcc::Abgr8888,
            tex_size,
            false,
        ) {
            Ok(tex) => frames.push(tex),
            Err(error) => {
                crate::life(format!("grain: frame import failed: {error:?}"));
                return;
            }
        }
    }
    grain_frames.insert((render_node.clone(), size), frames);
}

/// Generate `frames` full-resolution gray-noise images (white noise, opaque),
/// used as the animated film-grain overlay. Deterministic xorshift PRNG so no
fn generate_grain_frames(w: u32, h: u32, frames: usize) -> Vec<image::RgbaImage> {
    let mut state: u32 = 0x9E37_79B9u32
        ^ w.wrapping_mul(0xC2B2_AE35)
        ^ h.wrapping_mul(0x27D4_EB2F)
        ^ (frames as u32).wrapping_mul(0x1656_67B1);
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    (0..frames)
        .map(|_| {
            let mut img = image::RgbaImage::new(w, h);
            for px in img.pixels_mut() {
                let v = ((next() >> 24) & 0xff) as u8;
                *px = image::Rgba([v, v, v, 255]);
            }
            img
        })
        .collect()
}

/// Re-composite one output's element list into an offscreen texture and read
/// it back as RGBA bytes (top row first), cropped to `rect` when set.
#[allow(clippy::too_many_arguments)]
fn capture_output_screenshot<'a>(
    renderer: &mut UdevRenderer<'a>,
    screenshot_textures: &mut HashMap<(DrmNode, (u32, u32)), GlesTexture>,
    render_node: DrmNode,
    elements: &[NovaDrmElement<SurfaceRenderElement<'a>>],
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    scale: f64,
    rect: Option<&Rectangle<i32, Physical>>,
) -> Result<(Vec<u8>, u32, u32), String> {
    let size: smithay::utils::Size<i32, Physical> =
        output_geometry.size.to_physical_precise_round(scale);
    if size.w <= 0 || size.h <= 0 {
        return Err("screenshot: output has no size".to_string());
    }
    let (w, h) = (size.w as u32, size.h as u32);
    let key = (render_node, (w, h));

    screenshot_textures.retain(|existing, _| existing.0 != render_node || *existing == key);
    if !screenshot_textures.contains_key(&key) {
        let gles = AsMut::<GlesRenderer>::as_mut(&mut *renderer);
        let zeros = vec![0u8; (w as usize) * (h as usize) * 4];
        let texture = smithay::backend::renderer::ImportMem::import_memory(
            gles,
            zeros.as_slice(),
            Fourcc::Abgr8888,
            smithay::utils::Size::from((size.w, size.h)),
            false,
        )
        .map_err(|error| format!("screenshot: capture texture import failed: {error:?}"))?;
        screenshot_textures.insert(key.clone(), texture);
    }
    let texture = screenshot_textures
        .get_mut(&key)
        .ok_or_else(|| "screenshot: capture texture missing".to_string())?;

    let mut target = renderer
        .bind(&mut *texture)
        .map_err(|error| format!("screenshot: bind failed: {error:?}"))?;
    let mut frame = renderer
        .render(&mut target, size, output.current_transform())
        .map_err(|error| format!("screenshot: render failed: {error:?}"))?;
    let full = Rectangle::<i32, Physical>::new((0, 0).into(), (size.w, size.h).into());
    let clear: Color32F = [0.1, 0.1, 0.1, 1.0].into();
    frame
        .clear(clear, std::slice::from_ref(&full))
        .map_err(|error| format!("screenshot: clear failed: {error:?}"))?;
    let _capture_damage = draw_render_elements::<UdevRenderer<'a>, _, _>(
        &mut frame,
        Scale::from(scale),
        elements,
        std::slice::from_ref(&full),
    )
    .map_err(|error| format!("screenshot: draw failed: {error:?}"))?;
    let _sync_point = frame
        .finish()
        .map_err(|error| format!("screenshot: finish failed: {error:?}"))?;
    drop(target);

    let gles = AsMut::<GlesRenderer>::as_mut(&mut *renderer);
    let region = Rectangle::<i32, Buffer>::new((0, 0).into(), (size.w, size.h).into());
    let mapping = gles
        .copy_texture(&*texture, region, Fourcc::Abgr8888)
        .map_err(|error| format!("screenshot: copy_texture failed: {error:?}"))?;
    let bytes = gles
        .map_texture(&mapping)
        .map_err(|error| format!("screenshot: map_texture failed: {error:?}"))?;
    let expected = (w as usize) * (h as usize) * 4;
    if bytes.len() != expected {
        return Err(format!(
            "screenshot: mapping size {} != {expected}",
            bytes.len(),
        ));
    }
    let rgba = bytes.to_vec();

    if let Some(rect) = rect {
        let x0 = rect.loc.x.clamp(0, size.w);
        let y0 = rect.loc.y.clamp(0, size.h);
        let x1 = (rect.loc.x + rect.size.w).clamp(0, size.w);
        let y1 = (rect.loc.y + rect.size.h).clamp(0, size.h);
        if x1 <= x0 || y1 <= y0 {
            return Err("screenshot: selection is empty".to_string());
        }
        let stride = (w as usize) * 4;
        let (cw, ch) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let mut cropped = Vec::with_capacity((cw as usize) * (ch as usize) * 4);
        for row in y0..y1 {
            let start = (row as usize) * stride + (x0 as usize) * 4;
            cropped.extend_from_slice(&rgba[start..start + (cw as usize) * 4]);
        }
        return Ok((cropped, cw, ch));
    }

    Ok((rgba, w, h))
}

/// Build the area-select overlay (dim bands + white frame) for the current
/// drag, in output-local physical pixels. A full-output dim shows before
fn push_record_picker_overlay<'a>(
    output_name: &str,
    highlight: &Option<String>,
    index: usize,
    output_geometry: Rectangle<i32, Logical>,
    scale: f64,
    frame_cc: smithay::backend::renderer::utils::CommitCounter,
) -> Vec<NovaDrmElement<SurfaceRenderElement<'a>>> {
    use smithay::backend::renderer::element::Id;
    use smithay::backend::renderer::element::solid::SolidColorRenderElement;

    let mut out: Vec<NovaDrmElement<SurfaceRenderElement<'_>>> = Vec::new();
    let mut rect = |x: i32, y: i32, w: i32, h: i32, color: [f32; 4]| {
        if w > 0 && h > 0 {
            out.push(NovaDrmElement::Cursor(SolidColorRenderElement::new(
                Id::new(),
                Rectangle::<i32, Physical>::new((x, y).into(), (w, h).into()),
                frame_cc,
                color,
                Kind::Unspecified,
            )));
        }
    };

    let size: smithay::utils::Size<i32, Physical> =
        output_geometry.size.to_physical_precise_round(scale);
    let (full_w, full_h) = (size.w.max(1), size.h.max(1));
    rect(0, 0, full_w, full_h, [0.0, 0.0, 0.0, 0.45]);
    let bw = if highlight.as_deref() == Some(output_name) {
        6
    } else {
        3
    };
    rect(0, 0, full_w, bw, [1.0, 1.0, 1.0, 1.0]);
    rect(0, full_h - bw, full_w, bw, [1.0, 1.0, 1.0, 1.0]);
    rect(0, 0, bw, full_h, [1.0, 1.0, 1.0, 1.0]);
    rect(full_w - bw, 0, bw, full_h, [1.0, 1.0, 1.0, 1.0]);
    for i in 0..index {
        rect(16 + i as i32 * 16, 16, 10, 10, [1.0, 1.0, 1.0, 1.0]);
    }
    out
}

/// Build the recording indicator for the recorded output: a small red
/// square top-right. Pushed after the capture above, so it never appears
fn push_record_dot<'a>(
    output_geometry: Rectangle<i32, Logical>,
    scale: f64,
    frame_cc: smithay::backend::renderer::utils::CommitCounter,
) -> Vec<NovaDrmElement<SurfaceRenderElement<'a>>> {
    use smithay::backend::renderer::element::Id;
    use smithay::backend::renderer::element::solid::SolidColorRenderElement;

    let size: smithay::utils::Size<i32, Physical> =
        output_geometry.size.to_physical_precise_round(scale);
    let full_w = size.w.max(1);
    let (d, m) = (12, 12);
    let dot = SolidColorRenderElement::new(
        Id::new(),
        Rectangle::<i32, Physical>::new((full_w - m - d, m).into(), (d, d).into()),
        frame_cc,
        [1.0, 0.0, 0.0, 1.0],
        Kind::Unspecified,
    );
    vec![NovaDrmElement::Cursor(dot)]
}

#[allow(clippy::too_many_arguments)]
fn push_screenshot_overlay<'a>(
    output_geometry: Rectangle<i32, Logical>,
    scale: f64,
    frame_cc: smithay::backend::renderer::utils::CommitCounter,
    anchor: &Point<f64, Logical>,
    current: &Point<f64, Logical>,
    dragging: bool,
) -> Vec<NovaDrmElement<SurfaceRenderElement<'a>>> {
    use smithay::backend::renderer::element::Id;
    use smithay::backend::renderer::element::solid::SolidColorRenderElement;

    let mut out: Vec<NovaDrmElement<SurfaceRenderElement<'_>>> = Vec::new();
    let mut dim = |rect: Rectangle<i32, Physical>, color: [f32; 4]| {
        if rect.size.w > 0 && rect.size.h > 0 {
            out.push(NovaDrmElement::Cursor(SolidColorRenderElement::new(
                Id::new(),
                rect,
                frame_cc,
                color,
                Kind::Unspecified,
            )));
        }
    };

    let size: smithay::utils::Size<i32, Physical> =
        output_geometry.size.to_physical_precise_round(scale);
    let (full_w, full_h) = (size.w.max(1), size.h.max(1));
    let local = |point: &Point<f64, Logical>| {
        let x =
            (point.x - output_geometry.loc.x as f64).clamp(0.0, output_geometry.size.w as f64);
        let y =
            (point.y - output_geometry.loc.y as f64).clamp(0.0, output_geometry.size.h as f64);
        ((x * scale).round() as i32, (y * scale).round() as i32)
    };
    let (ax, ay) = local(anchor);
    let (cx, cy) = local(current);
    let (rx0, rx1) = (ax.min(cx), ax.max(cx));
    let (ry0, ry1) = (ay.min(cy), ay.max(cy));
    if !dragging || rx1 - rx0 < 2 || ry1 - ry0 < 2 {
        dim(
            Rectangle::<i32, Physical>::new((0, 0).into(), (full_w, full_h).into()),
            [0.0, 0.0, 0.0, 0.45],
        );
        return out;
    }

    dim(
        Rectangle::<i32, Physical>::new((0, 0).into(), (full_w, ry0).into()),
        [0.0, 0.0, 0.0, 0.55],
    );
    dim(
        Rectangle::<i32, Physical>::new((0, ry1).into(), (full_w, full_h - ry1).into()),
        [0.0, 0.0, 0.0, 0.55],
    );
    dim(
        Rectangle::<i32, Physical>::new((0, ry0).into(), (rx0, ry1 - ry0).into()),
        [0.0, 0.0, 0.0, 0.55],
    );
    dim(
        Rectangle::<i32, Physical>::new((rx1, ry0).into(), (full_w - rx1, ry1 - ry0).into()),
        [0.0, 0.0, 0.0, 0.55],
    );
    let bw = 2;
    dim(
        Rectangle::<i32, Physical>::new((rx0, ry0).into(), (rx1 - rx0, bw).into()),
        [1.0, 1.0, 1.0, 1.0],
    );
    dim(
        Rectangle::<i32, Physical>::new((rx0, ry1 - bw).into(), (rx1 - rx0, bw).into()),
        [1.0, 1.0, 1.0, 1.0],
    );
    dim(
        Rectangle::<i32, Physical>::new((rx0, ry0).into(), (bw, ry1 - ry0).into()),
        [1.0, 1.0, 1.0, 1.0],
    );
    dim(
        Rectangle::<i32, Physical>::new((rx1 - bw, ry0).into(), (bw, ry1 - ry0).into()),
        [1.0, 1.0, 1.0, 1.0],
    );
    out
}

pub fn run_drm() -> Result<(), Box<dyn std::error::Error>> {
    FRAME_EPOCH.get_or_init(Instant::now);

    if std::env::var_os("XDG_CURRENT_DESKTOP").is_none() {
        unsafe {
            std::env::set_var("XDG_CURRENT_DESKTOP", "wlroots");
            std::env::set_var("XDG_SESSION_TYPE", "wayland");
        }
    }

    crate::life("run_drm: creating event loop");

    let mut event_loop: EventLoop<Smallvil> = EventLoop::try_new()?;

    let display: Display<Smallvil> = Display::new()?;

    crate::life("run_drm: initializing libseat");

    let (session, notifier) = LibSeatSession::new()
        .map_err(|error| io::Error::other(format!("failed to initialize libseat: {error}")))?;

    let seat_name = session.seat().to_string();

    crate::life(format!("run_drm: seat={seat_name}"));

    let mut selected_gpu = if let Ok(path) = std::env::var("NOVAWM_DRM_DEVICE") {
        Some(DrmNode::from_path(&path).map_err(|error| {
            io::Error::other(format!(
                "invalid NOVAWM_DRM_DEVICE={path:?}: \
                             {error}"
            ))
        })?)
    } else {
        primary_gpu(seat_name.clone())
            .ok()
            .flatten()
            .and_then(|path| DrmNode::from_path(&path).ok())
    };

    if selected_gpu.is_none() {
        if let Ok(devices) = all_gpus(seat_name.clone()) {
            selected_gpu = devices
                .into_iter()
                .find_map(|path| DrmNode::from_path(&path).ok());
        }
    }

    let selected_gpu = selected_gpu
        .ok_or_else(|| io::Error::other(format!("no GPU found for seat {seat_name}")))?;

    let primary_card = selected_gpu
        .node_with_type(NodeType::Primary)
        .unwrap_or(Ok(selected_gpu))
        .unwrap_or(selected_gpu);

    let primary_render = selected_gpu
        .node_with_type(NodeType::Render)
        .unwrap_or(Ok(selected_gpu))
        .unwrap_or(selected_gpu);

    crate::life(format!(
        "run_drm: primary_card={primary_card:?} \
         primary_render={primary_render:?}"
    ));

    let gpus = GpuManager::new(GbmGlesBackend::with_factory(|display| {
        let context = EGLContext::new_with_priority(display, ContextPriority::High)
            .or_else(|_| EGLContext::new(display))?;

        let capabilities = unsafe { GlesRenderer::supported_capabilities(&context)? };

        Ok(unsafe { GlesRenderer::with_capabilities(context, capabilities)? })
    }))?;

    let drm_state = DrmState {
        session,
        handle: event_loop.handle(),
        clock: Clock::new(),
        primary_gpu: primary_render,
        gpus,
        backends: HashMap::new(),
        keyboards: Vec::new(),
        pointer_devices: Vec::new(),
        paused: false,
        cursor_sprite: CursorSprite::new(),
        rounded_program: HashMap::new(),
        border_program: HashMap::new(),
        stub_texture: HashMap::new(),
        frame_counter: 0,
        wallpaper_source: None,
        wallpaper_blurred: None,
        wallpaper_tried: false,
        wallpaper_textures: HashMap::new(),
        grain_frames: HashMap::new(),
        cursor_theme_key: None,
        cursor_theme_pixels: None,
        cursor_theme_textures: HashMap::new(),
        cursor_theme_id: smithay::backend::renderer::element::Id::new(),
        screenshot_textures: HashMap::new(),
    };

    let mut state = Smallvil::new_with_drm(&mut event_loop, display, drm_state);

    if state.seat.get_keyboard().is_none() {
        crate::life("run_drm: registering seat keyboard");

        let (xkb_rules, xkb_model, xkb_layout, xkb_variant, xkb_options) =
            state.config_watcher.config.xkb_parts();
        state
            .seat
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
            .map_err(|error| {
                io::Error::other(format!("failed to initialize seat keyboard: {error:?}"))
            })?;
    }

    if state.seat.get_pointer().is_none() {
        crate::life("run_drm: registering seat pointer");
        state.seat.add_pointer();
    }

    crate::life(format!(
        "run_drm: seat capabilities keyboard={} pointer={}",
        state.seat.get_keyboard().is_some(),
        state.seat.get_pointer().is_some(),
    ));

    let udev_backend = UdevBackend::new(&seat_name)?;

    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        state.drm().unwrap().session.clone().into(),
    );

    libinput_context
        .udev_assign_seat(&seat_name)
        .map_err(|_| io::Error::other(format!("failed to assign libinput to {seat_name}")))?;

    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    event_loop
        .handle()
        .insert_source(
            libinput_backend,
            move |mut event, _, data: &mut Smallvil| {
                match &mut event {
                    InputEvent::DeviceAdded { device } => {
                        crate::life(format!("libinput: device added: {:?}", device.name(),));

                        if device.has_capability(DeviceCapability::Keyboard) {
                            crate::life(format!("libinput: keyboard added: {:?}", device.name(),));

                            if let Some(led_state) = data
                                .seat
                                .get_keyboard()
                                .map(|keyboard| keyboard.led_state())
                            {
                                device.led_update(led_state.into());
                            }

                            if let Some(drm) = data.drm.as_mut() {
                                if !drm.keyboards.contains(device) {
                                    drm.keyboards.push(device.clone());
                                }
                            }
                        }

                        if device.has_capability(DeviceCapability::Pointer) {
                            crate::life(format!("libinput: pointer added: {:?}", device.name(),));

                            let config = data.config_watcher.config.clone();
                            apply_libinput_config(device, &config);

                            if let Some(drm) = data.drm.as_mut() {
                                if !drm.pointer_devices.contains(device) {
                                    drm.pointer_devices.push(device.clone());
                                }
                            }
                        }
                    }

                    InputEvent::DeviceRemoved { device } => {
                        crate::life(format!("libinput: device removed: {:?}", device.name(),));

                        if let Some(drm) = data.drm.as_mut() {
                            drm.keyboards.retain(|item| item != device);
                            drm.pointer_devices.retain(|item| item != device);
                        }
                    }

                    InputEvent::Keyboard { .. } => {
                        tracing::debug!("DRM: forwarding physical keyboard event");
                    }

                    _ => {}
                }

                data.process_input_event(event);
            },
        )
        .expect("failed to register libinput event source");

    event_loop
        .handle()
        .insert_source(
            notifier,
            move |event, &mut (), data: &mut Smallvil| match event {
                SessionEvent::PauseSession => {
                    crate::life("session: paused");

                    libinput_context.suspend();

                    if let Some(drm) = data.drm.as_mut() {
                        drm.paused = true;

                        for backend in drm.backends.values_mut() {
                            backend.drm_output_manager.pause();
                        }
                    }
                }

                SessionEvent::ActivateSession => {
                    crate::life("session: activated");

                    if let Err(error) = libinput_context.resume() {
                        crate::life(format!(
                            "session: libinput resume failed: \
                                 {error:?}"
                        ));
                    }

                    if let Some(drm) = data.drm.as_mut() {
                        drm.paused = false;
                    }

                    let nodes: Vec<DrmNode> = data
                        .drm()
                        .map(|drm| drm.backends.keys().copied().collect())
                        .unwrap_or_default();

                    for node in nodes {
                        if let Some(drm) = data.drm.as_mut() {
                            if let Some(backend) = drm.backends.get_mut(&node) {
                                if let Err(error) =
                                    backend.drm_output_manager.lock().activate(false)
                                {
                                    crate::life(format!(
                                        "session: activation \
                                             failed for {node:?}: \
                                             {error}"
                                    ));
                                }
                            }
                        }

                        if let Some(drm) = data.drm.as_ref() {
                            let target = drm.clock.now();

                            drm.handle.clone().insert_idle(move |state| {
                                state.render_drm(node, None, target);
                            });
                        }
                    }
                }
            },
        )
        .expect("failed to register session event source");

    let mut devices: Vec<_> = udev_backend
        .device_list()
        .map(|(device_id, path)| (device_id, path.to_path_buf()))
        .collect();

    let primary_device_id = primary_card.dev_id();

    devices.sort_by_key(|(device_id, _)| *device_id != primary_device_id);

    for (device_id, path) in devices {
        match DrmNode::from_dev_id(device_id) {
            Ok(node) => {
                if let Err(error) = state.device_added(node, &path) {
                    crate::life(format!(
                        "run_drm: GPU initialization failed \
                         path={path:?}: {error}"
                    ));
                }
            }

            Err(error) => {
                crate::life(format!("run_drm: cannot identify {path:?}: {error}"));
            }
        }
    }

    drm_cursor::configure_outputs(&mut state)?;

    state.shm_state.update_formats(vec![
        smithay::reexports::wayland_server::protocol::wl_shm::Format::Argb8888,
        smithay::reexports::wayland_server::protocol::wl_shm::Format::Xrgb8888,
        smithay::reexports::wayland_server::protocol::wl_shm::Format::Abgr8888,
        smithay::reexports::wayland_server::protocol::wl_shm::Format::Xbgr8888,
    ]);

    let output_count = state.space.outputs().count();

    crate::life(format!(
        "run_drm: outputs={output_count} WAYLAND_DISPLAY={:?}",
        state.socket_name,
    ));

    if output_count == 0 {
        return Err(
            io::Error::other("no DRM outputs initialized; inspect GPU/connector logs").into(),
        );
    }

    let initial_geometry = state
        .space
        .outputs()
        .filter_map(|output| state.space.output_geometry(output))
        .min_by_key(|geometry| (geometry.loc.x, geometry.loc.y));

    if let Some(geometry) = initial_geometry {
        state.cursor_position = (
            geometry.loc.x as f64 + geometry.size.w as f64 / 2.0,
            geometry.loc.y as f64 + geometry.size.h as f64 / 2.0,
        )
            .into();
    }

    {
        let mut tick = 0u64;

        event_loop
            .handle()
            .insert_source(
                Timer::from_duration(Duration::from_secs(5)),
                move |_, _, data: &mut Smallvil| {
                    tick = tick.wrapping_add(1);

                    let paused = data.drm().map(|drm| drm.paused).unwrap_or(true);

                    let keyboards = data.drm().map(|drm| drm.keyboards.len()).unwrap_or(0);

                    crate::life(format!(
                        "HEARTBEAT tick={tick} paused={paused} \
                         outputs={} keyboards={keyboards}",
                        data.space.outputs().count(),
                    ));

                    TimeoutAction::ToDuration(Duration::from_secs(5))
                },
            )
            .expect("failed to register heartbeat");
    }

    crate::spawn_client(&state);
    crate::start_portal_stack(&state.socket_name);

    event_loop.run(None, &mut state, |state| {
        state.space.refresh();

        if let Err(error) = state.display_handle.flush_clients() {
            tracing::warn!(?error, "failed to flush Wayland clients");
        }
    })?;

    Ok(())
}

impl Smallvil {
    fn drm_mut(&mut self) -> &mut DrmState {
        self.drm.as_mut().expect("DRM state is not initialized")
    }

    fn kms_driver_name(path: &Path) -> Option<String> {
        let name = path.file_name()?.to_str()?;

        let target = std::fs::read_link(format!("/sys/class/drm/{name}/device/driver")).ok()?;

        target.file_name()?.to_str().map(str::to_string)
    }

    pub fn device_added(&mut self, node: DrmNode, path: &Path) -> Result<(), DeviceAddError> {
        crate::life(format!(
            "device_added: path={path:?} node={node:?} driver={:?}",
            Self::kms_driver_name(path),
        ));

        if self.drm_mut().backends.contains_key(&node) {
            return Ok(());
        }

        let fd = self
            .drm_mut()
            .session
            .open(
                path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .map_err(DeviceAddError::DeviceOpen)?;

        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (drm_device, notifier) =
            DrmDevice::new(fd.clone(), true).map_err(DeviceAddError::DrmDevice)?;

        let gbm = GbmDevice::new(fd).map_err(DeviceAddError::GbmDevice)?;

        let render_node = {
            let display = unsafe { EGLDisplay::new(gbm.clone()).map_err(DeviceAddError::AddNode)? };

            let device =
                EGLDevice::device_for_display(&display).map_err(DeviceAddError::AddNode)?;

            if device.is_software() {
                return Err(DeviceAddError::NoRenderNode);
            }

            let render_node = device.try_get_render_node().ok().flatten().unwrap_or(node);

            self.drm_mut()
                .gpus
                .as_mut()
                .add_node(render_node, gbm.clone())
                .map_err(DeviceAddError::AddNode)?;

            render_node
        };

        crate::life(format!(
            "device_added: card={node:?} render={render_node:?}"
        ));

        let allocator = GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );

        let exporter = GbmFramebufferExporter::new(gbm.clone(), Some(render_node).into());

        let render_formats = {
            match self.drm_mut().gpus.single_renderer(&render_node) {
                Ok(mut renderer) => renderer
                    .as_mut()
                    .egl_context()
                    .dmabuf_render_formats()
                    .iter()
                    .copied()
                    .collect::<FormatSet>(),

                Err(error) => {
                    crate::life(format!(
                        "device_added: renderer unavailable \
                         for {render_node:?}: {error:?}"
                    ));

                    return Err(DeviceAddError::NoRenderNode);
                }
            }
        };

        let drm_output_manager = DrmOutputManager::new(
            drm_device,
            allocator,
            exporter,
            Some(gbm),
            SUPPORTED_FORMATS.iter().copied(),
            render_formats,
        );

        let registration_token = self
            .drm_mut()
            .handle
            .clone()
            .insert_source(
                notifier,
                move |event, metadata, data: &mut Smallvil| match event {
                    DrmEvent::VBlank(crtc) => {
                        data.frame_finish(node, crtc, metadata);
                    }

                    DrmEvent::Error(error) => {
                        crate::life(format!(
                            "DRM event error node={node:?}: \
                                 {error:?}"
                        ));
                    }
                },
            )
            .expect("failed to register DRM events");

        self.drm_mut().backends.insert(
            node,
            DrmDeviceData {
                surfaces: HashMap::new(),
                drm_output_manager,
                drm_scanner: DrmScanner::new(),
                render_node: Some(render_node),
                registration_token,
            },
        );

        self.device_changed(node);

        Ok(())
    }

    fn device_changed(&mut self, node: DrmNode) {
        let Some(device) = self
            .drm
            .as_mut()
            .and_then(|drm| drm.backends.get_mut(&node))
        else {
            return;
        };

        let events = match device
            .drm_scanner
            .scan_connectors(device.drm_output_manager.device())
        {
            Ok(events) => events,

            Err(error) => {
                crate::life(format!(
                    "device_changed: scan failed node={node:?}: \
                     {error:?}"
                ));

                return;
            }
        };

        for event in events {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => {
                    self.connector_connected(node, connector, crtc);
                }

                DrmScanEvent::Connected {
                    connector,
                    crtc: None,
                } => {
                    crate::life(format!(
                        "device_changed: {}-{} CONNECTED BUT \
                         NO CRTC ASSIGNED on {node:?}; check \
                         CRTC routing/allocation and hardware limits",
                        connector.interface().as_str(),
                        connector.interface_id(),
                    ));
                }

                DrmScanEvent::Disconnected {
                    connector,
                    crtc: Some(crtc),
                } => {
                    self.connector_disconnected(node, connector, crtc);
                }

                _ => {}
            }
        }
    }

    fn connector_connected(
        &mut self,
        node: DrmNode,
        connector: connector::Info,
        crtc: crtc::Handle,
    ) {
        let name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id(),
        );

        crate::life(format!(
            "connector_connected: {name} node={node:?} crtc={crtc:?}"
        ));

        let modes = connector.modes();

        let Some(drm_mode) = modes
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| modes.first())
            .copied()
        else {
            crate::life(format!("connector_connected: {name} has no modes"));

            return;
        };

        let wl_mode = Mode::from(drm_mode);

        let (physical_width, physical_height) = connector.size().unwrap_or((0, 0));

        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (physical_width as i32, physical_height as i32).into(),
                subpixel: connector.subpixel().into(),
                make: "Unknown".into(),
                model: "Unknown".into(),
                serial_number: "Unknown".into(),
            },
        );

        let x = self
            .space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .map(|geometry| geometry.loc.x + geometry.size.w)
            .max()
            .unwrap_or(0);

        let position: Point<i32, smithay::utils::Logical> = self
            .config_watcher
            .config
            .monitor_configs
            .iter()
            .find(|mc| mc.name == name)
            .map(|mc| (mc.x, mc.y).into())
            .unwrap_or((x, 0).into());

        output.set_preferred(wl_mode);

        output.change_current_state(Some(wl_mode), None, None, Some(position));

        output.user_data().insert_if_missing(|| UdevOutputId {
            device_id: node,
            crtc,
        });

        let Some(drm) = self.drm.as_mut() else {
            return;
        };

        let Some(device) = drm.backends.get_mut(&node) else {
            return;
        };

        if device.surfaces.contains_key(&crtc) {
            return;
        }

        let render_node = device.render_node.unwrap_or(drm.primary_gpu);

        let planes = device.drm_output_manager.device().planes(&crtc).ok();

        let mut renderer = match drm.gpus.single_renderer(&render_node) {
            Ok(renderer) => renderer,

            Err(error) => {
                crate::life(format!(
                    "connector_connected: NO RENDERER \
                         output={name}: {error:?}"
                ));

                return;
            }
        };

        let drm_output = match device
            .drm_output_manager
            .lock()
            .initialize_output::<_, NovaDrmElement<SurfaceRenderElement<'_>>>(
                crtc,
                drm_mode,
                &[connector.handle()],
                &output,
                planes,
                &mut renderer,
                &DrmOutputRenderElements::default(),
            ) {
            Ok(output) => output,

            Err(error) => {
                crate::life(format!(
                    "connector_connected: initialize_output \
                     FAILED output={name} node={node:?}: {error:?}"
                ));

                return;
            }
        };

        let global = output.create_global::<Smallvil>(&self.display_handle);

        self.space.map_output(&output, position);

        crate::life(format!(
            "connector_connected: output {name} mapped \
             x={x} size={:?} refresh={}mHz",
            wl_mode.size, wl_mode.refresh,
        ));

        device.surfaces.insert(
            crtc,
            DrmSurface {
                output,
                drm_output,
                global,
                device_id: node,
                render_node: Some(render_node),
            },
        );

        let target = drm.clock.now();

        drm.handle.clone().insert_idle(move |state: &mut Smallvil| {
            state.render_drm(node, Some(crtc), target);
        });
    }

    fn connector_disconnected(
        &mut self,
        node: DrmNode,
        connector: connector::Info,
        crtc: crtc::Handle,
    ) {
        crate::life(format!(
            "connector_disconnected: {}-{} node={node:?}",
            connector.interface().as_str(),
            connector.interface_id(),
        ));

        let removed = self
            .drm
            .as_mut()
            .and_then(|drm| drm.backends.get_mut(&node))
            .and_then(|backend| backend.surfaces.remove(&crtc));

        if let Some(surface) = removed {
            let layers: Vec<_> = layer_map_for_output(&surface.output)
                .layers()
                .cloned()
                .collect();
            {
                let mut old = layer_map_for_output(&surface.output);
                for layer in &layers {
                    old.unmap_layer(layer);
                }
            }

            self.space.unmap_output(&surface.output);
            self.space.refresh();

            self.display_handle
                .remove_global::<Smallvil>(surface.global);

            if !layers.is_empty() {
                if let Some(target) = self.space.outputs().next().cloned() {
                    let mut map = layer_map_for_output(&target);
                    for layer in &layers {
                        if map.map_layer(layer).is_err() {
                            layer.layer_surface().send_close();
                        }
                    }
                    map.arrange();
                    crate::life(format!(
                        "hotplug: migrated {} layer surface(s) to {}",
                        layers.len(),
                        target.name(),
                    ));
                } else {
                    for layer in &layers {
                        layer.layer_surface().send_close();
                    }
                }
                self.arrange_windows();
            }
        }
    }

    fn schedule_drm_repaint(
        &self,
        node: DrmNode,
        crtc: crtc::Handle,
        delay: Duration,
        target: Time<Monotonic>,
    ) {
        let Some(drm) = self.drm.as_ref() else {
            return;
        };

        if drm.paused {
            return;
        }

        if let Err(error) =
            drm.handle
                .clone()
                .insert_source(Timer::from_duration(delay), move |_, _, state| {
                    state.render_drm(node, Some(crtc), target);
                    TimeoutAction::Drop
                })
        {
            tracing::error!(?error, "failed to schedule DRM repaint");
        }
    }

    pub fn frame_finish(
        &mut self,
        node: DrmNode,
        crtc: crtc::Handle,
        metadata: &mut Option<DrmEventMetadata>,
    ) {
        let _ = metadata;

        let Some(drm) = self.drm.as_mut() else {
            return;
        };

        if drm.paused {
            return;
        }

        let now = drm.clock.now();

        let Some(surface) = drm
            .backends
            .get_mut(&node)
            .and_then(|backend| backend.surfaces.get_mut(&crtc))
        else {
            return;
        };

        let duration = output_frame_duration(&surface.output);

        let result = surface
            .drm_output
            .frame_submitted()
            .map_err(Into::<SwapBuffersError>::into);

        let retry = match result {
            Ok(_) => true,

            Err(SwapBuffersError::AlreadySwapped) => true,

            Err(SwapBuffersError::TemporaryFailure(error)) => {
                let inactive = matches!(
                    error.downcast_ref::<DrmError>(),
                    Some(DrmError::DeviceInactive)
                );

                let denied = matches!(
                    error.downcast_ref::<DrmError>(),
                    Some(DrmError::Access(
                        DrmAccessError { source, .. }
                    )) if source.kind() == io::ErrorKind::PermissionDenied
                );

                tracing::warn!(?node, ?crtc, ?error, "frame submission failed");

                !inactive && !denied
            }

            Err(SwapBuffersError::ContextLost(error)) => {
                panic!("DRM rendering context lost: {error}");
            }
        };

        if retry {
            self.schedule_drm_repaint(
                node,
                crtc,
                Duration::from_secs_f64(duration.as_secs_f64() * 0.6),
                now + duration,
            );
        }
    }

    pub fn render_drm(
        &mut self,
        node: DrmNode,
        crtc: Option<crtc::Handle>,
        target: Time<Monotonic>,
    ) {
        if let Some(crtc) = crtc {
            self.render_surface(node, crtc, target);
            return;
        }

        let crtcs: Vec<_> = self
            .drm
            .as_ref()
            .and_then(|drm| drm.backends.get(&node))
            .map(|backend| backend.surfaces.keys().copied().collect())
            .unwrap_or_default();

        for crtc in crtcs {
            self.render_surface(node, crtc, target);
        }
    }

    fn render_surface(&mut self, node: DrmNode, crtc: crtc::Handle, target: Time<Monotonic>) {
        if self.drm().map(|drm| drm.paused).unwrap_or(true) {
            return;
        }

        let output = self
            .drm
            .as_ref()
            .and_then(|drm| drm.backends.get(&node))
            .and_then(|backend| backend.surfaces.get(&crtc))
            .map(|surface| surface.output.clone());

        let Some(output) = output else {
            return;
        };

        self.update_animations();
        self.space.refresh();

        {
            let background = self.config_watcher.config.background.clone();
            let drm = self.drm.as_mut().unwrap();
            if !drm.wallpaper_tried {
                drm.wallpaper_tried = true;
                let path = Path::new(&background);
                if path.exists() {
                    match image::open(path) {
                        Ok(img) => {
                            let sharp = img.to_rgba8();
                            crate::border::note_wallpaper_colors(&sharp);
                            drm.wallpaper_source = Some(sharp.clone());
                            drm.wallpaper_blurred = Some(drm_cursor::blur_image(&sharp, 18));
                            crate::life(format!(
                                "wallpaper: loaded {} ({}x{})",
                                path.display(),
                                sharp.width(),
                                sharp.height(),
                            ));
                        }
                        Err(error) => crate::life(format!(
                            "wallpaper: failed to open {}: {error:?}",
                            path.display(),
                        )),
                    }
                } else {
                    crate::life(format!("wallpaper: {} not found", path.display()));
                }
            }
        }

        {
            let theme = self.config_watcher.config.xcursor_theme.clone();
            let size = self.config_watcher.config.xcursor_size.max(8);
            let key = if theme.trim().is_empty() {
                None
            } else {
                Some((theme, size))
            };
            let drm = self.drm.as_mut().unwrap();
            if drm.cursor_theme_key != key {
                drm.cursor_theme_key = key.clone();
                drm.cursor_theme_pixels = key.and_then(|(theme, size)| {
                    drm_cursor::load_cursor_image_with_fallback(&theme, size as u32).map(
                        |(image, used)| {
                            if used != theme {
                                crate::life(format!(
                                    "cursor: {theme:?} not installed, using {used:?} instead"
                                ));
                            }
                            image
                        },
                    )
                });
                drm.cursor_theme_textures.clear();
                if drm.cursor_theme_pixels.is_none() && drm.cursor_theme_key.is_some() {
                    crate::life("cursor: theme missing or unloadable, using drawn arrow");
                }
            }
        }

        let frame_duration = output_frame_duration(&output);

        let Some(output_geometry) = self.space.output_geometry(&output) else {
            return;
        };

        let cursor_position = self.cursor_position;


        let config = self.config_watcher.config.clone();
        let overview = self.overview_active;
        let closing = self.overview_closing();
        let lock_active = self.exclusive_keyboard_layer().is_some();
        let canvas_view = config.layout == "canvas" || overview || closing;
        let cam = if overview {
            self.overview_camera_for(&output.name())
        } else if closing {
            self.overview_closing_camera_for(&output.name())
        } else {
            self.canvas_camera_for(&output.name())
        };
        let canvas_view_serial = self.canvas_view_serial;
        type BorderSpec = (
            smithay::backend::renderer::element::Id,
            Rectangle<i32, Logical>,
            Color32F,
            i32,
            i32,
            bool,
        );
        let mut border_specs: Vec<BorderSpec> = Vec::new();
        if config.border_width > 0 {
            let bw = config.border_width.max(1);
            let rounding = config.rounding.max(0);
            let active_arr = config.active_border_color;
            let inactive: Color32F = config.inactive_border_color.into();
            let focus_surface = self.seat.get_keyboard().and_then(|k| k.current_focus());

            for window in self.render_windows_on(&output) {
                let Some(geometry) = self.space.element_geometry(&window) else {
                    continue;
                };
                let Some(location) = self.space.element_location(&window) else {
                    continue;
                };
                let Some(toplevel) = window.toplevel() else {
                    continue;
                };
                let focused = focus_surface.as_ref() == Some(toplevel.wl_surface());
                let color: Color32F = if focused {
                    active_arr.into()
                } else {
                    inactive
                };

                let base_id = smithay::backend::renderer::element::Id::from_wayland_resource(
                    toplevel.wl_surface(),
                );
                let outer = Rectangle::new(
                    location - output_geometry.loc - smithay::utils::Point::from((bw, bw)),
                    (geometry.size.w + 2 * bw, geometry.size.h + 2 * bw).into(),
                );
                let (outer, rounding, bw) = if canvas_view {
                    (
                        cam.to_screen(Point::default(), outer),
                        ((rounding as f64 * cam.zoom).round() as i32).max(0),
                        ((bw as f64 * cam.zoom).round() as i32).max(1),
                    )
                } else {
                    (outer, rounding, bw)
                };
                border_specs.push((base_id, outer, color, rounding, bw, focused));
            }
        }

        let border_count = border_specs.len();

        let mut rounded_windows: Vec<(
            smithay::desktop::Window,
            Point<i32, Logical>,
            Rectangle<i32, Logical>,
        )> = Vec::new();
        if config.rounding > 0 || canvas_view {
            for window in self.render_windows_on(&output) {
                let Some(geometry) = self.space.element_geometry(&window) else {
                    continue;
                };
                let Some(location) = self.space.element_location(&window) else {
                    continue;
                };
                rounded_windows.push((window.clone(), location, geometry));
            }
        }

        let mut render_space: smithay::desktop::Space<smithay::desktop::Window> =
            smithay::desktop::Space::default();

        render_space.map_output(&output, output_geometry.loc);

        for window in self.render_windows_on(&output) {
            if let Some(location) = self.space.element_location(&window) {
                render_space.map_element(window, location, false);
            }
        }

        let lock_wl = self
            .lock_surface_for(&output)
            .map(|s| s.wl_surface().clone());

        let render_result: Result<(bool, Option<Result<(Vec<u8>, u32, u32), String>>), String> =
            (|| {
            let drm = self.drm.as_mut().unwrap();
            drm.frame_counter = drm.frame_counter.wrapping_add(1);
            let frame_cc: smithay::backend::renderer::utils::CommitCounter =
                drm.frame_counter.into();

            let render_node = drm
                .backends
                .get(&node)
                .and_then(|backend| backend.render_node)
                .unwrap_or(drm.primary_gpu);

            let mut renderer = drm
                .gpus
                .single_renderer(&render_node)
                .map_err(|error| format!("renderer unavailable: {error:?}"))?;

            let mut window_elements: Vec<NovaDrmElement<SurfaceRenderElement<'_>>> = Vec::new();
            let mut ring_elements: Vec<NovaDrmElement<SurfaceRenderElement<'_>>> = Vec::new();
            let mut upper_layer_elements: Vec<NovaDrmElement<SurfaceRenderElement<'_>>> =
                Vec::new();
            let mut lower_layer_elements: Vec<NovaDrmElement<SurfaceRenderElement<'_>>> =
                Vec::new();

            let scale = output.current_scale().fractional_scale();

            if config.rounding > 0 || canvas_view {
                let gles = AsMut::<GlesRenderer>::as_mut(&mut renderer);

                let program = match drm.rounded_program.get(&render_node) {
                    Some(program) => program.clone(),
                    None => {
                        let program = gles
                            .compile_custom_texture_shader(
                                drm_cursor::ROUNDED_TEXTURE_FRAGMENT,
                                &[
                                    UniformName::new("u_size", UniformType::_2f),
                                    UniformName::new("u_origin", UniformType::_2f),
                                    UniformName::new("u_radius", UniformType::_1f),
                                ],
                            )
                            .map_err(|error| format!("rounded shader compile failed: {error:?}"))?;
                        drm.rounded_program.insert(render_node, program.clone());
                        program
                    }
                };

                let scale = output.current_scale().fractional_scale();

                let focus_surface = self.seat.get_keyboard().and_then(|k| k.current_focus());

                let border_program_stub: Option<(GlesTexProgram, GlesTexture)> =
                    if !border_specs.is_empty() && config.rounding > 0 {
                        let border_program = match drm.border_program.get(&render_node).cloned() {
                            Some(program) => program,
                            None => {
                                let program = gles
                                    .compile_custom_texture_shader(
                                        drm_cursor::ROUNDED_BORDER_FRAGMENT,
                                        &[
                                            UniformName::new("u_size", UniformType::_2f),
                                            UniformName::new("u_origin", UniformType::_2f),
                                            UniformName::new("u_radius", UniformType::_1f),
                                            UniformName::new("u_border", UniformType::_1f),
                                            UniformName::new("u_color", UniformType::_4f),
                                        ],
                                    )
                                    .map_err(|error| {
                                        format!("border shader compile failed: {error:?}")
                                    })?;
                                drm.border_program.insert(render_node, program.clone());
                                program
                            }
                        };
                        let stub_texture = match drm.stub_texture.get(&render_node).cloned() {
                            Some(texture) => texture,
                            None => {
                                let texture = smithay::backend::renderer::ImportMem::import_memory(
                                    gles,
                                    &[255u8; 4],
                                    Fourcc::Argb8888,
                                    (1, 1).into(),
                                    false,
                                )
                                .map_err(|error| {
                                    format!("stub texture import failed: {error:?}")
                                })?;
                                drm.stub_texture.insert(render_node, texture.clone());
                                texture
                            }
                        };
                        Some((border_program, stub_texture))
                    } else {
                        None
                    };

                let bw_physical = (config.border_width.max(0) as f64 * scale).round() as i32;
                let bw_physical = if canvas_view {
                    ((bw_physical as f64 * cam.zoom).round() as i32).max(1)
                } else {
                    bw_physical
                };

                for (window, window_loc, window_geo) in rounded_windows.iter().rev() {
                    let window_surface = match window.underlying_surface() {
                        smithay::desktop::WindowSurface::Wayland(surface) => surface.wl_surface(),
                    };
                    let focused = focus_surface.as_ref() == Some(window_surface);
                    let id = window_surface.id();
                    let window_opacity = self
                        .window_opacity
                        .get(&id)
                        .copied()
                        .unwrap_or(if focused {
                            config.focus_opacity
                        } else {
                            config.opacity
                        })
                        .clamp(0.0, 1.0);
                    let window_region: Rectangle<i32, Physical> = Rectangle::new(
                        (*window_loc - output_geometry.loc).to_physical_precise_round(scale),
                        window_geo.size.to_physical_precise_round(scale),
                    );
                    let mask_for = |program: &smithay::backend::renderer::gles::GlesTexProgram,
                                    element: WaylandSurfaceRenderElement<GlesRenderer>,
                                    clip_to_tile: bool|
                     -> Option<NovaDrmElement<SurfaceRenderElement<'_>>> {
                        let region = element.geometry(scale.into());
                        let region = if canvas_view {
                            cam.scale_physical(scale, region)
                        } else {
                            region
                        };
                        let region = if clip_to_tile {
                            let tile = if canvas_view {
                                cam.scale_physical(scale, window_region)
                            } else {
                                window_region
                            };
                            region.intersection(tile).unwrap_or(region)
                        } else {
                            region
                        };
                        let zoom = if canvas_view { cam.zoom as f32 } else { 1.0 };
                        let radius = (config.rounding as f32 * scale as f32 * zoom)
                            .min(region.size.w.min(region.size.h) as f32 / 2.0)
                            .max(0.0);
                        let rounded = RoundedSurfaceElement::new(
                            element,
                            program.clone(),
                            region,
                            radius,
                        );
                        Some(if canvas_view {
                            NovaDrmElement::Canvas(drm_cursor::CanvasElement::new(
                                rounded,
                                cam,
                                scale,
                                drm.frame_counter
                                    ^ (canvas_view_serial as usize).wrapping_mul(0x9E37_79B9),
                            ))
                        } else {
                            NovaDrmElement::Rounded(rounded)
                        })
                    };

                    for (popup, popup_offset) in
                        smithay::desktop::PopupManager::popups_for_surface(window_surface)
                    {
                        let offset = (window_geo.loc + popup_offset - popup.geometry().loc)
                            .to_physical_precise_round(scale);

                        let popup_elements = render_elements_from_surface_tree::<
                            GlesRenderer,
                            WaylandSurfaceRenderElement<GlesRenderer>,
                        >(
                            gles,
                            popup.wl_surface(),
                            window_region.loc + offset,
                            scale,
                            window_opacity,
                            Kind::Unspecified,
                        );

                        window_elements.extend(
                            popup_elements
                                .into_iter()
                                .filter_map(|element| mask_for(&program, element, false)),
                        );
                    }

                    let mut surface_elements = render_elements_from_surface_tree::<
                        GlesRenderer,
                        WaylandSurfaceRenderElement<GlesRenderer>,
                    >(
                        gles,
                        window_surface,
                        window_region.loc,
                        scale,
                        window_opacity,
                        Kind::Unspecified,
                    );

                    let root_element = surface_elements
                        .iter()
                        .min_by_key(|element| {
                            let geo = element.geometry(scale.into());
                            (geo.size.w - window_region.size.w).abs()
                                + (geo.size.h - window_region.size.h).abs()
                        })
                        .or_else(|| {
                            surface_elements.iter().max_by_key(|e| {
                                let geo = e.geometry(scale.into());
                                geo.size.w.saturating_mul(geo.size.h)
                            })
                        })
                        .or_else(|| surface_elements.first());
                    let toplevel_id =
                        smithay::backend::renderer::element::Id::from_wayland_resource(
                            window_surface,
                        );
                    let root_is_toplevel =
                        root_element.is_some_and(|e| e.id() == &toplevel_id);
                    let mut root_geometry =
                        root_element.map(|e| e.geometry(scale.into()));
                    let geo_offset = if root_is_toplevel {
                        window.geometry().loc.to_physical_precise_round(scale)
                    } else {
                        Point::from((0, 0))
                    };
                    if let Some(root) = root_geometry {
                        let correction = window_region.loc - (root.loc + geo_offset);
                        if correction != (0, 0).into() {
                            surface_elements = render_elements_from_surface_tree::<
                                GlesRenderer,
                                WaylandSurfaceRenderElement<GlesRenderer>,
                            >(
                                gles,
                                window_surface,
                                window_region.loc + correction,
                                scale,
                                window_opacity,
                                Kind::Unspecified,
                            );
                            root_geometry = Some(Rectangle::new(root.loc + correction, root.size));
                        }
                    }
                    if std::env::var("NOVA_DEBUG_GEOM").is_ok() {
                        use std::io::Write as _;
                        use std::sync::atomic::{AtomicU64, Ordering};
                        static TREE_FRAME: AtomicU64 = AtomicU64::new(0);
                        let tree_frame_no = TREE_FRAME.fetch_add(1, Ordering::Relaxed);
                        if tree_frame_no % 120 == 0 {
                            let tree_summary: String = surface_elements
                                .iter()
                                .enumerate()
                                .map(|(i, e)| {
                                    let id = e.id();
                                    let is_toplevel = id
                                        == &smithay::backend::renderer::element::Id::from_wayland_resource(
                                            window_surface,
                                        );
                                    format!(
                                        "[{i}{} {:?} {is_toplevel}]",
                                        if is_toplevel { "*" } else { "" },
                                        e.geometry(scale.into()),
                                    )
                                })
                                .collect();
                            let _ = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open("/tmp/nova_debug.log")
                                .and_then(|mut f| {
                                    writeln!(f, "tree: f={tree_frame_no} n={} {tree_summary}", surface_elements.len())
                                });
                        }
                    }

                    window_elements.extend(
                        surface_elements
                            .into_iter()
                            .filter(|element| {
                                root_geometry
                                    .map(|root| element.geometry(scale.into()).overlaps(root))
                                    .unwrap_or(true)
                            })
                            .filter_map(|element| mask_for(&program, element, true)),
                    );

                    if let (Some((border_program, stub_texture)), Some(toplevel)) =
                        (&border_program_stub, window.toplevel())
                    {
                        let base_id =
                            smithay::backend::renderer::element::Id::from_wayland_resource(
                                toplevel.wl_surface(),
                            );
                        let focused = focus_surface.as_ref() == Some(window_surface);
                        let color = if focused {
                            crate::border::border_effect_color(
                                config.active_border_color,
                                &config.border_effect,
                                crate::border::border_effect_clock(config.border_effect_speed),
                                0.0,
                                config.border_gradient_color,
                            )
                        } else {
                            config.inactive_border_color
                        };
                        let content_size = root_geometry
                            .map(|r| r.size)
                            .unwrap_or(window_region.size);
                        let content_size = (
                            content_size.w.min(window_region.size.w),
                            content_size.h.min(window_region.size.h),
                        )
                            .into();
                        let base_rect =
                            Rectangle::new(window_region.loc, content_size);
                        let base_rect = if canvas_view {
                            cam.scale_physical(scale, base_rect)
                        } else {
                            base_rect
                        };
                        if std::env::var("NOVA_DEBUG_GEOM").is_ok() {
                            use std::io::Write as _;
                            use std::sync::atomic::{AtomicU64, Ordering};
                            static FRAME: AtomicU64 = AtomicU64::new(0);
                            let frame_no = FRAME.fetch_add(1, Ordering::Relaxed);
                            if frame_no % 120 == 0 {
                                let output_name = output.name();
                                let _ = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open("/tmp/nova_debug.log")
                                    .and_then(|mut f| {
                                        writeln!(
                                            f,
                                            "win: frame={frame_no} window_loc={window_loc:?} window_geo={window_geo:?} \
                                             window_region={window_region:?} root_geometry={root_geometry:?} \
                                             output={output_name:?}"
                                        )
                                    });
                            }
                        }
                        let outer = Rectangle::new(
                            base_rect.loc
                                - smithay::utils::Point::from((bw_physical, bw_physical)),
                            (
                                (base_rect.size.w + 2 * bw_physical).max(1),
                                (base_rect.size.h + 2 * bw_physical).max(1),
                            )
                                .into(),
                        );
                        let zoom = if canvas_view { cam.zoom as f32 } else { 1.0 };
                        let ring_radius = (config.rounding as f32 * scale as f32 * zoom)
                            .min(base_rect.size.w.min(base_rect.size.h) as f32 / 2.0)
                            .max(0.0);
                        ring_elements.push(NovaDrmElement::Border(RoundedBorderElement::new(
                            base_id.namespaced(0x4000),
                            border_program.clone(),
                            stub_texture.clone(),
                            outer,
                            bw_physical.max(1) as f32,
                            ring_radius,
                            color.into(),
                            frame_cc,
                        )));
                    }
                }
                if !(overview || closing) || lock_active {
                    let gles = AsMut::<GlesRenderer>::as_mut(&mut renderer);
                    let map = layer_map_for_output(&output);
                    let layers: Vec<_> = map.layers().cloned().collect();
                    for layer in layers {
                        let Some(geo) = map.layer_geometry(&layer) else {
                            continue;
                        };
                        let layer_loc: Point<i32, Physical> =
                            geo.loc.to_physical_precise_round(scale);
                        let layer_surface = layer.wl_surface();
                        let upper =
                            matches!(layer.layer(), WlrLayer::Top | WlrLayer::Overlay);
                        let push: &mut Vec<NovaDrmElement<SurfaceRenderElement<'_>>> =
                            if upper {
                                &mut upper_layer_elements
                            } else {
                                &mut lower_layer_elements
                            };

                        for (popup, popup_offset) in
                            smithay::desktop::PopupManager::popups_for_surface(
                                layer_surface,
                            )
                        {
                            let offset = (popup_offset - popup.geometry().loc)
                                .to_physical_precise_round(scale);
                            for element in render_elements_from_surface_tree::<
                                GlesRenderer,
                                WaylandSurfaceRenderElement<GlesRenderer>,
                            >(
                                gles,
                                popup.wl_surface(),
                                layer_loc + offset,
                                scale,
                                1.0,
                                Kind::Unspecified,
                            ) {
                                let mut region = element.geometry(scale.into());
                                region.loc -= (1, 1).into();
                                region.size = (region.size.w + 2, region.size.h + 2).into();
                                push.push(NovaDrmElement::Rounded(
                                    RoundedSurfaceElement::new(
                                        element,
                                        program.clone(),
                                        region,
                                        0.0,
                                    ),
                                ));
                            }
                        }

                        for element in render_elements_from_surface_tree::<
                            GlesRenderer,
                            WaylandSurfaceRenderElement<GlesRenderer>,
                        >(
                            gles, layer_surface, layer_loc, scale, 1.0, Kind::Unspecified,
                        ) {
                            let mut region = element.geometry(scale.into());
                            region.loc -= (1, 1).into();
                            region.size = (region.size.w + 2, region.size.h + 2).into();
                            push.push(NovaDrmElement::Rounded(
                                RoundedSurfaceElement::new(
                                    element,
                                    program.clone(),
                                    region,
                                    0.0,
                                ),
                            ));
                        }
                    }
                }
            } else {
                window_elements = space_render_elements(&mut renderer, [&render_space], &output, 1.0)
                    .map_err(|error| format!("surface element collection failed: {error:?}"))?
                    .into_iter()
                    .map(|element| NovaDrmElement::Space(Wrap::from(element)))
                    .collect();
            }

            let themed_cursor = {
                let gles = AsMut::<GlesRenderer>::as_mut(&mut renderer);
                ensure_cursor_texture(
                    &mut drm.cursor_theme_textures,
                    drm.cursor_theme_pixels.as_ref(),
                    &render_node,
                    gles,
                )
            };
            let mut elements: Vec<NovaDrmElement<SurfaceRenderElement<'_>>> = Vec::new();
            if let Some((texture, (width, height, xhot, yhot))) = themed_cursor {
                let x = ((cursor_position.x - output_geometry.loc.x as f64 - f64::from(xhot))
                    * scale)
                    .round() as i32;
                let y = ((cursor_position.y - output_geometry.loc.y as f64 - f64::from(yhot))
                    * scale)
                    .round() as i32;
                let width = (f64::from(width) * scale).round() as i32;
                let height = (f64::from(height) * scale).round() as i32;
                let output_size = output_geometry.size.to_physical_precise_round(scale);
                if x + width > 0 && y + height > 0 && x < output_size.w && y < output_size.h {
                    let id = drm.cursor_theme_id.clone();
                    elements.push(NovaDrmElement::Wallpaper(
                        drm_cursor::WallpaperElement::new(
                            id,
                            texture,
                            Rectangle::<i32, Physical>::new((x, y).into(), (width, height).into()),
                            frame_cc,
                            1.0,
                        ),
                    ));
                }
            }
            if elements.is_empty() {
                elements = drm
                    .cursor_sprite
                    .elements(cursor_position, output_geometry, &output, frame_cc)
                    .into_iter()
                    .map(NovaDrmElement::Cursor)
                    .collect();
            }
            let cursor_len = elements.len();

            if config.rounding == 0 {
                for (id, outer, color, _rounding, bw, focused) in &border_specs {
                    let ring = crate::border::border_ring_rects(*outer, *bw, 0);
                    let center = (
                        outer.loc.x as f32 + outer.size.w as f32 / 2.0,
                        outer.loc.y as f32 + outer.size.h as f32 / 2.0,
                    );
                    for (index, rect) in ring.into_iter().enumerate() {
                        let mut color = *color;
                        if *focused
                            && crate::border::is_animated_border_effect(&config.border_effect)
                        {
                            let angle = crate::border::ring_angle(
                                (
                                    rect.loc.x as f32 + rect.size.w as f32 / 2.0,
                                    rect.loc.y as f32 + rect.size.h as f32 / 2.0,
                                ),
                                center,
                            );
                            color = crate::border::border_effect_color(
                                config.active_border_color,
                                &config.border_effect,
                                crate::border::border_effect_clock(config.border_effect_speed),
                                angle,
                                config.border_gradient_color,
                            )
                            .into();
                        }
                        let elem = smithay::backend::renderer::element::solid::SolidColorRenderElement::new(
                            id.namespaced(index),
                            rect.to_physical_precise_round(scale),
                            frame_cc,
                            color,
                            smithay::backend::renderer::element::Kind::Unspecified,
                        );
                        elements.push(NovaDrmElement::Cursor(elem));
                    }
                }
            }

            elements.extend(ring_elements);

            elements.extend(window_elements);

            elements.extend(lower_layer_elements);
            elements.splice(cursor_len..cursor_len, upper_layer_elements);

            if let Some(ref lock_surface) = lock_wl {
                let gles = AsMut::<GlesRenderer>::as_mut(&mut renderer);
                let program = match drm.rounded_program.get(&render_node) {
                    Some(program) => program.clone(),
                    None => {
                        let program = gles
                            .compile_custom_texture_shader(
                                drm_cursor::ROUNDED_TEXTURE_FRAGMENT,
                                &[
                                    UniformName::new("u_size", UniformType::_2f),
                                    UniformName::new("u_origin", UniformType::_2f),
                                    UniformName::new("u_radius", UniformType::_1f),
                                ],
                            )
                            .map_err(|error| {
                                format!("lock surface shader compile failed: {error:?}")
                            })?;
                        drm.rounded_program.insert(render_node, program.clone());
                        program
                    }
                };
                let lock_loc: Point<i32, Physical> = (0, 0).into();
                let mut lock_elems: Vec<NovaDrmElement<SurfaceRenderElement<'_>>> = Vec::new();
                for element in render_elements_from_surface_tree::<
                    GlesRenderer,
                    WaylandSurfaceRenderElement<GlesRenderer>,
                >(
                    gles,
                    lock_surface,
                    lock_loc,
                    scale,
                    1.0,
                    Kind::Unspecified,
                ) {
                    let mut region = element.geometry(scale.into());
                    region.loc -= (1, 1).into();
                    region.size = (region.size.w + 2, region.size.h + 2).into();
                    lock_elems.push(NovaDrmElement::Rounded(RoundedSurfaceElement::new(
                        element,
                        program.clone(),
                        region,
                        0.0,
                    )));
                }
                elements.splice(cursor_len..cursor_len, lock_elems);
            }

            {
                let gles = AsMut::<GlesRenderer>::as_mut(&mut renderer);
                if drm.wallpaper_source.is_some() {
                    let pending = drm
                        .wallpaper_textures
                        .entry(render_node)
                        .or_insert([None, None]);
                    if pending[0].is_none() {
                        if let Some(sharp) = drm.wallpaper_source.as_ref() {
                            let tex_size = smithay::utils::Size::from((
                                sharp.width() as i32,
                                sharp.height() as i32,
                            ));
                            match smithay::backend::renderer::ImportMem::import_memory(
                                gles,
                                sharp.deref(),
                                Fourcc::Abgr8888,
                                tex_size,
                                false,
                            ) {
                                Ok(tex) => pending[0] = Some(tex),
                                Err(error) => crate::life(format!(
                                    "wallpaper: sharp texture import failed: {error:?}"
                                )),
                            }
                        }
                    }
                    if pending[1].is_none() {
                        if let Some(blurred) = drm.wallpaper_blurred.as_ref() {
                            let tex_size = smithay::utils::Size::from((
                                blurred.width() as i32,
                                blurred.height() as i32,
                            ));
                            match smithay::backend::renderer::ImportMem::import_memory(
                                gles,
                                blurred.deref(),
                                Fourcc::Abgr8888,
                                tex_size,
                                false,
                            ) {
                                Ok(tex) => pending[1] = Some(tex),
                                Err(error) => crate::life(format!(
                                    "wallpaper: blurred texture import failed: {error:?}"
                                )),
                            }
                        }
                    }
                }
            }

            let wallpaper_texture = drm
                .wallpaper_textures
                .get(&render_node)
                .and_then(|textures| {
                    if config.blur {
                        textures[1].as_ref().or(textures[0].as_ref())
                    } else {
                        textures[0].as_ref()
                    }
                })
                .cloned();
            if let Some(texture) = wallpaper_texture {
                let scale = output.current_scale().fractional_scale();
                let region = Rectangle::from_size(
                    output_geometry.size.to_physical_precise_round(scale),
                );
                let wallpaper_cc: smithay::backend::renderer::utils::CommitCounter =
                    (if config.blur { 1 } else { 0 }).into();
                let element = drm_cursor::WallpaperElement::new(
                    smithay::backend::renderer::element::Id::new(),
                    texture,
                    region,
                    wallpaper_cc,
                    1.0,
                );
                elements.push(NovaDrmElement::Wallpaper(element));
            }

            if config.grain && config.grain_intensity > 0.0 {
                {
                    let gles = AsMut::<GlesRenderer>::as_mut(&mut renderer);
                    let scale = output.current_scale().fractional_scale();
                    let size: smithay::utils::Size<i32, smithay::utils::Physical> =
                        output_geometry.size.to_physical_precise_round(scale);
                    ensure_grain_frames(
                        &mut drm.grain_frames,
                        &render_node,
                        gles,
                        (size.w as u32, size.h as u32),
                    );
                }
                let grain_key = (
                    render_node,
                    {
                        let scale = output.current_scale().fractional_scale();
                        let size: smithay::utils::Size<i32, smithay::utils::Physical> =
                            output_geometry.size.to_physical_precise_round(scale);
                        (size.w as u32, size.h as u32)
                    },
                );
                let cycle = (drm.frame_counter / 24)
                    % drm.grain_frames.get(&grain_key).map(|f| f.len()).unwrap_or(1).max(1);
                let grain_texture = drm.grain_frames.get(&grain_key).and_then(|frames| {
                    frames.get(cycle).cloned()
                });
                if let Some(texture) = grain_texture {
                    let scale = output.current_scale().fractional_scale();
                    let region = Rectangle::from_size(
                        output_geometry.size.to_physical_precise_round(scale),
                    );
                    let grain_cc: smithay::backend::renderer::utils::CommitCounter =
                        (drm.frame_counter / 24).into();
                    let element = drm_cursor::WallpaperElement::new(
                        smithay::backend::renderer::element::Id::new(),
                        texture,
                        region,
                        grain_cc,
                        config.grain_intensity.clamp(0.0, 1.0),
                    );
                    elements.push(NovaDrmElement::Wallpaper(element));
                }
            }

            let armed = self
                .screenshot_capture
                .clone()
                .filter(|cap| cap.output == output.name());
            if let Some(cap) = armed {
                match capture_output_screenshot(
                    &mut renderer,
                    &mut drm.screenshot_textures,
                    render_node,
                    &elements,
                    &output,
                    output_geometry,
                    scale,
                    cap.rect.as_ref(),
                ) {
                    Ok((rgba, w, h)) => {
                        self.screenshot_capture = None;
                        match crate::screenshot::encode_png_rgba(w, h, rgba) {
                            Ok(png) => match &cap.dest {
                                crate::screenshot::ScreenshotDest::Save(path) => {
                                    match std::fs::write(path, &png) {
                                        Ok(()) => crate::life(format!(
                                            "screenshot: saved {} ({w}x{h})",
                                            path.display(),
                                        )),
                                        Err(error) => crate::life(format!(
                                            "screenshot: save failed {}: {error}",
                                            path.display(),
                                        )),
                                    }
                                }
                                crate::screenshot::ScreenshotDest::Copy => {
                                    let bytes = png.len();
                                    self.clipboard_png = Some(png);
                                    set_data_device_selection(
                                        &self.display_handle,
                                        &self.seat,
                                        vec!["image/png".to_string()],
                                        (),
                                    );
                                    crate::life(format!(
                                        "screenshot: copied {w}x{h} ({bytes} bytes)"
                                    ));
                                }
                            },
                            Err(error) => crate::life(format!("screenshot: {error}")),
                        }
                    }
                    Err(error) => {
                        self.screenshot_capture = None;
                        crate::life(format!("screenshot: capture failed: {error}"));
                    }
                }
            }

            {
                let pending: Vec<_> = self
                    .pending_capture_frames
                    .drain(..)
                    .collect();
                let mut keep = Vec::new();
                for (name, frame) in pending {
                    if name != output.name() {
                        keep.push((name, frame));
                        continue;
                    }
                    match capture_output_screenshot(
                        &mut renderer,
                        &mut drm.screenshot_textures,
                        render_node,
                        &elements,
                        &output,
                        output_geometry,
                        scale,
                        None,
                    ) {
                        Ok((rgba, w, h)) => {
                            let buffer = frame.buffer();
                            match crate::handlers::capture::write_rgba_to_shm_buffer(
                                &buffer, &rgba, w, h,
                            ) {
                                Ok(()) => {
                                    let transform = output.current_transform();
                                    let presented = self.start_time.elapsed();
                                    crate::life(format!(
                                        "capture: frame ready {name} {w}x{h}"
                                    ));
                                    frame.success(transform, None, presented);
                                }
                                Err(error) => {
                                    crate::life(format!("capture: {error}"));
                                    frame.fail(
                                        smithay::wayland::image_copy_capture::CaptureFailureReason::Unknown,
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            crate::life(format!("capture: frame failed: {error}"));
                            frame.fail(
                                smithay::wayland::image_copy_capture::CaptureFailureReason::Unknown,
                            );
                        }
                    }
                }
                self.pending_capture_frames = keep;
            }

            let mut pending_record: Option<Result<(Vec<u8>, u32, u32), String>> = None;
            let recording_here = self
                .record_session
                .as_ref()
                .is_some_and(|session| session.output == output.name());
            if recording_here {
                let due = self.record_session.as_ref().is_some_and(|session| {
                    session.last_capture.elapsed() >= crate::record::FRAME_INTERVAL
                });
                if due {
                    pending_record = Some(capture_output_screenshot(
                        &mut renderer,
                        &mut drm.screenshot_textures,
                        render_node,
                        &elements,
                        &output,
                        output_geometry,
                        scale,
                        None,
                    ));
                }
            }

            if let Some(sel) = self.screenshot_select.as_ref() {
                if sel.output == output.name() {
                    let overlay = push_screenshot_overlay(
                        output_geometry,
                        scale,
                        frame_cc,
                        &sel.anchor,
                        &sel.current,
                        sel.dragging,
                    );
                    elements.splice(cursor_len..cursor_len, overlay);
                }
            }

            if let Some(picker) = self.record_picker.as_ref() {
                let name = output.name();
                if let Some(index) = picker.outputs.iter().position(|o| o == &name) {
                    let overlay = push_record_picker_overlay(
                        &name,
                        &picker.highlight,
                        index + 1,
                        output_geometry,
                        scale,
                        frame_cc,
                    );
                    elements.splice(cursor_len..cursor_len, overlay);
                }
            }
            if self
                .record_session
                .as_ref()
                .is_some_and(|session| session.output == output.name())
            {
                let dot = push_record_dot(output_geometry, scale, frame_cc);
                elements.splice(cursor_len..cursor_len, dot);
            }

            let surface = drm
                .backends
                .get_mut(&node)
                .and_then(|backend| backend.surfaces.get_mut(&crtc))
                .ok_or_else(|| "DRM surface disappeared".to_string())?;

            let clear: smithay::backend::renderer::Color32F = [0.1, 0.1, 0.1, 1.0].into();

            let frame_render_start = Instant::now();

            let (n_space, n_cursor, n_rounded, n_border): (usize, usize, usize, usize) =
                elements.iter().fold((0, 0, 0, 0), |(s, c, r, b), e| match e {
                    NovaDrmElement::Space(_) => (s + 1, c, r, b),
                    NovaDrmElement::Cursor(_) => (s, c + 1, r, b),
                    NovaDrmElement::Rounded(_) => (s, c, r + 1, b),
                    NovaDrmElement::Canvas(_) => (s, c, r + 1, b),
                    NovaDrmElement::Border(_) => (s, c, r, b + 1),
                    NovaDrmElement::Wallpaper(_) => (s, c, r, b),
                });

            let empty = surface
                .drm_output
                .render_frame::<_, _>(&mut renderer, &elements, clear, FrameFlags::DEFAULT)
                .map(|result| result.is_empty)
                .map_err(|error| format!("render_frame failed: {error:?}"))?;

            let render_took = frame_render_start.elapsed();

            if !empty {
                surface
                    .drm_output
                    .queue_frame(None)
                    .map_err(|error| format!("queue_frame failed: {error:?}"))?;
            }

            if drm.frame_counter % 30 == 0 {
                crate::life(format!(
                    "DRMFMT out={} empty={empty} total={} space={n_space} cursor={n_cursor} rounded={n_rounded} border={n_border} windows={} borders={border_count} rounding={} render={render_took:?}",
                    output.name(),
                    elements.len(),
                    rounded_windows.len(),
                    config.rounding,
                ));
            }

            Ok((!empty, pending_record))
        })();

        let (queued, pending_record) = match render_result {
            Ok(pair) => pair,
            Err(error) => {
                tracing::warn!(
                    output = %output.name(),
                    %error,
                    "DRM repaint failed"
                );
                let delay = Duration::from_millis(100);
                self.schedule_drm_repaint(node, crtc, delay, target + delay);
                return;
            }
        };

        if let Some(result) = pending_record {
            match result {
                Ok((rgba, width, height)) => {
                    self.record_push_frame(&rgba, width, height);
                }
                Err(error) => {
                    self.record_abort(&format!("capture failed: {error}"));
                }
            }
        }

        let elapsed = FRAME_EPOCH.get_or_init(Instant::now).elapsed();

        for window in self.render_windows_on(&output) {
            window.send_frame(
                &output,
                elapsed,
                Some(Duration::from_secs(1)),
                |_, _| Some(output.clone()),
            );
        }
        if let Some(lock_surface) = self.lock_surface_for(&output) {
            smithay::desktop::utils::send_frames_surface_tree(
                lock_surface.wl_surface(),
                &output,
                elapsed,
                Some(Duration::from_secs(1)),
                |_, _| Some(output.clone()),
            );
        }

        {
            let mut map = layer_map_for_output(&output);
            for layer in map.layers() {
                layer.send_frame(
                    &output,
                    elapsed,
                    Some(Duration::from_secs(1)),
                    |_, _| Some(output.clone()),
                );
            }
            map.cleanup();
        }
        self.popups.cleanup();

        if !queued {
            self.schedule_drm_repaint(node, crtc, frame_duration, target + frame_duration);
        }
    }
}
