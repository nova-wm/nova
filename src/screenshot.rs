//! Built-in screenshot utility (`novactl screenshot`).
//!

use std::path::PathBuf;

use smithay::{
    backend::input::ButtonState,
    utils::{Logical, Physical, Point, Rectangle, Size},
};

use crate::Smallvil;

/// Interactive area-select state: the output the gesture is pinned to, the
/// drag anchor/current points (global logical), whether the left button is
#[derive(Debug, Clone)]
pub struct ScreenshotSelect {
    pub output: String,
    pub anchor: Point<f64, Logical>,
    pub current: Point<f64, Logical>,
    pub dragging: bool,
    pub dest: ScreenshotDest,
}

/// A capture armed for the next frame of `output`: `rect` is the crop in
/// output-local physical pixels (`None` = whole output).
#[derive(Debug, Clone)]
pub struct ScreenshotCapture {
    pub output: String,
    pub rect: Option<Rectangle<i32, Physical>>,
    pub dest: ScreenshotDest,
}

/// Where finished PNG bytes go.
#[derive(Debug, Clone)]
pub enum ScreenshotDest {
    Copy,
    Save(PathBuf),
}

/// Default save path: `~/Pictures/novawm-<unix-millis>.png`, creating
/// `~/Pictures` when missing.
pub fn default_save_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "HOME is not set".to_string())?;
    let dir = home.join("Pictures");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(dir.join(format!("novawm-{millis}.png")))
}

/// Encode RGBA bytes (top row first) as a PNG image.
pub fn encode_png_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Vec<u8>, String> {
    let image = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| format!("screenshot: bad pixel buffer {width}x{height}"))?;
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|error| format!("screenshot: png encode failed: {error}"))?;
    Ok(cursor.into_inner())
}

impl Smallvil {
    /// `screenshot <area|monitor> <copy|save> [path]` (+ `screenshot cancel`).
    pub fn ipc_screenshot(&mut self, args: &mut Vec<String>) -> String {
        const USAGE: &str =
            "E usage: screenshot <area|monitor> <copy|save> [path] | screenshot cancel\n";
        let mut parts = args.iter().map(String::as_str);
        match parts.next() {
            Some("cancel") => {
                self.screenshot_select = None;
                self.screenshot_capture = None;
                crate::life("screenshot: cancelled via ipc");
                "ok\n".to_string()
            }
            Some(kind @ ("area" | "monitor")) => {
                if self.drm.is_none() {
                    return "E screenshots need the drm backend\n".to_string();
                }
                if self.screenshot_select.is_some() || self.screenshot_capture.is_some() {
                    return "E busy\n".to_string();
                }
                if self.record_picker.is_some() {
                    return "E busy (record picker active; record cancel to leave)\n".to_string();
                }
                let dest = match parts.next() {
                    Some("copy") => ScreenshotDest::Copy,
                    Some("save") => match parts.next() {
                        Some(path) => ScreenshotDest::Save(PathBuf::from(path)),
                        None => match default_save_path() {
                            Ok(path) => ScreenshotDest::Save(path),
                            Err(error) => return format!("E {error}\n"),
                        },
                    },
                    _ => return USAGE.to_string(),
                };
                let Some(output) = self.output_under(Some(self.cursor_position)) else {
                    return "E no output under cursor\n".to_string();
                };
                let name = output.name();
                if kind == "monitor" {
                    self.screenshot_capture = Some(ScreenshotCapture {
                        output: name.clone(),
                        rect: None,
                        dest: dest.clone(),
                    });
                    crate::life(format!("screenshot: monitor armed on {name} dest={dest:?}"));
                    match dest {
                        ScreenshotDest::Save(path) => {
                            format!("ok {}\n", path.display())
                        }
                        ScreenshotDest::Copy => "ok\n".to_string(),
                    }
                } else {
                    let at = self.screenshot_clamp(&name, self.cursor_position);
                    self.screenshot_select = Some(ScreenshotSelect {
                        output: name.clone(),
                        anchor: at,
                        current: at,
                        dragging: false,
                        dest: dest.clone(),
                    });
                    crate::life(format!("screenshot: select started on {name}"));
                    match dest {
                        ScreenshotDest::Save(path) => {
                            format!("ok {}\n", path.display())
                        }
                        ScreenshotDest::Copy => "ok\n".to_string(),
                    }
                }
            }
            _ => USAGE.to_string(),
        }
    }

    /// Clamp a global-logical point into the named output's geometry (area
    /// select is pinned to its starting output).
    pub fn screenshot_clamp(
        &self,
        output_name: &str,
        point: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        let Some(output) = self.space.outputs().find(|o| o.name() == output_name) else {
            return point;
        };
        let Some(geo) = self.space.output_geometry(output) else {
            return point;
        };
        let left = geo.loc.x as f64;
        let top = geo.loc.y as f64;
        let right = left + geo.size.w as f64 - 0.001;
        let bottom = top + geo.size.h as f64 - 0.001;
        Point::from((point.x.clamp(left, right), point.y.clamp(top, bottom)))
    }

