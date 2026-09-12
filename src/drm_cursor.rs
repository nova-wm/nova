use std::collections::HashMap;

use smithay::{
    backend::renderer::{
        Color32F, Frame, RendererSuper, Texture,
        element::{
            Element, Id, Kind, RenderElement, Wrap,
            solid::SolidColorRenderElement,
            surface::{WaylandSurfaceRenderElement, WaylandSurfaceTexture},
        },
        gles::{GlesRenderer, GlesTexProgram, GlesTexture, Uniform},
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    output::Output,
    utils::{
        user_data::UserDataMap,
        Buffer, Logical, Physical, Point, Rectangle, Scale, Transform,
    },
};

use crate::Smallvil;

/// Fragment shader driving rounded window corners.
///
pub(crate) const ROUNDED_TEXTURE_FRAGMENT: &str = r#"
#version 100

//_DEFINES_

precision highp float;
uniform sampler2D tex;
uniform float alpha;
uniform vec2 u_size;
uniform vec2 u_origin;
uniform float u_radius;
varying vec2 v_coords;

float rounded_rect_mask(vec2 p, vec2 size, float radius) {
    vec2 q = abs(p) - (size - vec2(radius));
    vec2 clamped = max(q, 0.0);
    float outside = length(clamped);
    float inside = min(max(q.x, q.y), 0.0);
    float dist = outside + inside - radius;
    return 1.0 - clamp(dist + 0.5, 0.0, 1.0);
}

void main() {
    vec4 color = texture2D(tex, v_coords);
    vec2 p = v_coords * u_size + u_origin;
    vec2 centered = p - u_size * 0.5;
    float mask = rounded_rect_mask(centered, u_size * 0.5, u_radius);
    gl_FragColor = vec4(color.rgb, color.a) * (alpha * mask);
}
"#;

/// Fragment shader drawing a rounded border ring (annulus) between an outer
/// rounded rectangle (radius `u_radius + u_border`) and the inner window
pub(crate) const ROUNDED_BORDER_FRAGMENT: &str = r#"
#version 100

//_DEFINES_

precision highp float;
uniform sampler2D tex;
uniform float alpha;
uniform vec2 u_size;
uniform vec2 u_origin;
uniform float u_radius;
uniform float u_border;
uniform vec4 u_color;
varying vec2 v_coords;

float rounded_rect_mask(vec2 p, vec2 size, float radius) {
    vec2 q = abs(p) - (size - vec2(radius));
    vec2 clamped = max(q, 0.0);
    float outside = length(clamped);
    float inside = min(max(q.x, q.y), 0.0);
    float dist = outside + inside - radius;
    return 1.0 - clamp(dist + 0.5, 0.0, 1.0);
}

void main() {
    vec2 p = v_coords * u_size + u_origin;
    vec2 center = p - (u_origin + u_size * 0.5);
    vec2 hsize = u_size * 0.5;
    float radius_out = u_radius + u_border;
    float outer = rounded_rect_mask(center, hsize, radius_out);
    vec2 half_inner = hsize - vec2(u_border);
    float inner = rounded_rect_mask(center, half_inner, u_radius);
    float mask = outer * (1.0 - inner);
    gl_FragColor = vec4(u_color.rgb * u_color.a, u_color.a) * (mask * alpha);
}
"#;

/// Render elements drawn by NovaWM's DRM backend.
///
#[allow(clippy::large_enum_variant)]
pub enum NovaDrmElement<E> {
    /// A plain surface/space element.
    Space(Wrap<E>),
    /// A flat colored rectangle (software cursor and border ring).
    Cursor(SolidColorRenderElement),
    /// A window surface drawn with rounded corners.
    Rounded(RoundedSurfaceElement),
    /// A smooth rounded border ring around a window.
    Border(RoundedBorderElement),
    /// A full-output background texture (wallpaper).
    Wallpaper(WallpaperElement),
    /// A window surface viewed through the canvas camera (canvas layout).
    Canvas(CanvasElement),
}

impl<E> Element for NovaDrmElement<E>
where
    E: Element,
    RoundedSurfaceElement: Element,
{
    fn id(&self) -> &Id {
        match self {
            Self::Space(element) => element.id(),
            Self::Cursor(element) => element.id(),
            Self::Rounded(element) => element.id(),
            Self::Border(element) => element.id(),
            Self::Wallpaper(element) => element.id(),
            Self::Canvas(element) => element.id(),
        }
    }

    fn current_commit(&self) -> CommitCounter {
        match self {
            Self::Space(element) => element.current_commit(),
            Self::Cursor(element) => element.current_commit(),
            Self::Rounded(element) => element.current_commit(),
            Self::Border(element) => element.current_commit(),
            Self::Wallpaper(element) => element.current_commit(),
            Self::Canvas(element) => element.current_commit(),
        }
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        match self {
            Self::Space(element) => element.location(scale),
            Self::Cursor(element) => element.location(scale),
            Self::Rounded(element) => element.location(scale),
            Self::Border(element) => element.location(scale),
            Self::Wallpaper(element) => element.location(scale),
            Self::Canvas(element) => element.location(scale),
        }
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        match self {
            Self::Space(element) => element.src(),
            Self::Cursor(element) => element.src(),
            Self::Rounded(element) => element.src(),
            Self::Border(element) => element.src(),
            Self::Wallpaper(element) => element.src(),
            Self::Canvas(element) => element.src(),
        }
    }

    fn transform(&self) -> Transform {
        match self {
            Self::Space(element) => element.transform(),
            Self::Cursor(element) => element.transform(),
            Self::Rounded(element) => element.transform(),
            Self::Border(element) => element.transform(),
            Self::Wallpaper(element) => element.transform(),
            Self::Canvas(element) => element.transform(),
        }
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            Self::Space(element) => element.geometry(scale),
            Self::Cursor(element) => element.geometry(scale),
            Self::Rounded(element) => element.geometry(scale),
            Self::Border(element) => element.geometry(scale),
            Self::Wallpaper(element) => element.geometry(scale),
            Self::Canvas(element) => element.geometry(scale),
        }
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match self {
            Self::Space(element) => element.damage_since(scale, commit),
            Self::Cursor(element) => element.damage_since(scale, commit),
            Self::Rounded(element) => element.damage_since(scale, commit),
            Self::Border(element) => element.damage_since(scale, commit),
            Self::Wallpaper(element) => element.damage_since(scale, commit),
            Self::Canvas(element) => element.damage_since(scale, commit),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        match self {
            Self::Space(element) => element.opaque_regions(scale),
            Self::Cursor(element) => element.opaque_regions(scale),
            Self::Rounded(_element) => OpaqueRegions::default(),
            Self::Border(_element) => OpaqueRegions::default(),
            Self::Wallpaper(_element) => OpaqueRegions::default(),
            Self::Canvas(element) => element.opaque_regions(scale),
        }
    }

    fn alpha(&self) -> f32 {
        match self {
            Self::Space(element) => element.alpha(),
            Self::Cursor(element) => element.alpha(),
            Self::Rounded(element) => element.alpha(),
            Self::Border(element) => element.alpha(),
            Self::Wallpaper(element) => element.alpha(),
            Self::Canvas(element) => element.alpha(),
        }
    }

    fn kind(&self) -> Kind {
        match self {
            Self::Space(element) => element.kind(),
            Self::Cursor(element) => element.kind(),
            Self::Rounded(element) => element.kind(),
            Self::Border(element) => element.kind(),
            Self::Wallpaper(element) => element.kind(),
            Self::Canvas(element) => element.kind(),
        }
    }

    fn is_framebuffer_effect(&self) -> bool {
        match self {
            Self::Space(element) => element.is_framebuffer_effect(),
            Self::Cursor(element) => element.is_framebuffer_effect(),
            Self::Rounded(_element) => false,
            Self::Border(_element) => false,
            Self::Wallpaper(_element) => false,
            Self::Canvas(element) => element.is_framebuffer_effect(),
        }
    }
}

impl<'a, E> RenderElement<super::UdevRenderer<'a>> for NovaDrmElement<E>
where
    E: RenderElement<super::UdevRenderer<'a>>,
{
    fn draw(
        &self,
        frame: &mut <super::UdevRenderer<'a> as RendererSuper>::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), <super::UdevRenderer<'a> as RendererSuper>::Error> {
        match self {
            Self::Space(element) => RenderElement::<super::UdevRenderer<'a>>::draw(
                element,
                frame,
                src,
                dst,
                damage,
                opaque_regions,
                cache,
            ),
            Self::Cursor(element) => RenderElement::<super::UdevRenderer<'a>>::draw(
                element,
                frame,
                src,
                dst,
                damage,
                opaque_regions,
                cache,
            ),
            Self::Rounded(element) => RenderElement::<super::UdevRenderer<'a>>::draw(
                element,
                frame,
                src,
                dst,
                damage,
                opaque_regions,
                cache,
            ),
            Self::Border(element) => RenderElement::<super::UdevRenderer<'a>>::draw(
                element,
                frame,
                src,
                dst,
                damage,
                opaque_regions,
                cache,
            ),
            Self::Wallpaper(element) => RenderElement::<super::UdevRenderer<'a>>::draw(
                element,
                frame,
                src,
                dst,
                damage,
                opaque_regions,
                cache,
            ),
            Self::Canvas(element) => RenderElement::<super::UdevRenderer<'a>>::draw(
                element,
                frame,
                src,
                dst,
                damage,
                opaque_regions,
                cache,
            ),
        }
    }
}

