//! Built-in screen recorder (`novactl record`).
//!

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use smithay::{
    backend::input::ButtonState,
    utils::{Physical, Size},
};

use crate::Smallvil;

/// Recording frame rate (v1: fixed). The DRM loop time-paces captures to
/// this, so odd refresh rates and VRR just work.
pub const RECORD_FPS: u32 = 30;
/// Minimum time between captured frames (= 1s / RECORD_FPS).
pub const FRAME_INTERVAL: Duration = Duration::from_millis(1000 / RECORD_FPS as u64);

/// Interactive monitor picker: click an output (or press its 1-based number)
/// to start recording it, Esc/right-click cancels.
pub struct RecordPicker {
    /// Output names in listed order (snapshot when the picker opened).
    pub outputs: Vec<String>,
    /// Output under the cursor / keyboard focus (highlighted frame).
    pub highlight: Option<String>,
}

/// An active recording: frames are re-composited in the DRM loop and
/// written as raw RGBA to ffmpeg's stdin.
pub struct RecordSession {
    pub output: String,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub last_capture: Instant,
    pub child: Child,
    pub stdin: Option<ChildStdin>,
    /// Drain thread for ffmpeg's stderr (kept so ffmpeg can never block on
    /// a full pipe, and so failures come with a reason).
    pub stderr: Option<JoinHandle<String>>,
}

/// `~/Videos/novawm-<millis>.mp4`, creating `~/Videos` on demand.
pub fn default_record_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let dir = PathBuf::from(home).join("Videos");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|age| age.as_millis())
        .unwrap_or(0);
    Ok(dir.join(format!("novawm-{millis}.mp4")))
}

/// Check ffmpeg exists and pick an encoder: `libx264` when available, plain
/// `mpeg4` otherwise (both fine inside mp4).
fn probe_encoder() -> Result<bool, String> {
    let probed = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map_err(|_| "ffmpeg not found: install ffmpeg to record".to_string())?;
    Ok(String::from_utf8_lossy(&probed.stdout).contains("libx264"))
}

/// Spawn ffmpeg consuming raw RGBA frames on stdin. The `scale` filter folds
/// odd widths/heights down to even (H.264 needs even for yuv420p) and is a
fn spawn_ffmpeg(
    path: &Path,
    width: u32,
    height: u32,
    x264: bool,
) -> Result<(Child, ChildStdin, JoinHandle<String>), String> {
    let size = format!("{width}x{height}");
    let fps = RECORD_FPS.to_string();
    let mut command = Command::new("ffmpeg");
    command.arg("-y").args([
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "-s",
        size.as_str(),
        "-framerate",
        fps.as_str(),
        "-i",
        "-",
    ]);
    if x264 {
        command.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "23"]);
    } else {
        command.args(["-c:v", "mpeg4", "-q:v", "3"]);
    }
    command
        .args([
            "-pix_fmt",
            "yuv420p",
            "-vf",
            "scale=trunc(iw/2)*2:trunc(ih/2)*2",
            "-movflags",
            "+faststart",
        ])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("ffmpeg spawn failed: {error}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "ffmpeg stdin unavailable".to_string())?;
    let stderr_pipe = child.stderr.take();
    let stderr = std::thread::spawn(move || {
        use std::io::Read;

        let mut text = String::new();
        if let Some(mut pipe) = stderr_pipe {
            let mut buf = [0u8; 4096];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        text.push_str(&String::from_utf8_lossy(&buf[..read]));
                        if text.len() > 4096 {
                            text = text
                                .chars()
                                .rev()
                                .take(2048)
                                .collect::<String>()
                                .chars()
                                .rev()
                                .collect();
                        }
                    }
                }
            }
        }
        text
    });
    Ok((child, stdin, stderr))
}