    /// Pointer-button input while select mode is active (swallowed: never
    /// forwarded to clients). Left-drag draws the rect (release captures),
    pub fn screenshot_button(&mut self, button: u32, state: ButtonState) {
        const BTN_LEFT: u32 = 272;
        const BTN_RIGHT: u32 = 273;

        if button == BTN_RIGHT && state == ButtonState::Pressed {
            self.screenshot_cancel();
            return;
        }
        if button != BTN_LEFT {
            return;
        }
        let output = self
            .screenshot_select
            .as_ref()
            .map(|sel| sel.output.clone());
        let Some(output) = output else {
            return;
        };

        if state == ButtonState::Pressed {
            let at = self.screenshot_clamp(&output, self.cursor_position);
            if let Some(sel) = self.screenshot_select.as_mut() {
                sel.anchor = at;
                sel.current = at;
                sel.dragging = true;
            }
            crate::life(format!(
                "screenshot: drag started at ({:.0},{:.0})",
                at.x, at.y,
            ));
        } else {
            let finished = self
                .screenshot_select
                .as_ref()
                .map(|sel| {
                    let w = (sel.anchor.x - sel.current.x).abs();
                    let h = (sel.anchor.y - sel.current.y).abs();
                    w >= 2.0 && h >= 2.0
                })
                .unwrap_or(false);
            if let Some(sel) = self.screenshot_select.as_mut() {
                sel.dragging = false;
            }
            if finished {
                crate::life("screenshot: drag released, confirming");
                self.screenshot_confirm();
            } else {
                crate::life("screenshot: click without drag, awaiting Space/Enter");
            }
        }
    }

    /// Pointer motion while select mode is active: track the drag point
    /// (clamped to the select output). Returns true when select mode owns
    pub fn screenshot_motion(&mut self) -> bool {
        let output = self
            .screenshot_select
            .as_ref()
            .map(|sel| sel.output.clone());
        let Some(output) = output else {
            return false;
        };
        let at = self.screenshot_clamp(&output, self.cursor_position);
        if let Some(sel) = self.screenshot_select.as_mut() {
            sel.current = at;
        }
        true
    }

    /// Confirm the selection: arm a capture (full output when the rect is
    /// smaller than 2 logical pixels) and leave select mode, so the overlay
    pub fn screenshot_confirm(&mut self) {
        let Some(sel) = self.screenshot_select.take() else {
            return;
        };
        let Some(output) = self
            .space
            .outputs()
            .find(|o| o.name() == sel.output)
            .cloned()
        else {
            crate::life("screenshot: select output gone, dropped");
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            crate::life("screenshot: select output has no geometry, dropped");
            return;
        };
        let scale = output.current_scale().fractional_scale();
        let x0 = sel.anchor.x.min(sel.current.x);
        let x1 = sel.anchor.x.max(sel.current.x);
        let y0 = sel.anchor.y.min(sel.current.y);
        let y1 = sel.anchor.y.max(sel.current.y);
        let rect = if x1 - x0 >= 2.0 && y1 - y0 >= 2.0 {
            let px0 = ((x0 - geo.loc.x as f64) * scale).round() as i32;
            let py0 = ((y0 - geo.loc.y as f64) * scale).round() as i32;
            let px1 = ((x1 - geo.loc.x as f64) * scale).round() as i32;
            let py1 = ((y1 - geo.loc.y as f64) * scale).round() as i32;
            let size: Size<i32, Physical> = geo.size.to_physical_precise_round(scale);
            let cx0 = px0.clamp(0, size.w);
            let cy0 = py0.clamp(0, size.h);
            let cx1 = px1.clamp(0, size.w);
            let cy1 = py1.clamp(0, size.h);
            if cx1 > cx0 && cy1 > cy0 {
                Some(Rectangle::<i32, Physical>::new(
                    (cx0, cy0).into(),
                    (cx1 - cx0, cy1 - cy0).into(),
                ))
            } else {
                None
            }
        } else {
            None
        };
        crate::life(format!(
            "screenshot: confirmed on {} rect={rect:?} dest={:?}",
            sel.output, sel.dest,
        ));
        self.screenshot_capture = Some(ScreenshotCapture {
            output: sel.output,
            rect,
            dest: sel.dest,
        });
    }

    /// Leave select mode without capturing.
    pub fn screenshot_cancel(&mut self) {
        if self.screenshot_select.take().is_some() {
            crate::life("screenshot: select cancelled");
        }
    }
}