/// A window surface viewed through the canvas camera (canvas layout only).
///
pub struct CanvasElement {
    inner: RoundedSurfaceElement,
    cam: crate::CanvasCamera,
    output_scale: f64,
    /// Opaque commit token (DRM frame counter + canvas_view_serial). Smithay's
    /// CommitCounter is a newtype over usize with only From<usize> - no way to
    commit_token: usize,
}

impl CanvasElement {
    pub fn new(
        inner: RoundedSurfaceElement,
        cam: crate::CanvasCamera,
        output_scale: f64,
        commit_token: usize,
    ) -> Self {
        Self {
            inner,
            cam,
            output_scale,
            commit_token,
        }
    }

    fn scale_rect(&self, rect: Rectangle<i32, Physical>) -> Rectangle<i32, Physical> {
        self.cam.scale_physical(self.output_scale, rect)
    }
}

impl Element for CanvasElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit_token.into()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        let loc = self.inner.location(scale);
        let zoom = self.cam.zoom.max(0.05);
        Point::from((
            (loc.x as f64 * zoom + self.cam.offset_x * self.output_scale).round() as i32,
            (loc.y as f64 * zoom + self.cam.offset_y * self.output_scale).round() as i32,
        ))
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.scale_rect(self.inner.geometry(scale))
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if commit == Some(self.current_commit()) {
            return Default::default();
        }
        std::iter::once(self.geometry(scale)).collect()
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }

    fn is_framebuffer_effect(&self) -> bool {
        false
    }
}

