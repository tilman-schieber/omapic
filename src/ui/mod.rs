//! Window, key handling and the glue between session state and widgets.

mod commands;
#[cfg(debug_assertions)]
mod debug;
mod grid;
mod item;
mod palette;
mod preview;

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::cli::{Input, file_name};
use crate::model::{ImageId, Session, WORKSPACES};
use crate::theme::{self, Theme};
use crate::thumbs::{Thumbnail, Thumbnailer};
use item::ImageItem;
use palette::Palette;
use preview::Preview;

/// Thumbnails kept in memory; beyond this, off-screen ones are dropped again.
const THUMB_CACHE: usize = 1200;

const HELP: &str = "\
<b>←↓↑→</b>  <b>h j k l</b>      move around the grid
<b>home end</b>  <b>g G</b>      first / last image
<b>enter</b>  <b>space</b>       enlarge preview
<b>z</b>                  actual pixels at the pointer; drag to pan
<b>1</b> … <b>9</b>              put image into workspace
<b>0</b>                  take image out of its workspace
<b>alt+1</b> … <b>alt+9</b>      show only that workspace
<b>alt+0</b>              show all images
<b>s</b>                  manual sorting on / off
<b>shift+←→</b>  <b>H L</b>      move image backward / forward
<b>drag</b>               reorder thumbnails
<b>u</b>  <b>U</b> / <b>ctrl+r</b>      undo / redo binning and ordering
<b>f</b>                  file names under thumbnails
<b>:</b>  <b>ctrl+k</b>          commands
<b>?</b>                  this sheet (the status line hints at keys for the current context)
<b>q</b>                  quit";

pub struct App {
    window: gtk::ApplicationWindow,
    session: RefCell<Session>,
    items: Vec<ImageItem>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    grid: gtk::GridView,
    scroller: gtk::ScrolledWindow,
    preview: Rc<Preview>,
    palette: Rc<Palette>,
    strip: gtk::Box,
    info: gtk::Label,
    message: gtk::Label,
    message_generation: Cell<u64>,
    /// A transient message currently covers the key hints.
    message_shown: Cell<bool>,
    thumbs: Thumbnailer,
    thumb_order: RefCell<VecDeque<ImageId>>,
    bound: RefCell<HashSet<ImageId>>,
    /// Ids in the store, in display order.
    view: RefCell<Vec<ImageId>>,
    hovered: Cell<Option<ImageId>>,
    show_names: Cell<bool>,
    /// The file name label of every grid cell built so far.
    name_labels: RefCell<Vec<glib::WeakRef<gtk::Label>>>,
    enlarged: Cell<bool>,
    syncing: Cell<bool>,
    css: gtk::CssProvider,
    theme_monitor: RefCell<Option<gio::FileMonitor>>,
}

