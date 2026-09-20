//! Just enough EXIF: find the block in a JPEG, read a handful of tags, the
//! embedded preview and the orientation — and produce the bytes for setting
//! an orientation where none was recorded. Pure byte work, no I/O.

use std::ops::Range;

pub const ORIENTATION: u16 = 0x0112;
pub const MAKE: u16 = 0x010f;
pub const MODEL: u16 = 0x0110;
const EXIF_IFD: u16 = 0x8769;
const THUMBNAIL_OFFSET: u16 = 0x0201;
const THUMBNAIL_LENGTH: u16 = 0x0202;
pub const EXPOSURE_TIME: u16 = 0x829a;
pub const F_NUMBER: u16 = 0x829d;
pub const ISO: u16 = 0x8827;
pub const DATE_TAKEN: u16 = 0x9003;
pub const FOCAL_LENGTH: u16 = 0x920a;

/// The EXIF block of a JPEG, which is a small TIFF file.
pub struct Exif<'a> {
    jpeg: &'a [u8],
    /// The whole APP1 segment, marker included.
    segment: Range<usize>,
    /// File offset all TIFF offsets are relative to.
    tiff_start: usize,
    little_endian: bool,
}

#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    kind: u16,
    count: usize,
    /// TIFF offset of the entry's 4 value bytes.
    value_at: usize,
}

impl<'a> Exif<'a> {
    /// `jpeg` may be just the beginning of the file, as long as it holds the block.
    pub fn find(jpeg: &'a [u8]) -> Option<Self> {
        if jpeg.get(..2)? != [0xff, 0xd8] {
            return None;
        }
        let mut pos = 2;
        loop {
            let marker = jpeg.get(pos..pos + 4)?;
            if marker[0] != 0xff || marker[1] == 0xda || marker[1] == 0xd9 {
                return None;
            }
            let end = pos + 2 + u16::from_be_bytes([marker[2], marker[3]]) as usize;
            if marker[1] == 0xe1 && jpeg.get(pos + 4..pos + 10)? == b"Exif\0\0" {
                let little_endian = match jpeg.get(pos + 10..pos + 12)? {
                    b"II" => true,
                    b"MM" => false,
                    _ => return None,
                };
                return Some(Exif { jpeg, segment: pos..end, tiff_start: pos + 10, little_endian });
            }
            pos = end;
        }
    }

