//! ext-image-capture-source + ext-image-copy-capture (portal / screencast).
//!

use smithay::{
    output::{Output, WeakOutput},
    reexports::wayland_server::protocol::{
        wl_pointer::WlPointer,
        wl_shm,
    },
    utils::{Buffer as BufferCoords, Size},
    wayland::{
        image_capture_source::{
            ImageCaptureSource, ImageCaptureSourceHandler,
            OutputCaptureSourceHandler, OutputCaptureSourceState,
        },
        image_copy_capture::{
            BufferConstraints, CaptureFailureReason, Frame, ImageCopyCaptureHandler,
            ImageCopyCaptureState, Session, SessionRef,
        },
    },
};

use crate::Smallvil;

/// Resolve the Output stored on a capture source (set in output_source_created).
pub fn output_from_source(source: &ImageCaptureSource) -> Option<Output> {
    source
        .user_data()
        .get::<WeakOutput>()
        .and_then(|weak| weak.upgrade())
}

impl ImageCaptureSourceHandler for Smallvil {
    fn source_destroyed(&mut self, _source: ImageCaptureSource) {}
}

impl OutputCaptureSourceHandler for Smallvil {
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
        &mut self.output_capture_source_state
    }

    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
        let weak = output.downgrade();
        source.user_data().insert_if_missing(|| weak);
        crate::life(format!(
            "capture: output source created for {}",
            output.name()
        ));
    }
}

impl ImageCopyCaptureHandler for Smallvil {
    fn image_copy_capture_state(&mut self) -> &mut ImageCopyCaptureState {
        &mut self.image_copy_capture_state
    }

    fn capture_constraints(&mut self, source: &ImageCaptureSource) -> Option<BufferConstraints> {
        if self.session_locked {
            return None;
        }
        let output = output_from_source(source)?;
        let geo = self.space.output_geometry(&output)?;
        let scale = output.current_scale().fractional_scale();
        let physical = geo.size.to_physical_precise_round(scale);
        if physical.w <= 0 || physical.h <= 0 {
            return None;
        }
        let size = Size::<i32, BufferCoords>::from((physical.w, physical.h));
        Some(BufferConstraints {
            size,
            shm: vec![
                wl_shm::Format::Argb8888,
                wl_shm::Format::Xrgb8888,
                wl_shm::Format::Abgr8888,
                wl_shm::Format::Xbgr8888,
            ],
            dma: None,
        })
    }

    fn cursor_capture_constraints(
        &mut self,
        _source: &ImageCaptureSource,
        _pointer: &WlPointer,
    ) -> Option<BufferConstraints> {
        None
    }

    fn new_session(&mut self, session: Session) {
        if let Some(constraints) = self.capture_constraints(&session.source()) {
            session.update_constraints(constraints);
        }
        crate::life("capture: new session");
        self.capture_sessions.push(session);
    }

    fn frame(&mut self, session: &SessionRef, frame: Frame) {
        if self.session_locked {
            crate::life("capture: frame rejected (session locked)");
            frame.fail(CaptureFailureReason::Unknown);
            return;
        }
        let Some(output) = output_from_source(&session.source()) else {
            crate::life("capture: frame rejected (no output on source)");
            frame.fail(CaptureFailureReason::Unknown);
            return;
        };
        let name = output.name();
        self.pending_capture_frames
            .retain(|(n, _)| n != &name);
        crate::life(format!("capture: frame queued for {name}"));
        self.pending_capture_frames.push((name, frame));
    }

    fn session_destroyed(&mut self, session: SessionRef) {
        self.capture_sessions.retain(|s| s != &session);
        crate::life("capture: session destroyed");
    }
}


/// Copy top-left-origin RGBA8 pixels into a client SHM buffer.
///
pub fn write_rgba_to_shm_buffer(
    buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<(), String> {
    use smithay::reexports::wayland_server::protocol::wl_shm;
    use smithay::wayland::shm::{with_buffer_contents_mut, BufferAccessError};

    with_buffer_contents_mut(buffer, |ptr, len, data| {
        let bpp = 4usize;
        let expected = (width as usize) * (height as usize) * bpp;
        if rgba.len() < expected {
            return Err(format!(
                "capture: rgba short {} < {expected}",
                rgba.len()
            ));
        }
        if data.width < width as i32 || data.height < height as i32 {
            return Err(format!(
                "capture: buffer {}x{} < frame {width}x{height}",
                data.width, data.height
            ));
        }
        let stride = data.stride as usize;
        let offset = data.offset as usize;
        if offset + (height as usize).saturating_sub(1) * stride + (width as usize) * bpp > len {
            return Err("capture: buffer mapping too small".into());
        }

        let swizzle_to_bgra = match data.format {
            wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 => true,
            wl_shm::Format::Abgr8888 | wl_shm::Format::Xbgr8888 => false,
            other => {
                return Err(format!("capture: unsupported shm format {other:?}"));
            }
        };

        unsafe {
            for row in 0..(height as usize) {
                let src_row = rgba.as_ptr().add(row * (width as usize) * bpp);
                let dst_row = ptr.add(offset + row * stride);
                if swizzle_to_bgra {
                    for x in 0..(width as usize) {
                        let s = src_row.add(x * 4);
                        let d = dst_row.add(x * 4);
                        *d = *s.add(2);
                        *d.add(1) = *s.add(1);
                        *d.add(2) = *s;
                        *d.add(3) = *s.add(3);
                    }
                } else {
                    std::ptr::copy_nonoverlapping(src_row, dst_row, (width as usize) * bpp);
                }
            }
        }
        Ok(())
    })
    .map_err(|e: BufferAccessError| format!("capture: shm access: {e:?}"))?
}