impl<'a> RenderElement<super::UdevRenderer<'a>> for CanvasElement {
    fn draw(
        &self,
        frame: &mut <super::UdevRenderer<'a> as RendererSuper>::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), <super::UdevRenderer<'a> as RendererSuper>::Error> {
        RenderElement::<super::UdevRenderer<'a>>::draw(
            &self.inner,
            frame,
            src,
            dst,
            damage,
            &[],
            cache,
        )
    }
}

/// A [`WaylandSurfaceRenderElement`] drawn with rounded corners.
///
pub struct RoundedSurfaceElement {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    /// The rounded rectangle the shader clips against, in output-local
    /// physical pixels.
    region: Rectangle<i32, Physical>,
    /// Corner radius in physical pixels.
    radius: f32,
}

impl RoundedSurfaceElement {
    /// Create a rounded element for `inner`, clipped to `region` with `radius`.
    pub fn new(
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        program: GlesTexProgram,
        region: Rectangle<i32, Physical>,
        radius: f32,
    ) -> Self {
        Self {
            inner,
            program,
            region,
            radius,
        }
    }
}

impl Element for RoundedSurfaceElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<'a> RenderElement<super::UdevRenderer<'a>> for RoundedSurfaceElement {
    fn draw(
        &self,
        frame: &mut <super::UdevRenderer<'a> as RendererSuper>::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), <super::UdevRenderer<'a> as RendererSuper>::Error> {
        if std::env::var("NOVA_DEBUG_GEOM").is_ok() {
            use std::io::Write as _;
            use std::sync::atomic::{AtomicU64, Ordering};
            static FRAME: AtomicU64 = AtomicU64::new(0);
            let frame = FRAME.fetch_add(1, Ordering::Relaxed);
            if frame % 120 == 0 {
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/nova_debug.log")
                    .and_then(|mut f| {
                        writeln!(
                            f,
                            "round: frame={frame} dst={dst:?} region={:?} delta=(dloc={},dloc={}) \
                             src={src:?} dmg0={:?} alpha={}",
                            self.region,
                            dst.loc.x - self.region.loc.x,
                            dst.loc.y - self.region.loc.y,
                            damage.first(),
                            self.inner.alpha(),
                        )
                    });
            }
        }

        let radius = self
            .radius
            .min(self.region.size.w.min(self.region.size.h) as f32 / 2.0)
            .max(0.0);

        match self.inner.texture() {
            WaylandSurfaceTexture::Texture(texture) => {
                let primary = frame.as_mut();

                let uniforms = [
                    Uniform::new("u_size", [self.region.size.w as f32, self.region.size.h as f32]),
                    Uniform::new(
                        "u_origin",
                        [
                            (dst.loc.x - self.region.loc.x) as f32,
                            (dst.loc.y - self.region.loc.y) as f32,
                        ],
                    ),
                    Uniform::new("u_radius", radius),
                ];

                Ok(primary.render_texture_from_to(
                    texture,
                    src,
                    dst,
                    damage,
                    opaque_regions,
                    self.inner.transform(),
                    self.inner.alpha(),
                    Some(&self.program),
                    &uniforms,
                )?)
            }
            WaylandSurfaceTexture::SolidColor(color) => {
                Frame::draw_solid(frame, dst, damage, *color * self.inner.alpha())
            }
        }
    }
}

/// A rounded border ring (annulus) drawn with a custom mask shader.
///
pub struct RoundedBorderElement {
    id: Id,
    program: GlesTexProgram,
    texture: GlesTexture,
    region: Rectangle<i32, Physical>,
    border: f32,
    radius: f32,
    color: Color32F,
    commit: CommitCounter,
}

impl std::fmt::Debug for RoundedBorderElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoundedBorderElement")
            .field("id", &self.id)
            .field("region", &self.region)
            .field("border", &self.border)
            .field("radius", &self.radius)
            .field("color", &self.color)
            .finish()
    }
}