    fn tiff(&self) -> &'a [u8] {
        &self.jpeg[self.tiff_start..self.segment.end.min(self.jpeg.len())]
    }

    fn number(&self, at: usize, len: usize) -> Option<usize> {
        let bytes = self.tiff().get(at..at.checked_add(len)?)?;
        Some(bytes.iter().enumerate().fold(0, |value, (i, &b)| {
            value | (b as usize) << (8 * if self.little_endian { i } else { len - 1 - i })
        }))
    }

    fn entries(&self, ifd: usize) -> Vec<Entry> {
        let count = self.number(ifd, 2).unwrap_or(0);
        (0..count)
            .map_while(|i| {
                let at = ifd + 2 + i * 12;
                Some(Entry {
                    tag: self.number(at, 2)? as u16,
                    kind: self.number(at + 2, 2)? as u16,
                    count: self.number(at + 4, 4)?,
                    value_at: at + 8,
                })
            })
            .collect()
    }

    fn ifd0(&self) -> usize {
        self.number(4, 4).unwrap_or(0)
    }

    fn ifd1(&self) -> Option<usize> {
        let ifd0 = self.ifd0();
        self.number(ifd0 + 2 + self.number(ifd0, 2)? * 12, 4).filter(|&at| at != 0)
    }

    /// Look in IFD0 and the Exif sub-IFD, which is where the tags above live.
    fn entry(&self, tag: u16) -> Option<Entry> {
        let main = self.entries(self.ifd0());
        let find = |list: &[Entry], tag| list.iter().copied().find(|e| e.tag == tag);
        find(&main, tag).or_else(|| {
            let sub = find(&main, EXIF_IFD)?;
            find(&self.entries(self.number(sub.value_at, 4)?), tag)
        })
    }

    /// File offset of the two orientation bytes, their byte order, the value.
    pub fn orientation(&self) -> Option<(usize, bool, u16)> {
        let entry = self.entry(ORIENTATION)?;
        let value = self.number(entry.value_at, 2)? as u16;
        Some((self.tiff_start + entry.value_at, self.little_endian, value))
    }

    /// Byte range of the embedded JPEG preview.
    pub fn thumbnail(&self) -> Option<Range<usize>> {
        let entries = self.entries(self.ifd1()?);
        let value = |tag| entries.iter().find(|e| e.tag == tag).and_then(|e| self.number(e.value_at, 4));
        let start = self.tiff_start + value(THUMBNAIL_OFFSET)?;
        let range = start..start.checked_add(value(THUMBNAIL_LENGTH)?)?;
        self.jpeg.get(range.clone())?.starts_with(&[0xff, 0xd8]).then_some(range)
    }

    pub fn text(&self, tag: u16) -> Option<String> {
        let entry = self.entry(tag).filter(|e| e.kind == 2)?;
        let at = if entry.count <= 4 { entry.value_at } else { self.number(entry.value_at, 4)? };
        let bytes = self.tiff().get(at..at.checked_add(entry.count)?)?;
        let text = String::from_utf8_lossy(bytes.split(|&b| b == 0).next()?).trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    pub fn integer(&self, tag: u16) -> Option<usize> {
        let entry = self.entry(tag)?;
        match entry.kind {
            3 => self.number(entry.value_at, 2),
            4 => self.number(entry.value_at, 4),
            _ => None,
        }
    }

    /// (numerator, denominator)
    pub fn rational(&self, tag: u16) -> Option<(usize, usize)> {
        let entry = self.entry(tag).filter(|e| e.kind == 5)?;
        let at = self.number(entry.value_at, 4)?;
        Some((self.number(at, 4)?, self.number(at + 4, 4)?)).filter(|(_, d)| *d != 0)
    }

    /// Camera facts for the info line, most telling first.
    pub fn summary(&self) -> Vec<String> {
        let mut facts = Vec::new();
        if let Some(date) = self.text(DATE_TAKEN) {
            // "2024:05:01 12:30:00" → "taken 2024-05-01 12:30"
            let (day, time) = date.split_once(' ').unwrap_or((&date, ""));
            facts.push(format!("taken {} {}", day.replace(':', "-"), time.get(..5).unwrap_or(time)).trim().to_string());
        }
        let camera = match (self.text(MAKE), self.text(MODEL)) {
            (Some(make), Some(model)) if !model.starts_with(&make) => Some(format!("{make} {model}")),
            (_, Some(model)) => Some(model),
            (make, None) => make,
        };
        facts.extend(camera);
        if let Some((n, d)) = self.rational(EXPOSURE_TIME).filter(|(n, _)| *n != 0) {
            facts.push(if n < d { format!("1/{} s", (d as f64 / n as f64).round()) } else { format!("{} s", n as f64 / d as f64) });
        }
        if let Some((n, d)) = self.rational(F_NUMBER) {
            facts.push(format!("f/{}", (n as f64 / d as f64 * 10.0).round() / 10.0));
        }
        if let Some(iso) = self.integer(ISO) {
            facts.push(format!("ISO {iso}"));
        }
        if let Some((n, d)) = self.rational(FOCAL_LENGTH) {
            facts.push(format!("{} mm", (n as f64 / d as f64).round()));
        }
        facts
    }

    /// For a block that has no orientation entry: the APP1 segment to put in
    /// place of `old`, recording `orientation`.
    ///
    /// Nothing inside the block moves — maker notes and other data with
    /// absolute offsets stay valid. A copy of IFD0 with the extra entry is
    /// appended and the header pointed at it; the old directory stays behind
    /// as a few dead bytes.
    pub fn with_orientation(&self, orientation: u16) -> Option<(Range<usize>, Vec<u8>)> {
        if self.segment.end > self.jpeg.len() || self.orientation().is_some() {
            return None;
        }
        let put = |value: usize, len: usize| -> Vec<u8> {
            let big = (value as u32).to_be_bytes()[4 - len..].to_vec();
            if self.little_endian { big.into_iter().rev().collect() } else { big }
        };
        let ifd0 = self.ifd0();
        let old = self.entries(ifd0);
        let next_ifd = self.tiff().get(ifd0 + 2 + old.len() * 12..ifd0 + 6 + old.len() * 12)?;

        let mut raw: Vec<(u16, Vec<u8>)> = old
            .iter()
            .map(|e| Some((e.tag, self.tiff().get(e.value_at - 8..e.value_at + 4)?.to_vec())))
            .collect::<Option<_>>()?;
        let mut entry = [put(ORIENTATION as usize, 2), put(3, 2), put(1, 4)].concat();
        entry.extend(put(orientation as usize, 2));
        entry.extend([0, 0]);
        raw.push((ORIENTATION, entry));
        raw.sort_by_key(|(tag, _)| *tag); // TIFF wants directories sorted

        let mut tiff = self.tiff().to_vec();
        if tiff.len() % 2 == 1 {
            tiff.push(0);
        }
        let new_ifd0 = tiff.len();
        tiff.splice(4..8, put(new_ifd0, 4));
        tiff.extend(put(raw.len(), 2));
        raw.iter().for_each(|(_, bytes)| tiff.extend(bytes));
        tiff.extend(next_ifd);

        let length = u16::try_from(tiff.len() + 8).ok()?; // an APP1 segment holds 64 KiB at most
        let mut segment = vec![0xff, 0xe1];
        segment.extend(length.to_be_bytes());
        segment.extend(b"Exif\0\0");
        segment.extend(tiff);
        Some((self.segment.clone(), segment))
    }
}

