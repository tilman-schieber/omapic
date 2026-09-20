//! Image decoding off the main thread: a small worker pool for thumbnails
//! and a blocking `decode` that previews run through `gio::spawn_blocking`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use gtk::gdk;
use gtk::gdk_pixbuf::{Pixbuf, PixbufRotation};

use crate::model::ImageId;
use crate::quick;

/// Decode `path`, scaled down so neither side exceeds `max` pixels.
/// Small images are never scaled up. EXIF orientation is applied, then
/// `quarters` clockwise turns on top (rotation pending in the session).
pub fn decode(path: &Path, max: i32, quarters: u8) -> Option<gdk::Texture> {
    if crate::cli::is_jpeg(path) {
        if let Some(texture) = decode_jpeg(path, max, quarters) {
            return Some(texture);
        }
    }
    let (_, width, height) = Pixbuf::file_info(path)?;
    let pixbuf = if width > max || height > max {
        Pixbuf::from_file_at_scale(path, max, max, true).ok()?
    } else {
        Pixbuf::from_file(path).ok()?
    };
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    let rotation = match quarters % 4 {
        1 => PixbufRotation::Clockwise,
        2 => PixbufRotation::Upsidedown,
        3 => PixbufRotation::Counterclockwise,
        _ => PixbufRotation::None,
    };
    let pixbuf = pixbuf.rotate_simple(rotation).unwrap_or(pixbuf);
    Some(quick::texture_from_pixbuf(&pixbuf))
}

/// JPEGs through libjpeg-turbo, which can decode straight to 1/2, 1/4 or
/// 1/8 size — several times faster than decoding everything and scaling.
/// `None` (CMYK, damaged, not really a JPEG…) falls back to the generic loader.
fn decode_jpeg(path: &Path, max: i32, quarters: u8) -> Option<gdk::Texture> {
    let data = std::fs::read(path).ok()?;
    let mut decompressor = turbojpeg::Decompressor::new().ok()?;
    let header = decompressor.read_header(&data).ok()?;
    let longest = header.width.max(header.height);
    // The smallest size that still covers `max`; the rest is done by `fit`.
    let factor = turbojpeg::Decompressor::supported_scaling_factors()
        .into_iter()
        .filter(|f| f.scale(longest) >= (max as usize).min(longest))
        .min_by_key(|f| f.scale(longest))?;
    decompressor.set_scaling_factor(factor).ok()?;
    let size = header.scaled(factor);
    let mut image = turbojpeg::Image {
        pixels: vec![0u8; 3 * size.width * size.height],
        width: size.width,
        pitch: 3 * size.width,
        height: size.height,
        format: turbojpeg::PixelFormat::RGB,
    };
    decompressor.decompress(&data, image.as_deref_mut()).ok()?;
    let texture: gdk::Texture = gdk::MemoryTexture::new(
        size.width as i32,
        size.height as i32,
        gdk::MemoryFormat::R8g8b8,
        &gtk::glib::Bytes::from_owned(image.pixels),
        image.pitch,
    )
    .into();

    let exif = crate::exif::Exif::find(&data[..data.len().min(128 * 1024)]);
    let orientation = exif.and_then(|e| e.orientation()).map_or(1, |(_, _, value)| value);
    Some(quick::fit(texture, crate::exif::turned(orientation, quarters), max))
}

/// Cheap sources first for everything on screen, real decodes after that.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Quick,
    Full,
}

struct Job {
    id: ImageId,
    path: PathBuf,
    /// Grid position the thumbnail was requested for.
    position: usize,
    stage: Stage,
}

#[derive(Default)]
struct Jobs {
    queue: Vec<Job>,
    /// Requested and not cancelled; a worker may be busy with it right now.
    wanted: HashSet<ImageId>,
}

pub struct Thumbnail {
    pub id: ImageId,
    /// `None`: the image cannot be read.
    pub texture: Option<gdk::Texture>,
    /// `false` for a stand-in that a proper decode will replace.
    pub sharp: bool,
}