impl RoundedBorderElement {
    /// Create a border ring element. `region` is the outer box (window rect
    /// expanded by `border` on every side), `radius` the window corner radius.
    pub fn new(
        id: Id,
        program: GlesTexProgram,
        texture: GlesTexture,
        region: Rectangle<i32, Physical>,
        border: f32,
        radius: f32,
        color: Color32F,
        commit: CommitCounter,
    ) -> Self {
        Self {
            id,
            program,
            texture,
            region,
            border,
            radius,
            color,
            commit,
        }
    }
}

impl Element for RoundedBorderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::new(Point::from((0.0, 0.0)), (1.0, 1.0).into())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.region
    }
}

impl<'a> RenderElement<super::UdevRenderer<'a>> for RoundedBorderElement {
    fn draw(
        &self,
        frame: &mut <super::UdevRenderer<'a> as RendererSuper>::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), <super::UdevRenderer<'a> as RendererSuper>::Error> {
        if std::env::var("NOVA_DEBUG_GEOM").is_ok() {
            use std::io::Write as _;
            use std::sync::atomic::{AtomicU64, Ordering};
            static FRAME: AtomicU64 = AtomicU64::new(0);
            let frame_no = FRAME.fetch_add(1, Ordering::Relaxed);
            if frame_no % 120 == 0 {
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/nova_debug.log")
                    .and_then(|mut f| {
                        writeln!(
                            f,
                            "bord: frame={frame_no} dst={dst:?} region={:?} delta=(dloc={},dloc={}) \
                             b={} r={} col={:?} dmg0={:?}",
                            self.region,
                            dst.loc.x - self.region.loc.x,
                            dst.loc.y - self.region.loc.y,
                            self.border,
                            self.radius,
                            self.color,
                            damage.first(),
                        )
                    });
            }
        }
        let primary = frame.as_mut();

        let radius = self
            .radius
            .min(self.region.size.w.min(self.region.size.h) as f32 / 2.0)
            .max(0.0);
        let border = self
            .border
            .min(self.region.size.w.min(self.region.size.h) as f32 / 2.0)
            .max(0.0);

        let uniforms = [
            Uniform::new("u_size", [self.region.size.w as f32, self.region.size.h as f32]),
            Uniform::new("u_origin", [self.region.loc.x as f32, self.region.loc.y as f32]),
            Uniform::new("u_radius", radius),
            Uniform::new("u_border", border),
            Uniform::new("u_color", [self.color.r(), self.color.g(), self.color.b(), self.color.a()]),
        ];

        Ok(primary.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            1.0,
            Some(&self.program),
            &uniforms,
        )?)
    }
}

