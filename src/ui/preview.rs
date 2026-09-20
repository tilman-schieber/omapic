//! Preview pane. Shows the thumbnail at once, then swaps in a full decode
//! made off the main thread, and decodes the neighbours ahead of time so
//! stepping through images is instant.

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::cli::file_name;
use crate::model::ImageId;
use crate::thumbs;

/// Longest edge of a preview decode; larger images are scaled while loading.
const MAX_EDGE: i32 = 4096;
const CACHED: usize = 8;
/// Sweeping the pointer across the grid shouldn't start a decode per thumbnail.
const SETTLE: Duration = Duration::from_millis(40);

struct Full {
    id: ImageId,
    texture: gdk::Texture,
    dimensions: (i32, i32),
}

pub struct Preview {
    pub root: gtk::Box,
    picture: gtk::Picture,
    caption: gtk::Label,
    target: Cell<Option<ImageId>>,
    has_full: Cell<bool>,
    generation: Cell<u64>,
    cache: RefCell<VecDeque<Full>>,
    loading: RefCell<HashSet<ImageId>>,
}

impl Preview {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["preview"])
            .build();
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::ScaleDown)
            .can_shrink(true)
            .vexpand(true)
            .hexpand(true)
            .build();
        let caption = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["caption"])
            .build();
        root.append(&picture);
        root.append(&caption);
        Rc::new(Preview {
            root,
            picture,
            caption,
            target: Cell::new(None),
            has_full: Cell::new(false),
            generation: Cell::new(0),
            cache: RefCell::default(),
            loading: RefCell::default(),
        })
    }

    pub fn clear(&self) {
        self.target.set(None);
        self.generation.set(self.generation.get() + 1);
        self.picture.set_paintable(gdk::Paintable::NONE);
        self.caption.set_label("");
    }

    /// The thumbnail arrived after the preview was requested.
    pub fn offer_placeholder(&self, id: ImageId, texture: &gdk::Texture) {
        if self.target.get() == Some(id) && !self.has_full.get() {
            self.picture.set_paintable(Some(texture));
        }
    }

    /// `neighbours` are decoded ahead once `id` itself is on screen.
    pub fn show(
        self: &Rc<Self>,
        id: ImageId,
        path: PathBuf,
        placeholder: Option<gdk::Texture>,
        neighbours: Vec<(ImageId, PathBuf)>,
    ) {
        if self.target.get() == Some(id) {
            self.caption_for(&path, self.cached_dimensions(id));
            return;
        }
        self.target.set(Some(id));
        let generation = self.generation.get() + 1;
        self.generation.set(generation);

        if self.display_cached(id, &path) {
            for (id, path) in neighbours {
                self.fetch(id, path, Vec::new());
            }
            return;
        }
        self.has_full.set(false);
        // Scaled-up thumbnails look better than a flash of nothing.
        self.picture.set_content_fit(gtk::ContentFit::Contain);
        self.picture.set_paintable(placeholder.as_ref());
        self.caption_for(&path, None);

        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(SETTLE, move || {
            if let Some(this) = this.upgrade().filter(|t| t.generation.get() == generation) {
                this.fetch(id, path, neighbours);
            }
        });
    }

    /// Show `id` from the cache, marking it most recently used.
    fn display_cached(&self, id: ImageId, path: &std::path::Path) -> bool {
        let mut cache = self.cache.borrow_mut();
        let Some(index) = cache.iter().position(|f| f.id == id) else { return false };
        let full = cache.remove(index).unwrap();
        self.has_full.set(true);
        self.picture.set_content_fit(gtk::ContentFit::ScaleDown);
        self.picture.set_paintable(Some(&full.texture));
        self.caption_for(path, Some(full.dimensions));
        cache.push_back(full);
        true
    }

    /// Decode into the cache; display if it is (still) the image wanted.
    /// `then` are fetched afterwards, so they never compete with `id`.
    fn fetch(self: &Rc<Self>, id: ImageId, path: PathBuf, then: Vec<(ImageId, PathBuf)>) {
        let known = self.cache.borrow().iter().any(|f| f.id == id);
        if known || !self.loading.borrow_mut().insert(id) {
            for (id, path) in then {
                self.fetch(id, path, Vec::new());
            }
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            let job_path = path.clone();
            let loaded = gio::spawn_blocking(move || {
                let dimensions = Pixbuf::file_info(&job_path).map(|(_, w, h)| (w, h));
                (thumbs::decode(&job_path, MAX_EDGE), dimensions)
            })
            .await;
            this.loading.borrow_mut().remove(&id);
            if let Ok((Some(texture), dimensions)) = loaded {
                let dimensions = dimensions.unwrap_or((texture.width(), texture.height()));
                {
                    let mut cache = this.cache.borrow_mut();
                    cache.push_back(Full { id, texture, dimensions });
                    while cache.len() > CACHED {
                        cache.pop_front();
                    }
                }
                if this.target.get() == Some(id) && !this.has_full.get() {
                    this.display_cached(id, &path);
                }
            }
            for (id, path) in then {
                this.fetch(id, path, Vec::new());
            }
        });
    }

    fn cached_dimensions(&self, id: ImageId) -> Option<(i32, i32)> {
        self.cache.borrow().iter().find(|f| f.id == id).map(|f| f.dimensions)
    }

    fn caption_for(&self, path: &std::path::Path, dimensions: Option<(i32, i32)>) {
        let mut text = file_name(path);
        if let Some((w, h)) = dimensions {
            text.push_str(&format!("  ·  {w}×{h}"));
        }
        self.caption.set_label(&text);
    }
}