/// A minimal APP1 segment for a JPEG without any EXIF data.
pub fn minimal_segment(orientation: u16) -> Vec<u8> {
    let mut segment = vec![0xff, 0xe1, 0, 34];
    segment.extend(b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01");
    segment.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1]);
    segment.extend(orientation.to_be_bytes());
    segment.extend([0, 0, 0, 0, 0, 0]);
    segment
}

/// The EXIF orientation that shows an image `quarters` further clockwise.
pub fn turned(orientation: u16, quarters: u8) -> u16 {
    (0..quarters % 4).fold(orientation, |o, _| match o {
        6 => 3,
        3 => 8,
        8 => 1,
        // the mirrored ones
        2 => 7,
        7 => 4,
        4 => 5,
        5 => 2,
        _ => 6,
    })
}

/// Does this orientation swap width and height?
pub fn is_sideways(orientation: u16) -> bool {
    (5..=8).contains(&orientation)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// SOI, JFIF stub, APP1 with: IFD0 {make → string data, [orientation]},
    /// IFD1 {thumbnail offset, length}, then the data. `little`: byte order.
    pub fn sample_jpeg(orientation: Option<u16>, thumbnail: &[u8], little: bool) -> Vec<u8> {
        let n16 = |v: u16| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        let n32 = |v: u32| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        let entry = |tag: u16, kind: u16, count: u32, value: [u8; 4]| {
            [&n16(tag)[..], &n16(kind), &n32(count), &value].concat()
        };
        let count0 = 1 + orientation.is_some() as u32;
        let ifd1 = 8 + 2 + 12 * count0 + 4;
        let make_at = ifd1 + 2 + 24 + 4;
        let thumb_at = make_at + 6;

        let mut tiff = if little { b"II\x2a\0".to_vec() } else { b"MM\0\x2a".to_vec() };
        tiff.extend(n32(8));
        tiff.extend(n16(count0 as u16));
        tiff.extend(entry(MAKE, 2, 6, n32(make_at)));
        if let Some(o) = orientation {
            let short = n16(o);
            tiff.extend(entry(ORIENTATION, 3, 1, [short[0], short[1], 0, 0]));
        }
        tiff.extend(n32(ifd1));
        tiff.extend(n16(2));
        tiff.extend(entry(THUMBNAIL_OFFSET, 4, 1, n32(thumb_at)));
        tiff.extend(entry(THUMBNAIL_LENGTH, 4, 1, n32(thumbnail.len() as u32)));
        tiff.extend(n32(0));
        tiff.extend(b"ACME\0\0");
        tiff.extend(thumbnail);

        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, b'J', b'F', 0xff, 0xe1];
        jpeg.extend(((tiff.len() + 8) as u16).to_be_bytes());
        jpeg.extend(b"Exif\0\0");
        jpeg.extend(&tiff);
        jpeg.extend([0xff, 0xda, 0, 2, 9, 9, 0xff, 0xd9]);
        jpeg
    }

    const THUMB: [u8; 7] = [0xff, 0xd8, 1, 2, 3, 0xff, 0xd9];

    #[test]
    fn reads_both_byte_orders() {
        for little in [false, true] {
            let jpeg = sample_jpeg(Some(6), &THUMB, little);
            let exif = Exif::find(&jpeg).unwrap();
            let (offset, little_endian, value) = exif.orientation().unwrap();
            assert_eq!((little_endian, value), (little, 6));
            assert_eq!(jpeg[offset..offset + 2], if little { [6, 0] } else { [0, 6] });
            assert_eq!(&jpeg[exif.thumbnail().unwrap()], THUMB);
            assert_eq!(exif.text(MAKE).as_deref(), Some("ACME"));
            assert_eq!(exif.text(MODEL), None);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(Exif::find(b"junk").is_none());
        assert!(Exif::find(&[0xff, 0xd8, 0xff, 0xda, 0, 2]).is_none());
        let jpeg = sample_jpeg(Some(1), b"not a jpeg", false);
        assert!(Exif::find(&jpeg).unwrap().thumbnail().is_none());
        let mut truncated = sample_jpeg(Some(1), &THUMB, false);
        truncated.truncate(truncated.len() - 12);
        assert!(Exif::find(&truncated).unwrap().thumbnail().is_none());
    }

    #[test]
    fn adds_an_orientation_without_moving_anything() {
        for little in [false, true] {
            let jpeg = sample_jpeg(None, &THUMB, little);
            let exif = Exif::find(&jpeg).unwrap();
            assert!(exif.orientation().is_none());
            let (old, segment) = exif.with_orientation(8).unwrap();

            let mut rebuilt = jpeg[..old.start].to_vec();
            rebuilt.extend(&segment);
            rebuilt.extend(&jpeg[old.end..]);
            let exif = Exif::find(&rebuilt).unwrap();
            assert_eq!(exif.orientation().unwrap().2, 8);
            assert_eq!(exif.text(MAKE).as_deref(), Some("ACME"));
            assert_eq!(&rebuilt[exif.thumbnail().unwrap()], THUMB);
            assert_eq!(rebuilt[rebuilt.len() - 8..], jpeg[jpeg.len() - 8..]); // image data follows intact
            assert!(exif.with_orientation(1).is_none()); // there is one now
        }
    }

    #[test]
    fn turning() {
        assert_eq!([1, 2, 3].map(|q| turned(1, q)), [6, 3, 8]);
        assert_eq!(turned(8, 1), 1);
        assert_eq!(turned(6, 4), 6);
        for mirrored in [2, 4, 5, 7] {
            assert_eq!(turned(turned(mirrored, 1), 3), mirrored);
            assert!([2, 4, 5, 7].contains(&turned(mirrored, 1)));
        }
        assert!(is_sideways(6) && !is_sideways(3));
        assert_eq!(Exif::find(&minimal_segment_jpeg()).unwrap().orientation().unwrap().2, 6);
    }

    fn minimal_segment_jpeg() -> Vec<u8> {
        [&[0xff, 0xd8][..], &minimal_segment(6), &[0xff, 0xda, 0, 2]].concat()
    }
}