/// A full-screen background texture drawn behind every window on one output.
///
pub struct WallpaperElement {
    id: Id,
    texture: GlesTexture,
    region: Rectangle<i32, Physical>,
    commit: CommitCounter,
    alpha: f32,
}

impl WallpaperElement {
    /// Create a wallpaper element covering `region` with `texture`,
    /// drawn at the given `alpha` (used at 1.0 for the wallpaper and at
    pub fn new(
        id: Id,
        texture: GlesTexture,
        region: Rectangle<i32, Physical>,
        commit: CommitCounter,
        alpha: f32,
    ) -> Self {
        Self {
            id,
            texture,
            region,
            commit,
            alpha: alpha.clamp(0.0, 1.0),
        }
    }
}

impl Element for WallpaperElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(self.texture.size().to_f64())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.region
    }
}

impl<'a> RenderElement<super::UdevRenderer<'a>> for WallpaperElement {
    fn draw(&self,
        frame: &mut <super::UdevRenderer<'a> as RendererSuper>::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), <super::UdevRenderer<'a> as RendererSuper>::Error> {
        let primary = frame.as_mut();

        Ok(primary.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha,
            None,
            &[],
        )?)
    }
}

/// A cheap CPU box blur (sliding-window per line, three separable passes)
/// used to pre-blur the wallpaper once at startup.
pub fn blur_image(src: &image::RgbaImage, radius: u32) -> image::RgbaImage {
    let mut current = src.clone();
    for _ in 0..3 {
        current = box_blur_pass(&current, radius);
    }
    current
}

fn box_blur_pass(src: &image::RgbaImage, radius: u32) -> image::RgbaImage {
    let radius = radius.min(64) as usize;
    let (w, h) = src.dimensions();
    let (w, h) = (w as usize, h as usize);

    fn blur_line(src: &[u8], radius: usize) -> Vec<u8> {
        let n = src.len() / 4;
        let mut out = vec![0u8; n * 4];

        let mut prefix: Vec<[i64; 4]> = Vec::with_capacity(n + 1);
        prefix.push([0, 0, 0, 0]);
        for i in 0..n {
            let px = &src[i * 4..i * 4 + 4];
            let prev = prefix[i];
            prefix.push([
                prev[0] + px[0] as i64,
                prev[1] + px[1] as i64,
                prev[2] + px[2] as i64,
                prev[3] + px[3] as i64,
            ]);
        }

        for i in 0..n {
            let lo = i.saturating_sub(radius);
            let hi = (i + radius + 1).min(n);
            let count = (hi - lo) as i64;
            let sum = prefix[hi];
            let lo_sum = prefix[lo];
            for ch in 0..4 {
                let value = (sum[ch] - lo_sum[ch]) / count;
                out[i * 4 + ch] = value.clamp(0, 255) as u8;
            }
        }

        out
    }

    let raw = src.as_raw();
    let mut horizontal = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        horizontal.extend_from_slice(&blur_line(&raw[y * w * 4..(y + 1) * w * 4], radius));
    }

    let mut vertical = vec![0u8; w * h * 4];
    for x in 0..w {
        let mut column = Vec::with_capacity(h * 4);
        for y in 0..h {
            column.extend_from_slice(&horizontal[(y * w + x) * 4..(y * w + x) * 4 + 4]);
        }
        let blurred = blur_line(&column, radius);
        for (y, chunk) in blurred.chunks_exact(4).enumerate() {
            let offset = (y * w + x) * 4;
            vertical[offset..offset + 4].copy_from_slice(chunk);
        }
    }

    image::RgbaImage::from_raw(w as u32, h as u32, vertical).unwrap_or_else(|| src.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_preserves_dimensions_and_constant_color() {
        let (w, h) = (5u32, 6u32);
        let mut img = image::RgbaImage::new(w, h);
        for px in img.pixels_mut() {
            *px = image::Rgba([100, 150, 200, 255]);
        }
        let blurred = blur_image(&img, 8);
        assert_eq!(blurred.dimensions(), (w, h));
        for px in blurred.pixels() {
            assert_eq!(px.0, [100, 150, 200, 255]);
        }
    }

    #[test]
    fn blur_averages_distinct_regions() {
        let mut img = image::RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = if x < 32 { 0 } else { 255 };
                img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        let blurred = blur_image(&img, 8);
        let mid = blurred.get_pixel(31, 32).0[0];
        assert!(
            (100..160).contains(&mid),
            "expected mid-gray near the transition, got {mid}"
        );
    }
}

