//! Preview pane. Shows the thumbnail at once, then swaps in a full decode
//! made off the main thread, and decodes the neighbours ahead of time so
//! stepping through images is instant. `z` switches between fit-to-pane and
//! actual pixels; the latter pans by dragging and keeps its position from
//! image to image, for comparing near-duplicates.

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

/// An image as the preview should show it.
#[derive(Clone)]
pub struct Shot {
    pub id: ImageId,
    pub path: PathBuf,
    /// Pending rotation, quarter turns clockwise.
    pub turns: u8,
}

struct Full {
    id: ImageId,
    turns: u8,
    texture: gdk::Texture,
    dimensions: (i32, i32),
    /// File and camera facts for the info line.
    info: String,
}

pub struct Preview {
    pub root: gtk::Box,
    scroller: gtk::ScrolledWindow,
    picture: gtk::Picture,
    caption: gtk::Label,
    info: gtk::Label,
    target: Cell<Option<ImageId>>,
    target_path: RefCell<PathBuf>,
    target_turns: Cell<u8>,
    has_full: Cell<bool>,
    /// One image pixel per screen pixel instead of fit-to-pane.
    actual_size: Cell<bool>,
    /// Undownscaled decode of the target, when the cached one isn't.
    original: RefCell<Option<(ImageId, gdk::Texture)>>,
    /// A GIF playing in place of its still frame.
    animation: RefCell<Option<gtk::MediaFile>>,
    /// Last pointer position over the pane.
    pointer: Cell<Option<(f64, f64)>>,
    /// Scroll values to apply once the adjustments have grown: (value, needed upper).
    pending_scroll: [Cell<Option<(f64, f64)>>; 2],
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
        let scroller = gtk::ScrolledWindow::builder().child(&picture).vexpand(true).hexpand(true).build();
        let info = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .visible(false)
            .css_classes(["info"])
            .build();
        root.append(&scroller);
        root.append(&caption);
        root.append(&info);
        let preview = Rc::new(Preview {
            root,
            scroller,
            picture,
            caption,
            info,
            target: Cell::new(None),
            target_path: RefCell::default(),
            target_turns: Cell::new(0),
            has_full: Cell::new(false),
            actual_size: Cell::new(false),
            original: RefCell::default(),
            animation: RefCell::default(),
            pointer: Cell::new(None),
            pending_scroll: Default::default(),
            generation: Cell::new(0),
            cache: RefCell::default(),
            loading: RefCell::default(),
        });
        preview.connect_panning();
        preview
    }

    fn adjustments(&self) -> [gtk::Adjustment; 2] {
        [self.scroller.hadjustment(), self.scroller.vadjustment()]
    }

    fn connect_panning(self: &Rc<Self>) {
        let motion = gtk::EventControllerMotion::new();
        let this = Rc::downgrade(self);
        motion.connect_motion(move |_, x, y| {
            if let Some(this) = this.upgrade() {
                this.pointer.set(Some((x, y)));
            }
        });
        let this = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let Some(this) = this.upgrade() {
                this.pointer.set(None);
            }
        });
        self.scroller.add_controller(motion);

        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((0.0, 0.0)));
        let (this, from) = (Rc::downgrade(self), start.clone());
        drag.connect_drag_begin(move |_, _, _| {
            if let Some(this) = this.upgrade() {
                let [h, v] = this.adjustments();
                from.set((h.value(), v.value()));
            }
        });
        let this = Rc::downgrade(self);
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some(this) = this.upgrade().filter(|t| t.actual_size.get()) {
                let [h, v] = this.adjustments();
                h.set_value(start.get().0 - dx);
                v.set_value(start.get().1 - dy);
            }
        });
        self.scroller.add_controller(drag);

        // A freshly enlarged picture only becomes scrollable after layout.
        for (axis, adjustment) in self.adjustments().into_iter().enumerate() {
            let this = Rc::downgrade(self);
            adjustment.connect_changed(move |adjustment| {
                let Some(this) = this.upgrade() else { return };
                if let Some((value, upper)) = this.pending_scroll[axis].get() {
                    if adjustment.upper() >= upper - 1.0 {
                        this.pending_scroll[axis].set(None);
                        adjustment.set_value(value);
                    }
                }
            });
        }
    }

    /// `i`: a second caption line with file and camera facts.
    pub fn toggle_info(&self) {
        self.info.set_visible(!self.info.is_visible());
    }

    /// Scroll by fractions of the pane, e.g. (0.25, 0.0) a quarter to the right.
    pub fn pan(&self, dx: f64, dy: f64) {
        for (adjustment, step) in self.adjustments().into_iter().zip([dx, dy]) {
            adjustment.set_value(adjustment.value() + step * adjustment.page_size());
        }
    }

    pub fn is_actual_size(&self) -> bool {
        self.actual_size.get()
    }

    pub fn fit(&self) {
        self.actual_size.set(false);
        self.original.take();
        self.pending_scroll.iter().for_each(|p| p.set(None));
        self.layout();
    }

    /// Switch to actual pixels, keeping the spot under the pointer in place.
    pub fn actual_size(self: &Rc<Self>) {
        let Some(id) = self.target.get() else { return };
        let Some((width, height)) = self.dimensions(id) else { return };
        let (pane_w, pane_h) = (self.scroller.width() as f64, self.scroller.height() as f64);
        let scale = self.screen_scale();
        let (full_w, full_h) = (width as f64 / scale, height as f64 / scale);
        // Where the image sits while fitted, to find the spot under the pointer.
        let shrink = (pane_w / full_w).min(pane_h / full_h).min(1.0);
        let (fit_w, fit_h) = (full_w * shrink, full_h * shrink);
        let (x, y) = self.pointer.get().unwrap_or((pane_w / 2.0, pane_h / 2.0));
        let spot_x = ((x - (pane_w - fit_w) / 2.0) / fit_w).clamp(0.0, 1.0);
        let spot_y = ((y - (pane_h - fit_h) / 2.0) / fit_h).clamp(0.0, 1.0);
        self.pending_scroll[0].set(Some((spot_x * full_w - x, full_w)));
        self.pending_scroll[1].set(Some((spot_y * full_h - y, full_h)));
        self.actual_size.set(true);
        self.layout();
        self.fetch_original(id);
    }

    fn screen_scale(&self) -> f64 {
        self.root.native().and_then(|n| n.surface()).map_or(1.0, |s| s.scale())
    }

    fn dimensions(&self, id: ImageId) -> Option<(i32, i32)> {
        self.cached_dimensions(id).or_else(|| {
            let paintable = self.picture.paintable()?;
            Some((paintable.intrinsic_width(), paintable.intrinsic_height()))
        })
    }

    /// Size and fit of the picture for the current mode and image.
    fn layout(&self) {
        let dimensions = self.target.get().and_then(|id| self.dimensions(id));
        match dimensions.filter(|_| self.actual_size.get()) {
            Some((width, height)) => {
                let scale = self.screen_scale();
                let (w, h) = ((width as f64 / scale).round() as i32, (height as f64 / scale).round() as i32);
                self.picture.set_size_request(w, h);
                self.picture.set_halign(gtk::Align::Center);
                self.picture.set_valign(gtk::Align::Center);
                self.picture.set_content_fit(gtk::ContentFit::Contain);
                self.picture.set_cursor_from_name(Some("grab"));
            }
            None => {
                self.picture.set_size_request(-1, -1);
                self.picture.set_halign(gtk::Align::Fill);
                self.picture.set_valign(gtk::Align::Fill);
                // Scaled-up thumbnails look better than a flash of nothing;
                // real images are never blown up.
                let fit = if self.has_full.get() { gtk::ContentFit::ScaleDown } else { gtk::ContentFit::Contain };
                self.picture.set_content_fit(fit);
                self.picture.set_cursor(None::<&gdk::Cursor>);
            }
        }
    }

    /// Previews are capped at `MAX_EDGE`; actual size deserves every pixel.
    fn fetch_original(self: &Rc<Self>, id: ImageId) {
        let downscaled = self.cached_dimensions(id).is_some_and(|(w, h)| w.max(h) > MAX_EDGE);
        if !downscaled || self.original.borrow().as_ref().is_some_and(|(have, _)| *have == id) {
            return;
        }
        let (this, path, turns) = (self.clone(), self.target_path.borrow().clone(), self.target_turns.get());
        glib::spawn_future_local(async move {
            let loaded = gio::spawn_blocking(move || thumbs::decode(&path, i32::MAX, turns)).await;
            if let Ok(Some(texture)) = loaded {
                if this.wants(id, turns) && this.actual_size.get() {
                    this.picture.set_paintable(Some(&texture));
                    this.original.replace(Some((id, texture)));
                }
            }
        });
    }

    /// The file behind `id` changed: drop what was decoded from it.
    pub fn forget(&self, id: ImageId) {
        self.cache.borrow_mut().retain(|f| f.id != id);
        if self.target.get() == Some(id) {
            self.target.set(None);
            self.original.take();
        }
    }

    pub fn clear(&self) {
        self.stop_animation();
        self.target.set(None);
        self.generation.set(self.generation.get() + 1);
        self.picture.set_paintable(gdk::Paintable::NONE);
        self.caption.set_label("");
        self.info.set_label("");
        self.layout();
    }

    /// The thumbnail arrived after the preview was requested.
    pub fn offer_placeholder(&self, id: ImageId, texture: &gdk::Texture) {
        if self.target.get() == Some(id) && !self.has_full.get() {
            self.picture.set_paintable(Some(texture));
        }
    }

    fn wants(&self, id: ImageId, turns: u8) -> bool {
        self.target.get() == Some(id) && self.target_turns.get() == turns
    }

    /// `neighbours` are decoded ahead once the shot itself is on screen.
    pub fn show(self: &Rc<Self>, shot: Shot, placeholder: Option<gdk::Texture>, neighbours: Vec<Shot>) {
        if self.wants(shot.id, shot.turns) {
            self.caption_for(&shot.path, self.cached_dimensions(shot.id));
            return;
        }
        self.stop_animation();
        self.target.set(Some(shot.id));
        self.target_turns.set(shot.turns);
        self.target_path.replace(shot.path.clone());
        self.original.take();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);

        if self.display_cached(&shot) {
            self.animate(&shot);
            self.fetch_original(shot.id);
            for neighbour in neighbours {
                self.fetch(neighbour, Vec::new());
            }
            return;
        }
        self.has_full.set(false);
        self.picture.set_paintable(placeholder.as_ref());
        self.caption_for(&shot.path, None);
        self.info.set_label("");
        self.layout();

        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(SETTLE, move || {
            if let Some(this) = this.upgrade().filter(|t| t.generation.get() == generation) {
                this.fetch(shot, neighbours);
            }
        });
    }

    fn stop_animation(&self) {
        if let Some(media) = self.animation.take() {
            media.pause();
        }
    }

    /// GIFs play once their still frame is up — if GTK's media backend can
    /// decode them; otherwise the still simply stays.
    fn animate(self: &Rc<Self>, shot: &Shot) {
        let is_gif = shot.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"));
        if !is_gif || shot.turns != 0 {
            return;
        }
        let media = gtk::MediaFile::for_filename(&shot.path);
        media.set_loop(true);
        media.set_muted(true);
        let (this, id) = (Rc::downgrade(self), shot.id);
        media.connect_prepared_notify(move |media| {
            let Some(this) = this.upgrade() else { return };
            let current = this.animation.borrow().as_ref() == Some(media);
            if current && media.is_prepared() && media.error().is_none() && this.wants(id, 0) {
                this.picture.set_paintable(Some(media));
            }
        });
        media.play();
        self.animation.replace(Some(media));
    }

    /// Show the shot from the cache, marking it most recently used.
    fn display_cached(&self, shot: &Shot) -> bool {
        {
            let mut cache = self.cache.borrow_mut();
            let Some(index) = cache.iter().position(|f| f.id == shot.id && f.turns == shot.turns) else {
                return false;
            };
            let full = cache.remove(index).unwrap();
            self.has_full.set(true);
            self.picture.set_paintable(Some(&full.texture));
            self.caption_for(&shot.path, Some(full.dimensions));
            self.info.set_label(&full.info);
            cache.push_back(full);
        }
        self.layout();
        true
    }

    /// Decode into the cache; display if it is (still) the image wanted.
    /// `then` are fetched afterwards, so they never compete with `shot`.
    fn fetch(self: &Rc<Self>, shot: Shot, then: Vec<Shot>) {
        let known = self.cache.borrow().iter().any(|f| f.id == shot.id && f.turns == shot.turns);
        if known || !self.loading.borrow_mut().insert(shot.id) {
            for next in then {
                self.fetch(next, Vec::new());
            }
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            let job = shot.clone();
            let loaded = gio::spawn_blocking(move || {
                let dimensions = Pixbuf::file_info(&job.path).map(|(_, w, h)| (w, h));
                (thumbs::decode(&job.path, MAX_EDGE, job.turns), dimensions, facts(&job.path))
            })
            .await;
            this.loading.borrow_mut().remove(&shot.id);
            if let Ok((Some(texture), dimensions, (sideways, info))) = loaded {
                let (w, h) = dimensions.unwrap_or((texture.width(), texture.height()));
                // Dimensions as shown: EXIF orientation and pending rotation applied.
                let dimensions = if sideways != (shot.turns % 2 == 1) { (h, w) } else { (w, h) };
                {
                    let mut cache = this.cache.borrow_mut();
                    cache.retain(|f| f.id != shot.id);
                    cache.push_back(Full { id: shot.id, turns: shot.turns, texture, dimensions, info });
                    while cache.len() > CACHED {
                        cache.pop_front();
                    }
                }
                if this.wants(shot.id, shot.turns) && !this.has_full.get() {
                    this.display_cached(&shot);
                    this.animate(&shot);
                    this.fetch_original(shot.id);
                }
            }
            for next in then {
                this.fetch(next, Vec::new());
            }
        });
    }

    fn cached_dimensions(&self, id: ImageId) -> Option<(i32, i32)> {
        let turns = self.target_turns.get();
        self.cache.borrow().iter().find(|f| f.id == id && f.turns == turns).map(|f| f.dimensions)
    }

    fn caption_for(&self, path: &std::path::Path, dimensions: Option<(i32, i32)>) {
        let mut text = file_name(path);
        if let Some((w, h)) = dimensions {
            text.push_str(&format!("  ·  {w}×{h}"));
        }
        self.caption.set_label(&text);
    }
}

/// Blocking: is the image stored sideways (EXIF), and its info line.
fn facts(path: &std::path::Path) -> (bool, String) {
    use std::io::Read;
    let mut facts = Vec::new();
    if let Ok(meta) = path.metadata() {
        facts.push(glib::format_size(meta.len()).to_string());
        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
        let modified = modified.and_then(|d| glib::DateTime::from_unix_local(d.as_secs() as i64).ok());
        if let Some(text) = modified.and_then(|d| d.format("%Y-%m-%d %H:%M").ok()) {
            facts.push(format!("modified {text}"));
        }
    }
    let mut head = Vec::new();
    if crate::cli::is_jpeg(path) {
        let _ = std::fs::File::open(path).map(|f| f.take(128 * 1024).read_to_end(&mut head));
    }
    let exif = crate::exif::Exif::find(&head);
    let sideways = exif.as_ref().and_then(|e| e.orientation()).is_some_and(|(_, _, o)| crate::exif::is_sideways(o));
    facts.extend(exif.map(|e| e.summary()).unwrap_or_default());
    (sideways, facts.join("  ·  "))
}
