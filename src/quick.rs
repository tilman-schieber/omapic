//! Thumbnails that already exist somewhere, so nothing has to be decoded at
//! full size: the freedesktop thumbnail cache (read, never written) and the
//! preview embedded in a JPEG's EXIF block.

use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use gtk::gdk;
use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf, PixbufRotation};
use gtk::gio::prelude::*;
use gtk::glib;
use gtk::prelude::*;

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
    let (range, orientation) = exif_thumbnail(&start)?;
    let bytes = glib::Bytes::from(&start[range]);
    let texture = gdk::Texture::from_bytes(&bytes).ok()?;
    let texture = adjust(&texture, orientation, target).unwrap_or(texture);
    // Typically 160 px and letterboxed: fine to look at for a moment, not to keep.
    Some(Quick { texture, sharp: false })
}

/// Locate the JPEG preview in a JPEG's EXIF data: (byte range, orientation).
pub fn exif_thumbnail(jpeg: &[u8]) -> Option<(Range<usize>, u16)> {
    if jpeg.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut pos = 2;
    let tiff_start = loop {
        let marker = jpeg.get(pos..pos + 4)?;
        if marker[0] != 0xff || marker[1] == 0xda || marker[1] == 0xd9 {
            return None;
        }
        let length = u16::from_be_bytes([marker[2], marker[3]]) as usize;
        if marker[1] == 0xe1 && jpeg.get(pos + 4..pos + 10)? == b"Exif\0\0" {
            break pos + 10;
        }
        pos += 2 + length;
    };
    let tiff = &jpeg[tiff_start..];
    let little = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    };
    let u32_at = |at: usize| -> Option<usize> {
        let b: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) } as usize)
    };

    let ifd0 = u32_at(4)?;
    let entries0 = u16_at(ifd0)? as usize;
    let mut orientation = 1;
    for entry in (0..entries0).map(|i| ifd0 + 2 + i * 12) {
        if u16_at(entry)? == 0x0112 {
            orientation = u16_at(entry + 8)?;
        }
    }
    let ifd1 = u32_at(ifd0 + 2 + entries0 * 12)?;
    if ifd1 == 0 {
        return None;
    }
    let (mut offset, mut length) = (None, None);
    for entry in (0..u16_at(ifd1)? as usize).map(|i| ifd1 + 2 + i * 12) {
        match u16_at(entry)? {
            0x0201 => offset = u32_at(entry + 8),
            0x0202 => length = u32_at(entry + 8),
            _ => {}
        }
    }
    let start = tiff_start + offset?;
    let range = start..start.checked_add(length?)?;
    (jpeg.get(range.clone())?.starts_with(&[0xff, 0xd8])).then_some((range, orientation))
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

    /// SOI, APP0, then APP1 with a big-endian TIFF: IFD0 {orientation},
    /// IFD1 {offset, length}, thumbnail bytes.
    fn sample_jpeg(orientation: u16, thumbnail: &[u8]) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend(b"MM\0\x2a");
        tiff.extend(8u32.to_be_bytes()); // IFD0 at 8
        tiff.extend(1u16.to_be_bytes());
        tiff.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1]);
        tiff.extend(orientation.to_be_bytes());
        tiff.extend([0, 0]);
        let ifd1 = 8 + 2 + 12 + 4;
        tiff.extend((ifd1 as u32).to_be_bytes());
        let data = ifd1 + 2 + 24 + 4;
        tiff.extend(2u16.to_be_bytes());
        tiff.extend([0x02, 0x01, 0, 4, 0, 0, 0, 1]);
        tiff.extend((data as u32).to_be_bytes());
        tiff.extend([0x02, 0x02, 0, 4, 0, 0, 0, 1]);
        tiff.extend((thumbnail.len() as u32).to_be_bytes());
        tiff.extend(0u32.to_be_bytes());
        tiff.extend(thumbnail);

        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, b'J', b'F'];
        jpeg.extend([0xff, 0xe1]);
        jpeg.extend(((tiff.len() + 8) as u16).to_be_bytes());
        jpeg.extend(b"Exif\0\0");
        jpeg.extend(&tiff);
        jpeg.extend([0xff, 0xda, 0, 2]);
        jpeg
    }

    #[test]
    fn finds_exif_thumbnail() {
        let thumb = [0xff, 0xd8, 1, 2, 3, 0xff, 0xd9];
        let jpeg = sample_jpeg(6, &thumb);
        let (range, orientation) = exif_thumbnail(&jpeg).unwrap();
        assert_eq!(&jpeg[range], thumb);
        assert_eq!(orientation, 6);
    }

    #[test]
    fn rejects_garbage() {
        assert!(exif_thumbnail(b"junk").is_none());
        assert!(exif_thumbnail(&[0xff, 0xd8, 0xff, 0xda, 0, 2]).is_none());
        assert!(exif_thumbnail(&sample_jpeg(1, b"not a jpeg")).is_none());
        let mut truncated = sample_jpeg(1, &[0xff, 0xd8, 0, 0]);
        truncated.truncate(truncated.len() - 6);
        assert!(exif_thumbnail(&truncated).is_none());
    }

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
