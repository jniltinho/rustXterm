//! Remote-desktop (RDP/VNC) contract (design D13): protocol crates emit [`DesktopEvent`]s,
//! the engine applies them to a per-tab [`Framebuffer`], the UI uploads only dirty regions.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Largest accepted desktop edge, in pixels.
pub const MAX_EDGE: u32 = 8192;
/// Largest accepted remote cursor edge, in pixels.
pub const MAX_CURSOR_EDGE: u32 = 256;

/// Pointer shape set by the server; the UI draws it at the local pointer (no round trip).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RemoteCursor {
    /// The local OS arrow.
    #[default]
    Default,
    Hidden,
    /// Straight-alpha RGBA8 image (`width * height * 4` bytes) with its hotspot.
    Bitmap {
        width: u32,
        height: u32,
        hot_x: u32,
        hot_y: u32,
        rgba: Vec<u8>,
    },
}

/// Pixel rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

/// Output of an RDP/VNC session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopEvent {
    /// Desktop size (re)negotiated; the framebuffer is cleared to black.
    Resized {
        width: u32,
        height: u32,
    },
    /// Decoded RGBA8 pixels for `rect` (`rect.area() * 4` bytes, row-major).
    Update {
        rect: Rect,
        rgba: Vec<u8>,
    },
    /// Copy pixels inside the framebuffer (VNC CopyRect).
    Copy {
        src_x: u32,
        src_y: u32,
        dst: Rect,
    },
    /// Remote clipboard text (only when clipboard sharing is enabled).
    Clipboard(String),
    Bell,
    /// Remote pointer shape changed.
    Cursor(RemoteCursor),
}

/// Input sent to an RDP/VNC session. Keys are X11 keysyms (the RDP crate maps them to scancodes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesktopInput {
    Key {
        keysym: u32,
        down: bool,
    },
    /// Pointer position in desktop pixels; `buttons` bit 0 left, 1 middle, 2 right.
    Pointer {
        x: u32,
        y: u32,
        buttons: u8,
    },
    /// Wheel steps (positive = up / right).
    Wheel {
        x: u32,
        y: u32,
        dx: i16,
        dy: i16,
    },
    Clipboard(String),
    /// Release every pressed key and button (keyboard-grab release).
    ReleaseAll,
    /// Tab shown/hidden: hidden sessions stop requesting updates.
    Visible(bool),
    /// Ask the server for a new desktop size (RDP display control).
    Resize {
        width: u32,
        height: u32,
    },
}

/// One tab's desktop pixels plus dirty regions not yet uploaded by the UI.
#[derive(Debug, Clone, Default)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    /// RGBA8, `width * height * 4` bytes.
    pub pixels: Vec<u8>,
    pub dirty: Vec<Rect>,
    pub cursor: RemoteCursor,
    /// `cursor` changed since the UI last took it.
    pub cursor_dirty: bool,
}

