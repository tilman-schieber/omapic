//! Thumbnails that already exist somewhere, so nothing has to be decoded at
//! full size: the freedesktop thumbnail cache (read, never written) and the
//! preview embedded in a JPEG's EXIF block.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use gtk::gdk;
use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf, PixbufRotation};
use gtk::gio::prelude::*;
use gtk::glib;
use gtk::prelude::*;

use crate::exif::{self, Exif};

pub struct Quick {
    pub texture: gdk::Texture,
    /// Good enough to keep; otherwise a placeholder until the real decode.
    pub sharp: bool,
}

pub fn find(path: &Path, target: i32) -> Option<Quick> {
    cached(path, target).or_else(|| embedded(path, target))
}

// ── freedesktop thumbnail cache ─────────────────────────────────────────

const CACHE_SIZES: &[(&str, i32)] = &[("normal", 128), ("large", 256), ("x-large", 512), ("xx-large", 1024)];

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".cache")))?;
    Some(base.join("thumbnails"))
}

/// Flavours to try for a `target` sized cell: the smallest that is big
/// enough first, then the smaller ones from the top down.
fn size_preference(target: i32) -> Vec<(&'static str, i32)> {
    let (mut enough, mut small): (Vec<_>, Vec<_>) = CACHE_SIZES.iter().partition(|(_, s)| *s >= target);
    small.reverse();
    enough.append(&mut small);
    enough
}

fn cached(path: &Path, target: i32) -> Option<Quick> {
    let dir = cache_dir()?;
    let uri = gtk::gio::File::for_path(path).uri();
    let name = format!("{}.png", glib::compute_checksum_for_string(glib::ChecksumType::Md5, &uri)?);
    let mtime = path.metadata().ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    for (flavour, size) in size_preference(target) {
        let file = dir.join(flavour).join(&name);
        let Ok(mut head) = std::fs::File::open(&file) else { continue };
        let mut start = vec![0; 4096];
        let read = head.read(&mut start).unwrap_or(0);
        if png_text(&start[..read], "Thumb::MTime").and_then(|t| t.parse().ok()) != Some(mtime) {
            continue; // stale: the image changed after it was thumbnailed
        }
        let Ok(texture) = gdk::Texture::from_filename(&file) else { continue };
        let texture = adjust(&texture, 1, target).unwrap_or(texture);
        return Some(Quick { texture, sharp: size * 4 >= target * 3 });
    }
    None
}

/// Value of a `tEXt` chunk, looking no further than the first image data.
fn png_text(png: &[u8], key: &str) -> Option<String> {
    let mut rest = png.strip_prefix(b"\x89PNG\r\n\x1a\n")?;
    loop {
        let length = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        let kind = rest.get(4..8)?;
        if kind == b"IDAT" {
            return None;
        }
        if kind == b"tEXt" {
            let data = rest.get(8..8 + length)?;
            let (name, value) = data.split_at(data.iter().position(|&b| b == 0)?);
            if name == key.as_bytes() {
                return Some(String::from_utf8_lossy(&value[1..]).into_owned());
            }
        }
        rest = rest.get(12 + length..)?; // length, type, data, crc
    }
}

// ── EXIF preview ────────────────────────────────────────────────────────

fn embedded(path: &Path, target: i32) -> Option<Quick> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if extension != "jpg" && extension != "jpeg" {
        return None;
    }
    // APP1 sits at the very start and cannot exceed 64 KiB.
    let mut start = Vec::new();
    std::fs::File::open(path).ok()?.take(128 * 1024).read_to_end(&mut start).ok()?;
    let exif = Exif::find(&start)?;
    let orientation = exif.orientation().map_or(1, |(_, _, value)| value);
    let bytes = glib::Bytes::from(&start[exif.thumbnail()?]);
    let texture = gdk::Texture::from_bytes(&bytes).ok()?;
    let texture = adjust(&texture, orientation, target).unwrap_or(texture);
    // Typically 160 px and letterboxed: fine to look at for a moment, not to keep.
    Some(Quick { texture, sharp: false })
}

// ── texture helpers ─────────────────────────────────────────────────────

pub fn texture_from_pixbuf(pixbuf: &Pixbuf) -> gdk::Texture {
    let format = if pixbuf.has_alpha() { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
    gdk::MemoryTexture::new(
        pixbuf.width(),
        pixbuf.height(),
        format,
        &pixbuf.read_pixel_bytes(),
        pixbuf.rowstride() as usize,
    )
    .into()
}

/// Turn a texture clockwise by quarter turns.
pub fn rotate(texture: &gdk::Texture, quarters: u8) -> gdk::Texture {
    let turned = adjust(texture, exif::turned(1, quarters), i32::MAX - i32::MAX / 4);
    turned.unwrap_or_else(|| texture.clone())
}

/// Apply an EXIF orientation and shrink to `max`. `None` when nothing had to change.
fn adjust(texture: &gdk::Texture, orientation: u16, max: i32) -> Option<gdk::Texture> {
    let (width, height) = (texture.width(), texture.height());
    let oversized = width.max(height) > max + max / 4;
    if orientation <= 1 && !oversized {
        return None;
    }
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let mut pixbuf = Pixbuf::from_bytes(&bytes, Colorspace::Rgb, true, 8, width, height, stride as i32);
    if oversized {
        let scale = max as f64 / width.max(height) as f64;
        let (w, h) = ((width as f64 * scale).round() as i32, (height as f64 * scale).round() as i32);
        pixbuf = pixbuf.scale_simple(w.max(1), h.max(1), InterpType::Bilinear)?;
    }
    let rotate = |p: &Pixbuf, r| p.rotate_simple(r);
    let pixbuf = match orientation {
        2 => pixbuf.flip(true)?,
        3 => rotate(&pixbuf, PixbufRotation::Upsidedown)?,
        4 => pixbuf.flip(false)?,
        5 => rotate(&pixbuf, PixbufRotation::Clockwise)?.flip(true)?,
        6 => rotate(&pixbuf, PixbufRotation::Clockwise)?,
        7 => rotate(&pixbuf, PixbufRotation::Clockwise)?.flip(false)?,
        8 => rotate(&pixbuf, PixbufRotation::Counterclockwise)?,
        _ => pixbuf,
    };
    Some(texture_from_pixbuf(&pixbuf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_png_text() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, data) in [(b"IHDR", &b"0123456789abc"[..]), (b"tEXt", b"Thumb::MTime\x001700000000")] {
            png.extend((data.len() as u32).to_be_bytes());
            png.extend(kind);
            png.extend(data);
            png.extend([0; 4]);
        }
        assert_eq!(png_text(&png, "Thumb::MTime").as_deref(), Some("1700000000"));
        assert_eq!(png_text(&png, "Thumb::URI"), None);
        assert_eq!(png_text(b"nope", "Thumb::MTime"), None);
    }

    #[test]
    fn cache_flavour_order() {
        let names = |t| size_preference(t).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(names(336), ["x-large", "xx-large", "large", "normal"]);
        assert_eq!(names(168), ["large", "x-large", "xx-large", "normal"]);
    }
}
