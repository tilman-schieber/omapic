//! Preview pane. Shows the thumbnail at once, then swaps in a full decode
//! made off the main thread. Loads that are no longer wanted are dropped.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
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

    pub fn show(self: &Rc<Self>, id: ImageId, path: PathBuf, placeholder: Option<gdk::Texture>) {
        if self.target.get() == Some(id) {
            self.caption_for(&path, self.cached_dimensions(id));
            return;
        }
        self.target.set(Some(id));
        let generation = self.generation.get() + 1;
        self.generation.set(generation);

        if let Some(full) = self.cache.borrow().iter().find(|f| f.id == id) {
            self.has_full.set(true);
            self.picture.set_content_fit(gtk::ContentFit::ScaleDown);
            self.picture.set_paintable(Some(&full.texture));
            self.caption_for(&path, Some(full.dimensions));
            return;
        }
        self.has_full.set(false);
        // Scaled-up thumbnails look better than a flash of nothing.
        self.picture.set_content_fit(gtk::ContentFit::Contain);
        self.picture.set_paintable(placeholder.as_ref());
        self.caption_for(&path, None);

        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(SETTLE, move || {
            let Some(this) = this.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            glib::spawn_future_local(async move {
                let job_path = path.clone();
                let loaded = gio::spawn_blocking(move || {
                    let dimensions = Pixbuf::file_info(&job_path).map(|(_, w, h)| (w, h));
                    (thumbs::decode(&job_path, MAX_EDGE), dimensions)
                })
                .await;
                let Ok((Some(texture), dimensions)) = loaded else { return };
                let dimensions = dimensions.unwrap_or((texture.width(), texture.height()));
                if this.generation.get() == generation {
                    this.has_full.set(true);
                    this.picture.set_content_fit(gtk::ContentFit::ScaleDown);
                    this.picture.set_paintable(Some(&texture));
                    this.caption_for(&path, Some(dimensions));
                }
                let mut cache = this.cache.borrow_mut();
                cache.retain(|f| f.id != id);
                cache.push_back(Full { id, texture, dimensions });
                while cache.len() > CACHED {
                    cache.pop_front();
                }
            });
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