impl Framebuffer {
    /// Apply an event; rejects sizes over [`MAX_EDGE`] and out-of-bounds or short updates.
    /// Clipboard/Bell are ignored here.
    pub fn apply(&mut self, event: &DesktopEvent) -> Result<()> {
        match event {
            DesktopEvent::Resized { width, height } => {
                if *width == 0 || *height == 0 || *width > MAX_EDGE || *height > MAX_EDGE {
                    return Err(Error::Protocol(format!("desktop size {width}x{height} not allowed")));
                }
                self.width = *width;
                self.height = *height;
                self.pixels = vec![0; *width as usize * *height as usize * 4];
                self.pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 0xff);
                self.dirty = vec![Rect { x: 0, y: 0, width: *width, height: *height }];
            }
            DesktopEvent::Update { rect, rgba } => {
                self.check(rect)?;
                if rgba.len() as u64 != rect.area() * 4 {
                    return Err(Error::Protocol("update length does not match its rectangle".into()));
                }
                let row = rect.width as usize * 4;
                for (i, src) in rgba.chunks_exact(row).enumerate() {
                    let at = ((rect.y as usize + i) * self.width as usize + rect.x as usize) * 4;
                    self.pixels[at..at + row].copy_from_slice(src);
                }
                self.dirty.push(*rect);
            }
            DesktopEvent::Copy { src_x, src_y, dst } => {
                self.check(dst)?;
                self.check(&Rect { x: *src_x, y: *src_y, ..*dst })?;
                let row = dst.width as usize * 4;
                let w = self.width as usize;
                let rows: Vec<Vec<u8>> = (0..dst.height as usize)
                    .map(|i| {
                        let at = ((*src_y as usize + i) * w + *src_x as usize) * 4;
                        self.pixels[at..at + row].to_vec()
                    })
                    .collect();
                for (i, src) in rows.iter().enumerate() {
                    let at = ((dst.y as usize + i) * w + dst.x as usize) * 4;
                    self.pixels[at..at + row].copy_from_slice(src);
                }
                self.dirty.push(*dst);
            }
            DesktopEvent::Cursor(cursor) => {
                if let RemoteCursor::Bitmap { width, height, hot_x, hot_y, rgba } = cursor {
                    let edge = 1..=MAX_CURSOR_EDGE;
                    if !edge.contains(width) || !edge.contains(height) || hot_x >= width || hot_y >= height {
                        return Err(Error::Protocol(format!("cursor {width}x{height} not allowed")));
                    }
                    if rgba.len() as u64 != u64::from(*width) * u64::from(*height) * 4 {
                        return Err(Error::Protocol("cursor length does not match its size".into()));
                    }
                }
                self.cursor = cursor.clone();
                self.cursor_dirty = true;
            }
            DesktopEvent::Clipboard(_) | DesktopEvent::Bell => {}
        }
        Ok(())
    }

    fn check(&self, r: &Rect) -> Result<()> {
        let fits = u64::from(r.x) + u64::from(r.width) <= u64::from(self.width)
            && u64::from(r.y) + u64::from(r.height) <= u64::from(self.height);
        if fits { Ok(()) } else { Err(Error::Protocol(format!("rectangle {r:?} outside the desktop"))) }
    }

    /// Take the dirty regions, clearing them.
    pub fn take_dirty(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.dirty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_desktop() {
        let mut fb = Framebuffer::default();
        assert!(fb.apply(&DesktopEvent::Resized { width: MAX_EDGE + 1, height: 10 }).is_err());
        assert!(fb.apply(&DesktopEvent::Resized { width: 0, height: 10 }).is_err());
    }

    #[test]
    fn update_and_copy_track_dirty_and_bounds() {
        let mut fb = Framebuffer::default();
        fb.apply(&DesktopEvent::Resized { width: 4, height: 4 }).unwrap();
        assert_eq!(fb.take_dirty().len(), 1);
        let rect = Rect { x: 1, y: 1, width: 2, height: 1 };
        fb.apply(&DesktopEvent::Update { rect, rgba: vec![9; 8] }).unwrap();
        assert_eq!(&fb.pixels[(4 + 1) * 4..(4 + 3) * 4], &[9; 8]);
        fb.apply(&DesktopEvent::Copy { src_x: 1, src_y: 1, dst: Rect { x: 0, y: 3, width: 2, height: 1 } }).unwrap();
        assert_eq!(&fb.pixels[12 * 4..14 * 4], &[9; 8]);
        assert_eq!(fb.take_dirty().len(), 2);
        let outside = Rect { x: 3, y: 0, width: 2, height: 1 };
        assert!(fb.apply(&DesktopEvent::Update { rect: outside, rgba: vec![0; 8] }).is_err());
        assert!(fb.apply(&DesktopEvent::Update { rect, rgba: vec![0; 7] }).is_err());
    }

    #[test]
    fn cursor_apply_and_limits() {
        let bitmap = |width: u32, height: u32, hot_x: u32, len: usize| {
            DesktopEvent::Cursor(RemoteCursor::Bitmap { width, height, hot_x, hot_y: 0, rgba: vec![1; len] })
        };
        let mut fb = Framebuffer::default();
        assert_eq!(fb.cursor, RemoteCursor::Default);
        fb.apply(&bitmap(2, 3, 1, 24)).unwrap();
        assert!(fb.cursor_dirty);
        assert!(matches!(fb.cursor, RemoteCursor::Bitmap { width: 2, height: 3, .. }));
        fb.cursor_dirty = false;
        fb.apply(&DesktopEvent::Cursor(RemoteCursor::Hidden)).unwrap();
        assert_eq!((fb.cursor.clone(), fb.cursor_dirty), (RemoteCursor::Hidden, true));
        let big = MAX_CURSOR_EDGE + 1;
        assert!(fb.apply(&bitmap(big, 1, 0, big as usize * 4)).is_err(), "too wide");
        assert!(fb.apply(&bitmap(0, 1, 0, 0)).is_err(), "empty");
        assert!(fb.apply(&bitmap(2, 2, 2, 16)).is_err(), "hotspot outside");
        assert!(fb.apply(&bitmap(2, 2, 0, 15)).is_err(), "short data");
        assert_eq!(fb.cursor, RemoteCursor::Hidden, "rejected shapes leave the cursor alone");
    }
}