/// Last ~300 chars of ffmpeg's stderr, collapsed to one log line.
fn stderr_tail(handle: Option<JoinHandle<String>>) -> String {
    let text = handle
        .map(|joined| joined.join().unwrap_or_default())
        .unwrap_or_default();
    let tail: String = text
        .chars()
        .rev()
        .take(300)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    tail.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Smallvil {
    pub fn ipc_record(&mut self, args: &mut Vec<String>) -> String {
        let mut parts = args.iter().map(String::as_str);
        match parts.next() {
            None => self
                .record_open_picker()
                .map_or_else(|error| format!("E {error}\n"), |ok| ok),
            Some("stop") => self
                .record_stop()
                .map_or_else(|error| format!("E {error}\n"), |ok| ok),
            Some("cancel") => {
                if self.record_picker.take().is_some() {
                    crate::life("recording: picker cancelled via ipc");
                    "ok cancelled\n".to_string()
                } else {
                    "E not picking\n".to_string()
                }
            }
            Some("status") => self.record_status(),
            Some("toggle") => self.record_toggle_ipc(),
            Some(output) => {
                let path = parts.next().map(PathBuf::from);
                self.record_start(output, path)
                    .map_or_else(|error| format!("E {error}\n"), |ok| ok)
            }
        }
    }

    /// One-line recorder state for `record status`.
    pub fn record_status(&self) -> String {
        if let Some(session) = &self.record_session {
            format!(
                "recording {} {} {}x{} {}fps {}f\n",
                session.output,
                session.path.display(),
                session.width,
                session.height,
                RECORD_FPS,
                session.frames,
            )
        } else if let Some(picker) = &self.record_picker {
            format!(
                "picking {} outputs (click one or press 1-{}, Esc cancels)\n",
                picker.outputs.len(),
                picker.outputs.len(),
            )
        } else {
            "idle\n".to_string()
        }
    }

    /// Open the interactive monitor picker (shared by IPC and the keybind).
    pub fn record_open_picker(&mut self) -> Result<String, String> {
        if self.drm.is_none() {
            return Err("record needs the drm backend".to_string());
        }
        if let Some(session) = &self.record_session {
            return Err(format!(
                "already recording {} (record stop to finish)",
                session.output,
            ));
        }
        if self.screenshot_select.is_some() {
            return Err("busy (screenshot select active)".to_string());
        }
        if self.record_picker.is_some() {
            return Err("already picking (record cancel to leave)".to_string());
        }
        let outputs: Vec<String> = self.space.outputs().map(|o| o.name()).collect();
        if outputs.is_empty() {
            return Err("no outputs".to_string());
        }
        let list = outputs
            .iter()
            .enumerate()
            .map(|(index, name)| format!("[{}] {name}", index + 1))
            .collect::<Vec<_>>()
            .join(" ");
        crate::life(format!("recording: picker opened ({list})"));
        self.record_picker = Some(RecordPicker {
            outputs,
            highlight: None,
        });
        Ok(format!(
            "ok picking (click an output or press its number, Esc cancels): {list}\n",
        ))
    }

    /// Start recording `output_name` to `path` (default when None).
    pub fn record_start(
        &mut self,
        output_name: &str,
        path: Option<PathBuf>,
    ) -> Result<String, String> {
        if self.drm.is_none() {
            return Err("record needs the drm backend".to_string());
        }
        if let Some(session) = &self.record_session {
            return Err(format!(
                "already recording {} (record stop to finish)",
                session.output,
            ));
        }
        let (width, height) = {
            let Some(output) = self.space.outputs().find(|o| o.name() == output_name) else {
                let known: Vec<String> = self.space.outputs().map(|o| o.name()).collect();
                return Err(format!(
                    "unknown output {output_name} (outputs: {})",
                    known.join(", "),
                ));
            };
            let scale = output.current_scale().fractional_scale();
            let Some(geo) = self.space.output_geometry(output) else {
                return Err(format!("output {output_name} has no geometry"));
            };
            let size: Size<i32, Physical> = geo.size.to_physical_precise_round(scale);
            if size.w <= 0 || size.h <= 0 {
                return Err(format!("output {output_name} has no size"));
            }
            (size.w as u32, size.h as u32)
        };
        let path = match path {
            Some(explicit) => explicit,
            None => default_record_path()?,
        };
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
        }
        let x264 = probe_encoder()?;
        let (child, stdin, stderr) = spawn_ffmpeg(&path, width, height, x264)?;
        self.record_picker = None;
        self.record_session = Some(RecordSession {
            output: output_name.to_string(),
            path: path.clone(),
            width,
            height,
            frames: 0,
            last_capture: Instant::now(),
            child,
            stdin: Some(stdin),
            stderr: Some(stderr),
        });
        crate::life(format!(
            "recording: started {output_name} {width}x{height} -> {}",
            path.display(),
        ));
        Ok(format!(
            "ok recording {output_name} {}\n",
            path.display(),
        ))
    }

    /// Stop the active recording, finalizing the file (blocks briefly while
    /// ffmpeg writes the trailer).
    pub fn record_stop(&mut self) -> Result<String, String> {
        let mut session = self
            .record_session
            .take()
            .ok_or_else(|| "not recording".to_string())?;
        drop(session.stdin.take());
        let status = session
            .child
            .wait()
            .map_err(|error| format!("ffmpeg wait failed: {error}"))?;
        let tail = stderr_tail(session.stderr.take());
        if status.success() {
            crate::life(format!(
                "recording: stopped {} -> {} ({} frames)",
                session.output,
                session.path.display(),
                session.frames,
            ));
            Ok(format!(
                "ok saved {} ({} frames)\n",
                session.path.display(),
                session.frames,
            ))
        } else {
            crate::life(format!(
                "recording: ffmpeg failed ({status:?}): {tail}"
            ));
            Err(format!("ffmpeg failed ({status:?}): {tail}"))
        }
    }

    /// Abort the session after a mid-recording failure (broken ffmpeg pipe,
    /// output resize): reap the child and log the reason. The partial file
    pub fn record_abort(&mut self, reason: &str) {
        let Some(mut session) = self.record_session.take() else {
            return;
        };
        drop(session.stdin.take());
        let _ = session.child.kill();
        let _ = session.child.wait();
        let tail = stderr_tail(session.stderr.take());
        let mut message = format!(
            "recording: aborted {} after {} frames ({reason})",
            session.output, session.frames,
        );
        if !tail.is_empty() {
            message.push_str(": ");
            message.push_str(&tail);
        }
        crate::life(message);
    }

    /// Write one captured frame to ffmpeg (called from the DRM loop for the
    /// recorded output). Returns false when the session died and the caller
    pub fn record_push_frame(&mut self, rgba: &[u8], width: u32, height: u32) -> bool {
        let size_ok = self
            .record_session
            .as_ref()
            .is_some_and(|session| session.width == width && session.height == height);
        if !size_ok {
            self.record_abort("output resized");
            return false;
        }
        let written = self
            .record_session
            .as_mut()
            .and_then(|session| session.stdin.as_mut())
            .map(|stdin| stdin.write_all(rgba));
        match written {
            Some(Ok(())) => {
                if let Some(session) = self.record_session.as_mut() {
                    session.frames += 1;
                    session.last_capture = Instant::now();
                }
                true
            }
            _ => {
                self.record_abort("ffmpeg pipe failed");
                false
            }
        }
    }

    /// Keybind toggle: stop when recording, cancel when picking, otherwise
    /// open the picker.
    pub fn record_toggle(&mut self) {
        if self.record_session.is_some() {
            match self.record_stop() {
                Ok(done) => crate::life(format!("recording: toggle stop: {}", done.trim())),
                Err(error) => crate::life(format!("recording: toggle stop failed: {error}")),
            }
            return;
        }
        if self.record_picker.take().is_some() {
            crate::life("recording: picker cancelled via keybind");
            return;
        }
        match self.record_open_picker() {
            Ok(opened) => crate::life(format!("recording: toggle: {}", opened.trim())),
            Err(error) => crate::life(format!("recording: toggle: {error}")),
        }
    }

    /// Same toggle for `novactl record toggle` (with an IPC reply).
    pub fn record_toggle_ipc(&mut self) -> String {
        if self.record_session.is_some() {
            return self
                .record_stop()
                .map_or_else(|error| format!("E {error}\n"), |ok| ok);
        }
        if self.record_picker.take().is_some() {
            crate::life("recording: picker cancelled via ipc");
            return "ok cancelled\n".to_string();
        }
        self.record_open_picker()
            .map_or_else(|error| format!("E {error}\n"), |ok| ok)
    }

    /// Number-key selection while the picker is open (1-based).
    pub fn record_digit(&mut self, digit: u8) {
        let name = self.record_picker.as_ref().and_then(|picker| {
            picker
                .outputs
                .get(digit.saturating_sub(1) as usize)
                .cloned()
        });
        let Some(name) = name else {
            crate::life(format!("recording: no output #{digit}"));
            return;
        };
        self.record_confirm(&name);
    }

    /// Arrow-key highlight motion while the picker is open (wraps around).
    pub fn record_move(&mut self, direction: i8) {
        let Some(picker) = self.record_picker.as_mut() else {
            return;
        };
        if picker.outputs.is_empty() {
            return;
        }
        let len = picker.outputs.len();
        let current = picker
            .highlight
            .as_ref()
            .and_then(|hot| picker.outputs.iter().position(|o| o == hot))
            .unwrap_or(if direction > 0 { len - 1 } else { 0 });
        let next = (current as i32 + direction as i32).rem_euclid(len as i32) as usize;
        picker.highlight = Some(picker.outputs[next].clone());
    }

    /// Enter/Space: confirm the highlighted output (first when none).
    pub fn record_enter(&mut self) {
        let name = self.record_picker.as_ref().and_then(|picker| {
            picker
                .highlight
                .clone()
                .or_else(|| picker.outputs.first().cloned())
        });
        let Some(name) = name else {
            return;
        };
        self.record_confirm(&name);
    }

    /// Leave the picker without recording.
    pub fn record_cancel(&mut self) {
        if self.record_picker.take().is_some() {
            crate::life("recording: picker cancelled");
        }
    }

    /// Confirm one output: close the picker and start recording it.
    /// Failures (no ffmpeg, unknown output) are logged; direct IPC starts
    pub fn record_confirm(&mut self, output_name: &str) {
        self.record_picker = None;
        if let Err(error) = self.record_start(output_name, None) {
            crate::life(format!("recording: picker confirm failed: {error}"));
        }
    }

    /// Pointer-button input while the picker is open (swallowed: never
    /// forwarded to clients). Left click confirms the output under the
    pub fn record_button(&mut self, button: u32, state: ButtonState) {
        const BTN_LEFT: u32 = 272;
        const BTN_RIGHT: u32 = 273;
        if state != ButtonState::Pressed {
            return;
        }
        if button == BTN_RIGHT {
            self.record_cancel();
            return;
        }
        if button != BTN_LEFT {
            return;
        }
        let name = self
            .output_under(Some(self.cursor_position))
            .map(|output| output.name());
        let Some(name) = name else {
            crate::life("recording: click outside any output");
            return;
        };
        let known = self.record_picker.as_ref().is_some_and(|picker| {
            picker.outputs.iter().any(|listed| listed == &name)
        });
        if !known {
            crate::life(format!("recording: {name} not in picker list"));
            return;
        }
        self.record_confirm(&name);
    }

    /// Pointer motion while the picker is open: highlight the output under
    /// the cursor. Returns true (swallowed) while picking.
    pub fn record_motion(&mut self) -> bool {
        if self.record_picker.is_none() {
            return false;
        }
        let name = self
            .output_under(Some(self.cursor_position))
            .map(|output| output.name());
        if let Some(picker) = self.record_picker.as_mut() {
            picker.highlight = name;
        }
        true
    }
}