pub struct Thumbnailer {
    jobs: Arc<(Mutex<Jobs>, Condvar)>,
    /// Grid position at the centre of the viewport; nearby jobs go first.
    focus: Arc<AtomicUsize>,
    /// Longest edge thumbnails are decoded to, in device pixels.
    size: Arc<AtomicI32>,
}

impl Thumbnailer {
    pub fn new(size: i32) -> (Self, async_channel::Receiver<Thumbnail>) {
        let jobs: Arc<(Mutex<Jobs>, Condvar)> = Arc::default();
        let focus = Arc::new(AtomicUsize::new(0));
        let size = Arc::new(AtomicI32::new(size));
        let (tx, rx) = async_channel::unbounded();
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 8));
        for _ in 0..workers {
            let (jobs, tx, focus, size) = (jobs.clone(), tx.clone(), focus.clone(), size.clone());
            std::thread::spawn(move || {
                loop {
                    let job = next_job(&jobs, &focus);
                    let size = size.load(Ordering::Relaxed);
                    let result = match job.stage {
                        Stage::Quick => match quick::find(&job.path, size) {
                            Some(found) if found.sharp => {
                                jobs.0.lock().unwrap().wanted.remove(&job.id);
                                Some((Some(found.texture), true))
                            }
                            found => {
                                requeue(&jobs, &job);
                                found.map(|f| (Some(f.texture), false))
                            }
                        },
                        Stage::Full => {
                            jobs.0.lock().unwrap().wanted.remove(&job.id);
                            Some((decode(&job.path, size, 0), true))
                        }
                    };
                    if let Some((texture, sharp)) = result {
                        if tx.send_blocking(Thumbnail { id: job.id, texture, sharp }).is_err() {
                            return;
                        }
                    }
                }
            });
        }
        (Thumbnailer { jobs, focus, size }, rx)
    }

    pub fn set_size(&self, size: i32) {
        self.size.store(size, Ordering::Relaxed);
    }

    pub fn set_focus(&self, position: usize) {
        self.focus.store(position, Ordering::Relaxed);
    }

    /// `have_standin`: a placeholder is already showing, go straight to the real decode.
    pub fn request(&self, id: ImageId, path: PathBuf, position: usize, have_standin: bool) {
        let (lock, ready) = &*self.jobs;
        let mut jobs = lock.lock().unwrap();
        let stage = if have_standin { Stage::Full } else { Stage::Quick };
        if jobs.wanted.insert(id) {
            jobs.queue.push(Job { id, path, position, stage });
            ready.notify_one();
        } else if let Some(job) = jobs.queue.iter_mut().find(|j| j.id == id) {
            job.position = position;
        }
    }

    /// Forget a request that scrolled out of view before a worker got to it.
    pub fn cancel(&self, id: ImageId) {
        let mut jobs = self.jobs.0.lock().unwrap();
        jobs.wanted.remove(&id);
        jobs.queue.retain(|j| j.id != id);
    }
}

fn next_job(jobs: &(Mutex<Jobs>, Condvar), focus: &AtomicUsize) -> Job {
    let (lock, ready) = jobs;
    let mut jobs = lock.lock().unwrap();
    loop {
        let centre = focus.load(Ordering::Relaxed);
        let queue = &jobs.queue;
        let best = (0..queue.len()).min_by_key(|&i| (queue[i].stage, queue[i].position.abs_diff(centre)));
        match best {
            Some(i) => return jobs.queue.swap_remove(i),
            None => jobs = ready.wait(jobs).unwrap(),
        }
    }
}

/// The quick look wasn't enough: line the image up for a real decode,
/// unless it scrolled away in the meantime.
fn requeue(jobs: &(Mutex<Jobs>, Condvar), job: &Job) {
    let (lock, ready) = jobs;
    let mut jobs = lock.lock().unwrap();
    if jobs.wanted.contains(&job.id) {
        jobs.queue.push(Job { id: job.id, path: job.path.clone(), position: job.position, stage: Stage::Full });
        ready.notify_one();
    }
}