pub fn build(application: &gtk::Application, input: Input) {
    let display = gdk::Display::default().expect("no display");
    let scale = display
        .monitors()
        .iter::<gdk::Monitor>()
        .flatten()
        .map(|m| m.scale_factor())
        .max()
        .unwrap_or(1);
    let (thumbs, thumb_results) = Thumbnailer::new(grid::CELL * scale);

    let items: Vec<ImageItem> = (0..input.paths.len()).map(ImageItem::new).collect();
    let store = gio::ListStore::new::<ImageItem>();
    let selection = gtk::SingleSelection::builder()
        .model(&store)
        .autoselect(false)
        .can_unselect(true)
        .build();
    let grid = gtk::GridView::builder()
        .model(&selection)
        .min_columns(1)
        .max_columns(64)
        .css_classes(["grid"])
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&grid)
        .hexpand(true)
        .vexpand(true)
        .build();
    let preview = Preview::new();
    let paned = gtk::Paned::builder()
        .start_child(&scroller)
        .end_child(&preview.root)
        .resize_start_child(true)
        .resize_end_child(true)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .vexpand(true)
        .build();
    preview.root.set_size_request(320, -1);
    // Start with the grid on roughly three fifths of whatever width we are given.
    let placed = Cell::new(false);
    paned.connect_max_position_notify(move |paned| {
        if paned.width() > 1 && !placed.replace(true) {
            paned.set_position(paned.width() * 3 / 5);
        }
    });
    scroller.set_size_request(grid::CELL + 40, -1);

    let strip = gtk::Box::builder().spacing(2).css_classes(["strip"]).build();
    let info = gtk::Label::builder().css_classes(["info"]).build();
    let message = gtk::Label::builder()
        .hexpand(true)
        .xalign(1.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["message"])
        .build();
    let status = gtk::Box::builder().spacing(14).css_classes(["status"]).build();
    status.append(&strip);
    status.append(&info);
    status.append(&message);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&paned);
    content.append(&status);

    let palette = Palette::new(commands::titles(), HELP);
    let overlay = gtk::Overlay::builder().child(&content).build();
    overlay.add_overlay(&palette.root);
    palette.set_focus_return(&grid);

    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("omapic")
        .default_width(1400)
        .default_height(900)
        .decorated(false)
        .child(&overlay)
        .build();

    let css = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let mut session = Session::new(input.paths);
    session.set_active(0);
    if input.select.is_some() {
        session.select(input.select);
    }

    let app = Rc::new(App {
        window,
        session: RefCell::new(session),
        items,
        store,
        selection,
        grid,
        scroller,
        preview,
        palette,
        strip,
        info,
        message,
        message_generation: Cell::new(0),
        message_shown: Cell::new(false),
        thumbs,
        thumb_order: RefCell::default(),
        bound: RefCell::default(),
        view: RefCell::default(),
        hovered: Cell::new(None),
        show_names: Cell::new(false),
        name_labels: RefCell::default(),
        enlarged: Cell::new(false),
        syncing: Cell::new(false),
        css,
        theme_monitor: RefCell::default(),
    });

    app.load_theme();
    app.watch_theme();
    app.grid.set_factory(Some(&grid::factory(&app)));
    app.connect_signals();
    app.sync();

    let weak = Rc::downgrade(&app);
    glib::spawn_future_local(async move {
        while let Ok(thumbnail) = thumb_results.recv().await {
            let Some(app) = weak.upgrade() else { break };
            app.thumbnail_ready(thumbnail);
        }
    });

    if app.items.is_empty() {
        app.say("no images found", true);
    }
    app.window.present();
    app.grid.grab_focus();
    // Once the grid has a size, bring the initially selected image into view.
    let weak = Rc::downgrade(&app);
    glib::idle_add_local_once(move || {
        if let Some(app) = weak.upgrade() {
            app.scroll_to_selected();
        }
    });
    #[cfg(debug_assertions)]
    debug::run_script(&app);
    // The window keeps the app alive through its key controller.
}

impl App {
    fn connect_signals(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let app = self.clone();
        keys.connect_key_pressed(move |_, key, _, state| app.key(key, state));
        self.window.add_controller(keys);

        let weak = Rc::downgrade(self);
        self.selection.connect_selected_notify(move |selection| {
            let Some(app) = weak.upgrade() else { return };
            if app.syncing.get() {
                return;
            }
            let id = app.view.borrow().get(selection.selected() as usize).copied();
            app.session.borrow_mut().select(id);
            app.hovered.set(None);
            app.update_preview();
            app.update_status();
        });

        let weak = Rc::downgrade(self);
        self.grid.connect_activate(move |_, _| {
            if let Some(app) = weak.upgrade() {
                app.toggle_enlarged();
            }
        });

        let weak = Rc::downgrade(self);
        self.scroller.vadjustment().connect_value_changed(move |_| {
            if let Some(app) = weak.upgrade() {
                app.update_thumb_focus();
            }
        });

        let weak = Rc::downgrade(self);
        self.scroller.vadjustment().connect_changed(move |_| {
            if let Some(app) = weak.upgrade() {
                app.update_thumb_focus();
            }
        });

        let leave = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        leave.connect_leave(move |_| {
            if let Some(app) = weak.upgrade() {
                app.hover(None);
            }
        });
        self.scroller.add_controller(leave);

        let weak = Rc::downgrade(self);
        self.palette.connect_command(move |index| {
            if let Some(app) = weak.upgrade() {
                glib::spawn_future_local(commands::run(app, index));
            }
        });
    }