struct CursorRun {
    id: Id,
    x: i32,
    y: i32,
    width: i32,
    white: bool,
}

/// Small fallback arrow rendered without texture imports or GPU-specific
/// cursor planes. IDs remain stable so damage tracking can follow movement.
pub struct CursorSprite {
    runs: Vec<CursorRun>,
}

impl CursorSprite {
    pub fn new() -> Self {
        const ARROW: &[&str] = &[
            "#",
            "##",
            "#o#",
            "#oo#",
            "#ooo#",
            "#oooo#",
            "#ooooo#",
            "#oooooo#",
            "#ooooooo#",
            "#oooooooo#",
            "#ooooooooo#",
            "#oooooooooo#",
            "#ooooooooooo#",
            "#ooooooo######",
            "#oooo#oo#",
            "#ooo# #oo#",
            "#oo#  #oo#",
            "#o#    #oo#",
            "##     #oo#",
            "#       ##",
        ];

        let mut runs = Vec::new();

        for (y, row) in ARROW.iter().enumerate() {
            let bytes = row.as_bytes();
            let mut x = 0;

            while x < bytes.len() {
                if bytes[x] == b' ' {
                    x += 1;
                    continue;
                }

                let start = x;
                let color = bytes[x];

                while x < bytes.len() && bytes[x] == color {
                    x += 1;
                }

                runs.push(CursorRun {
                    id: Id::new(),
                    x: start as i32,
                    y: y as i32,
                    width: (x - start) as i32,
                    white: color == b'o',
                });
            }
        }

        Self { runs }
    }

    pub fn elements(
        &self,
        position: Point<f64, Logical>,
        output_geometry: Rectangle<i32, Logical>,
        output: &Output,
        cc: CommitCounter,
    ) -> Vec<SolidColorRenderElement> {
        let scale = output.current_scale().fractional_scale();

        let local_x = position.x - output_geometry.loc.x as f64;
        let local_y = position.y - output_geometry.loc.y as f64;

        if local_x >= output_geometry.size.w as f64
            || local_y >= output_geometry.size.h as f64
            || local_x + 16.0 <= 0.0
            || local_y + 24.0 <= 0.0
        {
            return Vec::new();
        }

        self.runs
            .iter()
            .map(|run| {
                let left = ((local_x + run.x as f64) * scale).round() as i32;
                let top = ((local_y + run.y as f64) * scale).round() as i32;
                let right = ((local_x + (run.x + run.width) as f64) * scale).round() as i32;
                let bottom = ((local_y + (run.y + 1) as f64) * scale).round() as i32;

                let rectangle = Rectangle::new(
                    (left, top).into(),
                    ((right - left).max(1), (bottom - top).max(1)).into(),
                );

                let color: [f32; 4] = if run.white {
                    [1.0, 1.0, 1.0, 1.0]
                } else {
                    [0.0, 0.0, 0.0, 1.0]
                };

                SolidColorRenderElement::new(
                    run.id.clone(),
                    rectangle,
                    cc,
                    color,
                    Kind::Unspecified,
                )
            })
            .collect()
    }
}

