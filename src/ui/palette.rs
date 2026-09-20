//! The one floating surface of the app: command list, text prompts and the
//! shortcut sheet all appear in the same centred box over the grid.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::fsops;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Closed,
    Commands,
    Prompt { complete_dirs: bool },
    /// A prompt with a list of lines to pick from: history and suggestions.
    Pick,
    Help,
}

pub struct Palette {
    pub root: gtk::Box,
    title: gtk::Label,
    entry: gtk::Entry,
    list: gtk::ListBox,
    help: gtk::Label,
    /// What a confirmation is about to do, e.g. the first lines of a plan.
    detail: gtk::Label,
    hint: gtk::Label,
    mode: Cell<Mode>,
    commands: Vec<&'static str>,
    /// Indices into `commands` currently listed.
    filtered: RefCell<Vec<usize>>,
    /// Lines offered in `Pick` mode: (text, note).
    offers: RefCell<Vec<(String, String)>>,
    reply: RefCell<Option<async_channel::Sender<Option<String>>>>,
    on_command: RefCell<Option<Box<dyn Fn(usize)>>>,
    focus_return: RefCell<Option<gtk::Widget>>,
}

impl Palette {
    pub fn new(commands: Vec<&'static str>, help_markup: &str) -> Rc<Self> {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .margin_top(96)
            .width_request(620)
            .visible(false)
            .focusable(true)
            .css_classes(["palette"])
            .build();
        let title = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["palette-title"]).build();
        let entry = gtk::Entry::builder().has_frame(false).css_classes(["palette-entry"]).build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Browse)
            .css_classes(["palette-list"])
            .build();
        let help = gtk::Label::builder().xalign(0.0).use_markup(true).css_classes(["palette-help"]).build();
        help.set_markup(help_markup);
        let hint = gtk::Label::builder().xalign(0.0).css_classes(["palette-hint"]).build();
        let detail = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["palette-detail"])
            .build();
        let parts: [&gtk::Widget; 6] =
            [title.upcast_ref(), detail.upcast_ref(), entry.upcast_ref(), list.upcast_ref(), help.upcast_ref(), hint.upcast_ref()];
        for w in parts {
            root.append(w);
        }

        let palette = Rc::new(Palette {
            root,
            title,
            entry,
            list,
            help,
            detail,
            hint,
            mode: Cell::new(Mode::Closed),
            commands,
            filtered: RefCell::default(),
            offers: RefCell::default(),
            reply: RefCell::default(),
            on_command: RefCell::default(),
            focus_return: RefCell::default(),
        });

        let weak = Rc::downgrade(&palette);
        palette.entry.connect_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                if matches!(p.mode.get(), Mode::Commands | Mode::Pick) {
                    p.refilter();
                }
            }
        });
        let weak = Rc::downgrade(&palette);
        palette.entry.connect_activate(move |_| {
            if let Some(p) = weak.upgrade() {
                p.accept();
            }
        });
        let weak = Rc::downgrade(&palette);
        palette.list.connect_row_activated(move |_, _| {
            if let Some(p) = weak.upgrade() {
                p.accept();
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&palette);
        keys.connect_key_pressed(move |_, key, _, state| {
            weak.upgrade().map_or(glib::Propagation::Proceed, |p| p.key(key, state))
        });
        palette.root.add_controller(keys);
        palette
    }

    pub fn set_focus_return(&self, widget: &impl IsA<gtk::Widget>) {
        self.focus_return.replace(Some(widget.clone().upcast()));
    }

    pub fn connect_command(&self, f: impl Fn(usize) + 'static) {
        self.on_command.replace(Some(Box::new(f)));
    }

    pub fn is_open(&self) -> bool {
        self.mode.get() != Mode::Closed
    }

    pub fn open_commands(&self) {
        self.show(Mode::Commands, "", "");
        self.refilter();
    }

    pub fn open_help(&self) {
        self.show(Mode::Help, "keys", "");
        self.root.grab_focus();
    }

    /// Ask for a line of text. Resolves to `None` when cancelled.
    pub async fn ask(&self, title: &str, initial: &str, complete_dirs: bool) -> Option<String> {
        self.ask_about(title, "", initial, complete_dirs).await
    }

    /// Like `ask`, with `detail` lines under the title.
    pub async fn ask_about(&self, title: &str, detail: &str, initial: &str, complete_dirs: bool) -> Option<String> {
        let (tx, rx) = async_channel::bounded(1);
        self.reply.replace(Some(tx)); // dropping an older sender cancels that prompt
        self.show(Mode::Prompt { complete_dirs }, title, initial);
        self.detail.set_label(detail);
        self.detail.set_visible(!detail.is_empty());
        rx.recv().await.ok().flatten()
    }

    /// Ask for a line, offering `offers` (text, note) underneath: typing
    /// narrows them down, picking one puts it into the entry for editing.
    pub async fn ask_with_offers(&self, title: &str, detail: &str, offers: Vec<(String, String)>) -> Option<String> {
        let (tx, rx) = async_channel::bounded(1);
        self.reply.replace(Some(tx));
        self.offers.replace(offers);
        self.show(Mode::Pick, title, "");
        self.detail.set_label(detail);
        self.detail.set_visible(!detail.is_empty());
        self.refilter();
        rx.recv().await.ok().flatten()
    }

    /// Yes/no question; an empty answer means `default`.
    pub async fn ask_yes_no(&self, question: &str, default: bool) -> Option<bool> {
        self.confirm(question, "", default).await
    }

    /// Yes/no with `detail` lines spelling out what would happen.
    pub async fn confirm(&self, question: &str, detail: &str, default: bool) -> Option<bool> {
        let choices = if default { "[Y/n]" } else { "[y/N]" };
        let answer = self.ask_about(&format!("{question} {choices}"), detail, "", false).await?;
        Some(match answer.trim().to_lowercase().as_str() {
            "" => default,
            "y" | "yes" | "j" | "ja" => true,
            _ => false,
        })
    }

    fn show(&self, mode: Mode, title: &str, text: &str) {
        self.mode.set(mode);
        self.title.set_label(title);
        self.title.set_visible(!title.is_empty());
        self.entry.set_visible(mode != Mode::Help);
        self.list.set_visible(matches!(mode, Mode::Commands | Mode::Pick));
        self.help.set_visible(mode == Mode::Help);
        self.detail.set_visible(false);
        self.hint.set_label(match mode {
            Mode::Prompt { complete_dirs: true } => "enter accept · tab complete · esc cancel",
            Mode::Prompt { .. } => "enter accept · esc cancel",
            Mode::Pick => "↑↓ pick a line to edit · enter run · esc cancel",
            Mode::Help => "any key to close",
            _ => "",
        });
        self.hint.set_visible(!self.hint.label().is_empty());
        self.entry.set_placeholder_text((mode == Mode::Commands).then_some("command"));
        self.entry.set_text(text);
        self.root.set_visible(true);
        if mode != Mode::Help {
            self.entry.grab_focus();
            self.entry.set_position(-1);
        }
    }

    pub fn close(&self) {
        self.mode.set(Mode::Closed);
        self.reply.take();
        self.root.set_visible(false);
        if let Some(w) = &*self.focus_return.borrow() {
            w.grab_focus();
        }
    }

    fn refilter(&self) {
        const OFFERED: usize = 12;
        let query = self.entry.text().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let matches = |text: &str| {
            let text = text.to_lowercase();
            words.iter().all(|w| text.contains(w))
        };
        self.list.remove_all();
        let filtered: Vec<usize> = if self.mode.get() == Mode::Pick {
            let offers = self.offers.borrow();
            let shown: Vec<usize> = (0..offers.len())
                .filter(|&i| matches(&format!("{} {}", offers[i].0, offers[i].1)) && offers[i].0 != self.entry.text())
                .take(OFFERED)
                .collect();
            for &i in &shown {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
                let text = gtk::Label::builder().label(&offers[i].0).xalign(0.0).hexpand(true).build();
                text.set_ellipsize(gtk::pango::EllipsizeMode::End);
                let note = gtk::Label::builder().label(&offers[i].1).css_classes(["palette-note"]).build();
                row.append(&text);
                row.append(&note);
                self.list.append(&row);
            }
            self.list.unselect_all(); // nothing is picked until asked for
            shown
        } else {
            let shown: Vec<usize> = (0..self.commands.len()).filter(|&i| matches(self.commands[i])).collect();
            for &i in &shown {
                let label = gtk::Label::builder().label(self.commands[i]).xalign(0.0).build();
                self.list.append(&label);
            }
            self.list.select_row(self.list.row_at_index(0).as_ref());
            shown
        };
        self.filtered.replace(filtered);
    }

    fn move_row(&self, delta: i32) {
        let count = self.filtered.borrow().len() as i32;
        if count == 0 {
            return;
        }
        let next = match self.list.selected_row() {
            Some(row) => (row.index() + delta).rem_euclid(count),
            None if delta > 0 => 0,
            None => count - 1,
        };
        self.list.select_row(self.list.row_at_index(next).as_ref());
    }

    fn accept(&self) {
        match self.mode.get() {
            Mode::Commands => {
                let index = self.list.selected_row().map(|r| r.index() as usize);
                let command = index.and_then(|i| self.filtered.borrow().get(i).copied());
                self.close();
                if let (Some(command), Some(f)) = (command, &*self.on_command.borrow()) {
                    f(command);
                }
            }
            Mode::Pick if self.list.selected_row().is_some() => {
                // Picked lines go into the entry to be looked at, not straight to the shell.
                let picked = self.list.selected_row().map(|r| r.index() as usize);
                let index = picked.and_then(|i| self.filtered.borrow().get(i).copied());
                let text = index.map(|i| self.offers.borrow()[i].0.clone()).unwrap_or_default();
                self.entry.set_text(&text);
                self.entry.set_position(-1);
            }
            Mode::Prompt { .. } | Mode::Pick => {
                let reply = self.reply.take();
                let text = self.entry.text().to_string();
                self.close();
                if let Some(tx) = reply {
                    let _ = tx.try_send(Some(text));
                }
            }
            _ => self.close(),
        }
    }

    fn key(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let mode = self.mode.get();
        if mode == Mode::Help {
            let modifier_only = matches!(
                key,
                gdk::Key::Shift_L | gdk::Key::Shift_R | gdk::Key::Control_L | gdk::Key::Control_R
                    | gdk::Key::Alt_L | gdk::Key::Alt_R | gdk::Key::Super_L | gdk::Key::Super_R
                    | gdk::Key::ISO_Level3_Shift
            );
            if !modifier_only {
                self.close();
            }
            return glib::Propagation::Stop;
        }
        let listing = matches!(mode, Mode::Commands | Mode::Pick);
        match key {
            gdk::Key::Escape => self.close(),
            gdk::Key::Down if listing => self.move_row(1),
            gdk::Key::Up if listing => self.move_row(-1),
            gdk::Key::n if ctrl && listing => self.move_row(1),
            gdk::Key::p if ctrl && listing => self.move_row(-1),
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => match mode {
                Mode::Commands | Mode::Pick => self.move_row(if key == gdk::Key::Tab { 1 } else { -1 }),
                Mode::Prompt { complete_dirs: true } => {
                    if let Some(done) = fsops::complete_dir(&self.entry.text()) {
                        self.entry.set_text(&done);
                        self.entry.set_position(-1);
                    }
                }
                _ => {}
            },
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    #[cfg(debug_assertions)]
    pub fn debug_text(&self, text: &str) {
        self.entry.set_text(text);
    }

    #[cfg(debug_assertions)]
    pub fn debug_answer(&self, text: &str) {
        self.entry.set_text(text);
        self.accept();
    }

    #[cfg(debug_assertions)]
    pub fn debug_key(&self, key: gdk::Key, state: gdk::ModifierType) {
        if key == gdk::Key::Return {
            self.accept();
        } else {
            self.key(key, state);
        }
    }
}