    // ── theme ────────────────────────────────────────────────────────────

    fn load_theme(&self) {
        let theme = Theme::load();
        self.css.load_from_string(&theme.css());
    }

    /// Follow Omarchy theme switches while running.
    fn watch_theme(self: &Rc<Self>) {
        let Some(dir) = theme::theme_dir() else { return };
        let file = gio::File::for_path(dir);
        let Ok(monitor) = file.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) else {
            return;
        };
        let weak = Rc::downgrade(self);
        let pending = Rc::new(Cell::new(false));
        monitor.connect_changed(move |_, _, _, _| {
            if pending.replace(true) {
                return;
            }
            let (weak, pending) = (weak.clone(), pending.clone());
            glib::timeout_add_local_once(Duration::from_millis(400), move || {
                pending.set(false);
                if let Some(app) = weak.upgrade() {
                    app.load_theme();
                }
            });
        });
        self.theme_monitor.replace(Some(monitor));
    }

    // ── session → widgets ────────────────────────────────────────────────

    /// Bring store, selection, preview and status in line with the session.
    fn sync(self: &Rc<Self>) {
        let want = self.session.borrow().visible();
        {
            let mut have = self.view.borrow_mut();
            let prefix = have.iter().zip(&want).take_while(|(a, b)| a == b).count();
            let suffix = have[prefix..]
                .iter()
                .rev()
                .zip(want[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let removed = have.len() - prefix - suffix;
            let added: Vec<ImageItem> =
                want[prefix..want.len() - suffix].iter().map(|&id| self.items[id].clone()).collect();
            if removed > 0 || !added.is_empty() {
                self.syncing.set(true);
                self.store.splice(prefix as u32, removed as u32, &added);
                self.syncing.set(false);
            }
            *have = want;
        }
        for (item, entry) in self.items.iter().zip(self.session.borrow().images()) {
            let ws = entry.workspace.map_or(0, u32::from);
            if item.workspace() != ws {
                item.set_workspace(ws);
            }
            let name = file_name(&entry.path);
            if item.name() != name {
                item.set_name(name); // files get renamed by commands
            }
        }
        self.syncing.set(true);
        let position = self.selected_position().map_or(gtk::INVALID_LIST_POSITION, |p| p as u32);
        self.selection.set_selected(position);
        self.syncing.set(false);
        self.scroll_to_selected();
        self.update_preview();
        self.update_status();
    }

    fn selected_position(&self) -> Option<usize> {
        let id = self.session.borrow().selected()?;
        self.view.borrow().iter().position(|&x| x == id)
    }

    fn scroll_to_selected(&self) {
        if let Some(position) = self.selected_position() {
            self.syncing.set(true);
            self.grid.scroll_to(position as u32, gtk::ListScrollFlags::FOCUS, None);
            self.syncing.set(false);
        }
    }

    fn update_preview(self: &Rc<Self>) {
        let session = self.session.borrow();
        let target = self
            .hovered
            .get()
            .filter(|_| !self.enlarged.get())
            .or(session.selected());
        match target {
            Some(id) => {
                let path = session.image(id).path.clone();
                // Stepping with the keyboard: have both neighbours ready.
                let mut neighbours = Vec::new();
                if session.selected() == Some(id) {
                    let view = self.view.borrow();
                    if let Some(position) = view.iter().position(|&x| x == id) {
                        let around = [position.checked_add(1), position.checked_sub(1)];
                        for &other in around.iter().flatten().filter_map(|&p| view.get(p)) {
                            neighbours.push((other, session.image(other).path.clone()));
                        }
                    }
                }
                self.preview.show(id, path, self.items[id].texture(), neighbours);
                if self.items[id].texture().is_none() && !self.items[id].failed() {
                    let position = self.view.borrow().iter().position(|&x| x == id).unwrap_or(0);
                    self.thumbs.request(id, session.image(id).path.clone(), position, false);
                }
            }
            None => self.preview.clear(),
        }
    }

    fn update_status(&self) {
        let session = self.session.borrow();
        while let Some(child) = self.strip.first_child() {
            self.strip.remove(&child);
        }
        for view in 0..=WORKSPACES {
            let count = if view == 0 { session.images().len() } else { session.workspace_count(view) };
            if view != 0 && count == 0 && session.active() != view {
                continue;
            }
            let text = if view == 0 { format!("all {count}") } else { format!("{view}:{count}") };
            let label = gtk::Label::builder().label(text).css_classes(["ws"]).build();
            if session.active() == view {
                label.add_css_class("active");
            }
            self.strip.append(&label);
        }
        let position = self.selected_position().map_or(0, |p| p + 1);
        let sort = if session.manual_sort() { "manual" } else { "natural" };
        self.info.set_label(&format!("{position}/{}  ·  {sort}", self.view.borrow().len()));
        if session.manual_sort() {
            self.info.add_css_class("manual");
        } else {
            self.info.remove_css_class("manual");
        }
        drop(session);
        self.update_hints();
    }

    /// Keys that matter right now, shown while no message occupies the spot.
    fn update_hints(&self) {
        if self.message_shown.get() {
            return;
        }
        let session = self.session.borrow();
        let in_workspace = session.active() != 0;
        // First, so a narrow window never ellipsizes it away.
        let mut hints: Vec<(&str, &str)> = vec![("?", "keys")];
        if self.view.borrow().is_empty() {
            if in_workspace {
                hints.push(("alt+0", "all images"));
            }
        } else if self.enlarged.get() {
            if self.preview.is_actual_size() {
                hints.extend([("drag", "pan"), ("z", "fit"), ("←→", "compare")]);
            } else {
                hints.extend([("←→", "browse"), ("z", "1:1"), ("1-9", "bin"), ("esc", "back")]);
            }
        } else {
            hints.push(("1-9", if in_workspace { "rebin" } else { "bin" }));
            if session.selected().is_some_and(|id| session.image(id).workspace.is_some()) {
                hints.push(("0", "unbin"));
            }
            if session.manual_sort() {
                hints.extend([("H L", "reorder"), ("s", "unsort")]);
            } else if in_workspace {
                hints.push(("s", "sort"));
            }
            if in_workspace {
                hints.extend([("alt+0", "all"), (":", "actions")]);
            } else {
                hints.extend([("alt+1-9", "show bin"), ("enter", "enlarge"), (":", "commands")]);
            }
        }
        let markup: Vec<String> = hints
            .iter()
            .map(|(key, what)| format!("<b>{}</b> {what}", glib::markup_escape_text(key)))
            .collect();
        self.message.remove_css_class("error");
        self.message.add_css_class("hints");
        self.message.set_markup(&markup.join("   "));
    }

    /// Transient note on the right of the status line.
    pub fn say(self: &Rc<Self>, text: &str, error: bool) {
        self.message_shown.set(true);
        self.message.remove_css_class("hints");
        self.message.set_label(text);
        if error {
            self.message.add_css_class("error");
        } else {
            self.message.remove_css_class("error");
        }
        let generation = self.message_generation.get() + 1;
        self.message_generation.set(generation);
        let weak = Rc::downgrade(self);
        let linger = Duration::from_secs(if error { 12 } else { 6 });
        glib::timeout_add_local_once(linger, move || {
            if let Some(app) = weak.upgrade() {
                if app.message_generation.get() == generation {
                    app.message_shown.set(false);
                    app.update_hints();
                }
            }
        });
    }

    // ── thumbnails ───────────────────────────────────────────────────────

    fn cell_bound(&self, item: &ImageItem, position: usize) {
        let id = item.id() as usize;
        self.bound.borrow_mut().insert(id);
        if !item.sharp() && !item.failed() {
            let path = self.session.borrow().image(id).path.clone();
            self.thumbs.request(id, path, position, item.texture().is_some());
        }
    }

    /// Tell the workers which grid position is mid-viewport.
    fn update_thumb_focus(&self) {
        let adjustment = self.scroller.vadjustment();
        let upper = adjustment.upper().max(1.0);
        let middle = (adjustment.value() + adjustment.page_size() / 2.0) / upper;
        self.thumbs.set_focus((middle * self.view.borrow().len() as f64) as usize);
    }

    fn cell_unbound(&self, item: &ImageItem) {
        let id = item.id() as usize;
        self.bound.borrow_mut().remove(&id);
        self.thumbs.cancel(id);
    }

    fn thumbnail_ready(&self, Thumbnail { id, texture, sharp }: Thumbnail) {
        let item = &self.items[id];
        let Some(texture) = texture else {
            item.set_failed(true);
            return;
        };
        self.preview.offer_placeholder(id, &texture);
        if item.texture().is_none() {
            self.thumb_order.borrow_mut().push_back(id);
        }
        item.set_texture(Some(texture));
        item.set_sharp(sharp);

        let mut order = self.thumb_order.borrow_mut();
        let mut spared = Vec::new();
        while order.len() + spared.len() > THUMB_CACHE {
            let Some(old) = order.pop_front() else { break };
            if self.bound.borrow().contains(&old) {
                spared.push(old);
            } else {
                self.items[old].set_texture(gdk::Texture::NONE);
                self.items[old].set_sharp(false);
            }
        }
        order.extend(spared);
    }

    // ── actions ──────────────────────────────────────────────────────────

    fn hover(self: &Rc<Self>, id: Option<ImageId>) {
        if self.hovered.replace(id) != id && !self.palette.is_open() {
            self.update_preview();
        }
    }

    fn columns(&self) -> usize {
        let mut child = self.grid.first_child();
        while let Some(cell) = child {
            if cell.is_mapped() && cell.width() > 0 {
                let columns = (self.grid.width() as f64 / cell.width() as f64).round();
                return (columns as usize).max(1);
            }
            child = cell.next_sibling();
        }
        1
    }

    fn select_position(self: &Rc<Self>, position: usize) {
        let id = self.view.borrow().get(position).copied();
        if id.is_none() {
            return;
        }
        self.session.borrow_mut().select(id);
        self.hovered.set(None);
        self.sync();
    }

    fn navigate(self: &Rc<Self>, columns: isize, rows: isize) {
        let len = self.view.borrow().len();
        if len == 0 {
            return;
        }
        let Some(current) = self.selected_position() else {
            return self.select_position(0);
        };
        let mut target = current.saturating_add_signed(columns).min(len - 1);
        if rows != 0 {
            let width = if self.enlarged.get() { 1 } else { self.columns() };
            let step = rows * width as isize;
            let moved = current as isize + step;
            if moved >= 0 && (moved as usize) < len {
                target = moved as usize;
            } else if moved > 0 && current / width < (len - 1) / width {
                target = len - 1; // short last row: land on its last image
            }
        }
        if target != current {
            self.select_position(target);
        }
    }

    fn assign(self: &Rc<Self>, workspace: Option<u8>) {
        let Some(id) = self.session.borrow().selected() else { return };
        self.session.borrow_mut().assign(id, workspace);
        self.hovered.set(None);
        self.sync();
    }

    fn show_workspace(self: &Rc<Self>, view: u8) {
        self.session.borrow_mut().set_active(view);
        self.hovered.set(None);
        self.sync();
    }

    fn undo(self: &Rc<Self>, redo: bool) {
        let mut session = self.session.borrow_mut();
        let changed = if redo { session.redo() } else { session.undo() };
        drop(session);
        self.hovered.set(None);
        if changed {
            self.sync();
        }
        let what = if redo { "redo" } else { "undo" };
        self.say(if changed { what } else { "nothing to take back" }, false);
    }

    fn toggle_sort(self: &Rc<Self>) {
        self.session.borrow_mut().toggle_manual_sort();
        self.sync();
    }

    fn move_selected(self: &Rc<Self>, delta: isize) {
        if self.session.borrow_mut().move_selected(delta) {
            self.sync();
        }
    }

    fn reorder_by_drop(self: &Rc<Self>, dragged: ImageId, target: ImageId) {
        let Some(position) = self.view.borrow().iter().position(|&x| x == target) else { return };
        let mut session = self.session.borrow_mut();
        session.move_to(dragged, position);
        session.select(Some(dragged));
        drop(session);
        self.hovered.set(None);
        self.sync();
    }

    fn register_name_label(&self, label: &gtk::Label) {
        label.set_visible(self.show_names.get());
        self.name_labels.borrow_mut().push(label.downgrade());
    }

    fn toggle_names(&self) {
        let show = !self.show_names.get();
        self.show_names.set(show);
        self.name_labels.borrow_mut().retain(|label| {
            label.upgrade().inspect(|l| l.set_visible(show)).is_some()
        });
    }

    /// `z`: actual pixels ↔ fit, enlarging the preview first if need be.
    fn toggle_actual_size(self: &Rc<Self>) {
        if self.preview.is_actual_size() {
            self.preview.fit();
        } else {
            if !self.enlarged.get() {
                self.toggle_enlarged();
            }
            self.preview.actual_size();
        }
        self.update_hints();
    }

    fn toggle_enlarged(self: &Rc<Self>) {
        let enlarged = !self.enlarged.get();
        self.enlarged.set(enlarged);
        self.scroller.set_visible(!enlarged);
        if enlarged {
            self.preview.root.add_css_class("enlarged");
        } else {
            self.preview.root.remove_css_class("enlarged");
            self.preview.fit();
            self.grid.grab_focus();
        }
        self.hovered.set(None);
        self.update_preview();
        self.update_hints();
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(app) = weak.upgrade() {
                app.scroll_to_selected();
            }
        });
    }

    fn key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        use gdk::Key;
        if self.palette.is_open() {
            return glib::Propagation::Proceed;
        }
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = state.contains(gdk::ModifierType::ALT_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let digit = key.to_unicode().and_then(|c| c.to_digit(10)).map(|d| d as u8);

        match (key, digit) {
            (_, Some(d)) if alt && !ctrl => self.show_workspace(d),
            (_, Some(0)) if !ctrl => self.assign(None),
            (_, Some(d)) if !ctrl => self.assign(Some(d)),
            (Key::k, _) if ctrl => self.palette.open_commands(),
            (Key::r, _) if ctrl && !alt => self.undo(true),
            _ if ctrl || alt => return glib::Propagation::Proceed,
            (Key::colon, _) => self.palette.open_commands(),
            (Key::question, _) => self.palette.open_help(),
            (Key::q, _) => self.window.close(),
            (Key::s, _) => self.toggle_sort(),
            (Key::u, _) => self.undo(false),
            (Key::U, _) => self.undo(true),
            (Key::f, _) => self.toggle_names(),
            (Key::Left, _) if shift => self.move_selected(-1),
            (Key::Right, _) if shift => self.move_selected(1),
            (Key::H, _) => self.move_selected(-1),
            (Key::L, _) => self.move_selected(1),
            (Key::Left | Key::h, _) => self.navigate(-1, 0),
            (Key::Right | Key::l, _) => self.navigate(1, 0),
            (Key::Up | Key::k, _) => self.navigate(0, -1),
            (Key::Down | Key::j, _) => self.navigate(0, 1),
            (Key::Home | Key::g, _) => self.select_position(0),
            (Key::End | Key::G, _) => {
                let len = self.view.borrow().len();
                self.select_position(len.saturating_sub(1));
            }
            (Key::Return | Key::KP_Enter | Key::space, _) => self.toggle_enlarged(),
            (Key::z, _) => self.toggle_actual_size(),
            (Key::Escape, _) if self.preview.is_actual_size() => self.toggle_actual_size(),
            (Key::Escape, _) if self.enlarged.get() => self.toggle_enlarged(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    // ── for commands ─────────────────────────────────────────────────────

    /// Paths of the images on screen, in display order.
    fn visible_files(&self) -> Vec<(ImageId, std::path::PathBuf)> {
        let session = self.session.borrow();
        self.view.borrow().iter().map(|&id| (id, session.image(id).path.clone())).collect()
    }

    fn view_name(&self) -> String {
        match self.session.borrow().active() {
            0 => "all images".into(),
            n => format!("workspace {n}"),
        }
    }

    fn selected_name(&self) -> Option<String> {
        let session = self.session.borrow();
        session.selected().map(|id| file_name(&session.image(id).path))
    }
}
