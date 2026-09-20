//! Image decoding off the main thread: a small worker pool for thumbnails
//! and a blocking `decode` that previews run through `gio::spawn_blocking`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use gtk::gdk;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::glib;

use crate::model::ImageId;

/// Decode `path`, scaled down so neither side exceeds `max` pixels.
/// Small images are never scaled up. EXIF orientation is applied.
pub fn decode(path: &Path, max: i32) -> Option<gdk::Texture> {
    let (_, width, height) = Pixbuf::file_info(path)?;
    let pixbuf = if width > max || height > max {
        Pixbuf::from_file_at_scale(path, max, max, true).ok()?
    } else {
        Pixbuf::from_file(path).ok()?
    };
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    let format = if pixbuf.has_alpha() {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    let bytes: glib::Bytes = pixbuf.read_pixel_bytes();
    let texture = gdk::MemoryTexture::new(
        pixbuf.width(),
        pixbuf.height(),
        format,
        &bytes,
        pixbuf.rowstride() as usize,
    );
    Some(texture.into())
}

/// Image, file, and the grid position it was requested for.
type Job = (ImageId, PathBuf, usize);
type Queue = Arc<(Mutex<Vec<Job>>, Condvar)>;

pub struct Thumbnailer {
    queue: Queue,
    /// Grid position at the centre of the viewport; nearby jobs go first.
    focus: Arc<AtomicUsize>,
}

impl Thumbnailer {
    /// Results arrive on the returned channel; `None` marks an unreadable image.
    pub fn new(size: i32) -> (Self, async_channel::Receiver<(ImageId, Option<gdk::Texture>)>) {
        let queue: Queue = Arc::default();
        let focus = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = async_channel::unbounded();
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 8));
        for _ in 0..workers {
            let (queue, tx, focus) = (queue.clone(), tx.clone(), focus.clone());
            std::thread::spawn(move || {
                loop {
                    let (id, path) = {
                        let (lock, ready) = &*queue;
                        let mut jobs = lock.lock().unwrap();
                        loop {
                            // Closest to the middle of the viewport first.
                            let centre = focus.load(Ordering::Relaxed);
                            let nearest = (0..jobs.len()).min_by_key(|&i| jobs[i].2.abs_diff(centre));
                            match nearest {
                                Some(i) => {
                                    let (id, path, _) = jobs.swap_remove(i);
                                    break (id, path);
                                }
                                None => jobs = ready.wait(jobs).unwrap(),
                            }
                        }
                    };
                    if tx.send_blocking((id, decode(&path, size))).is_err() {
                        return;
                    }
                }
            });
        }
        (Thumbnailer { queue, focus }, rx)
    }

    pub fn set_focus(&self, position: usize) {
        self.focus.store(position, Ordering::Relaxed);
    }

    pub fn request(&self, id: ImageId, path: PathBuf, position: usize) {
        let (lock, ready) = &*self.queue;
        let mut jobs = lock.lock().unwrap();
        jobs.retain(|(other, _, _)| *other != id);
        jobs.push((id, path, position));
        ready.notify_one();
    }

    /// Forget a request that scrolled out of view before a worker got to it.
    pub fn cancel(&self, id: ImageId) {
        self.queue.0.lock().unwrap().retain(|(other, _, _)| *other != id);
    }
}