/// Configure physical output placement after discovery, before clients spawn.
///
pub fn configure_outputs(state: &mut Smallvil) -> Result<(), Box<dyn std::error::Error>> {
    let mut outputs: Vec<Output> = state.space.outputs().cloned().collect();

    let order: Vec<String> = std::env::var("NOVAWM_OUTPUT_ORDER")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();

    let mut positions: HashMap<String, (i32, i32)> = HashMap::new();

    if let Ok(value) = std::env::var("NOVAWM_OUTPUT_POSITIONS") {
        for entry in value.split(';').map(str::trim) {
            if entry.is_empty() {
                continue;
            }

            let parts: Vec<_> = entry.split(':').map(str::trim).collect();

            if parts.len() != 3 {
                return Err(std::io::Error::other(format!(
                    "invalid output position {entry:?}; use NAME:X:Y"
                ))
                .into());
            }

            positions.insert(parts[0].to_string(), (parts[1].parse()?, parts[2].parse()?));
        }
    }

    for mc in &state.config_watcher.config.monitor_configs {
        positions.entry(mc.name.clone()).or_insert((mc.x, mc.y));
    }

    for name in order.iter().chain(positions.keys()) {
        if !outputs.iter().any(|output| output.name() == *name) {
            return Err(std::io::Error::other(format!(
                "configured output {name:?} was not initialized; \
                 available outputs: {:?}",
                outputs
                    .iter()
                    .map(|output| output.name())
                    .collect::<Vec<_>>(),
            ))
            .into());
        }
    }

    outputs.sort_by_key(|output| {
        let name = output.name();

        (
            order
                .iter()
                .position(|entry| entry == &name)
                .unwrap_or(usize::MAX),
            name,
        )
    });

    let mut right_edge = 0;

    for output in &outputs {
        let Some(geometry) = state.space.output_geometry(output) else {
            continue;
        };

        let position: Point<i32, Logical> = positions
            .get(&output.name())
            .copied()
            .unwrap_or((right_edge, 0))
            .into();

        output.change_current_state(None, None, None, Some(position));

        state.space.map_output(output, position);

        right_edge = right_edge.max(position.x + geometry.size.w);

        crate::life(format!(
            "OUTPUT LAYOUT: {} position=({}, {}) size={}x{}",
            output.name(),
            position.x,
            position.y,
            geometry.size.w,
            geometry.size.h,
        ));
    }

    for first in 0..outputs.len() {
        for second in first + 1..outputs.len() {
            let Some(a) = state.space.output_geometry(&outputs[first]) else {
                continue;
            };
            let Some(b) = state.space.output_geometry(&outputs[second]) else {
                continue;
            };

            if a.overlaps(b) {
                crate::life(format!(
                    "OUTPUT LAYOUT WARNING: {} overlaps {}; \
                     adjust NOVAWM_OUTPUT_POSITIONS",
                    outputs[first].name(),
                    outputs[second].name(),
                ));
            }
        }
    }

    state.space.refresh();

    Ok(())
}


/// A decoded cursor-theme image: CPU pixels plus the hotspot, ready to
/// import once per render node.
pub struct CursorThemeImage {
    pub rgba: image::RgbaImage,
    pub xhot: u32,
    pub yhot: u32,
}

/// Load the configured theme, falling back through the conventional system
/// themes so a missing theme still yields a themed cursor when the system
pub fn load_cursor_image_with_fallback(theme: &str, size: u32) -> Option<(CursorThemeImage, String)> {
    let theme = theme.trim();
    if theme.is_empty() {
        return None;
    }
    let mut candidates = vec![theme.to_string()];
    for fallback in ["default", "Adwaita", "breeze_cursors", "DMZ-White"] {
        if fallback != theme {
            candidates.push(fallback.to_string());
        }
    }
    for candidate in &candidates {
        if let Some(image) = load_cursor_image(candidate, size) {
            return Some((image, candidate.clone()));
        }
    }
    crate::life(format!(
        "cursor: none of {candidates:?} found; using drawn arrow \
         (install a theme to ~/.icons or /usr/share/icons)"
    ));
    None
}

/// Load the `left_ptr` image from an XCursor theme at the size closest to
/// `size`, preferring the smallest image that covers it. Returns None when
pub fn load_cursor_image(theme: &str, size: u32) -> Option<CursorThemeImage> {
    if theme.trim().is_empty() {
        return None;
    }
    let path = xcursor::CursorTheme::load(theme).load_icon("left_ptr")?;
    let bytes = std::fs::read(&path).ok()?;
    let images = xcursor::parser::parse_xcursor(&bytes)?;
    let wanted = size.max(8);
    let best = images
        .iter()
        .filter(|img| img.width > 0 && img.height > 0)
        .min_by_key(|img| {
            let s = img.size.max(1);
            if s >= wanted {
                (0, s)
            } else {
                (1, u32::MAX - s)
            }
        })?;
    let rgba = image::RgbaImage::from_raw(best.width, best.height, best.pixels_rgba.clone())?;
    crate::life(format!(
        "cursor: theme {theme:?} left_ptr {} ({}x{} hotspot {},{} nominal {})",
        path.display(),
        best.width,
        best.height,
        best.xhot,
        best.yhot,
        best.size,
    ));
    Some(CursorThemeImage {
        rgba,
        xhot: best.xhot,
        yhot: best.yhot,
    })
}
