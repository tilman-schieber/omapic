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
    Help,
}

pub struct Palette {
    pub root: gtk::Box,
    title: gtk::Label,
    entry: gtk::Entry,
    list: gtk::ListBox,
    help: gtk::Label,
    hint: gtk::Label,
    mode: Cell<Mode>,
    commands: Vec<&'static str>,
    /// Indices into `commands` currently listed.
    filtered: RefCell<Vec<usize>>,
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
        for w in [title.upcast_ref::<gtk::Widget>(), entry.upcast_ref(), list.upcast_ref(), help.upcast_ref(), hint.upcast_ref()] {
            root.append(w);
        }

        let palette = Rc::new(Palette {
            root,
            title,
            entry,
            list,
            help,
            hint,
            mode: Cell::new(Mode::Closed),
            commands,
            filtered: RefCell::default(),
            reply: RefCell::default(),
            on_command: RefCell::default(),
            focus_return: RefCell::default(),
        });

        let weak = Rc::downgrade(&palette);
        palette.entry.connect_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                if p.mode.get() == Mode::Commands {
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
        let (tx, rx) = async_channel::bounded(1);
        self.reply.replace(Some(tx)); // dropping an older sender cancels that prompt
        self.show(Mode::Prompt { complete_dirs }, title, initial);
        rx.recv().await.ok().flatten()
    }

    /// Yes/no question; an empty answer means `default`.
    pub async fn ask_yes_no(&self, question: &str, default: bool) -> Option<bool> {
        let choices = if default { "[Y/n]" } else { "[y/N]" };
        let answer = self.ask(&format!("{question} {choices}"), "", false).await?;
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
        self.list.set_visible(mode == Mode::Commands);
        self.help.set_visible(mode == Mode::Help);
        self.hint.set_label(match mode {
            Mode::Prompt { complete_dirs: true } => "enter accept · tab complete · esc cancel",
            Mode::Prompt { .. } => "enter accept · esc cancel",
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
        let query = self.entry.text().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let filtered: Vec<usize> = (0..self.commands.len())
            .filter(|&i| {
                let title = self.commands[i].to_lowercase();
                words.iter().all(|w| title.contains(w))
            })
            .collect();
        self.list.remove_all();
        for &i in &filtered {
            let label = gtk::Label::builder().label(self.commands[i]).xalign(0.0).build();
            self.list.append(&label);
        }
        self.list.select_row(self.list.row_at_index(0).as_ref());
        self.filtered.replace(filtered);
    }

    fn move_row(&self, delta: i32) {
        let count = self.filtered.borrow().len() as i32;
        if count == 0 {
            return;
        }
        let current = self.list.selected_row().map_or(0, |r| r.index());
        let next = (current + delta).rem_euclid(count);
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
            Mode::Prompt { .. } => {
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
        match key {
            gdk::Key::Escape => self.close(),
            gdk::Key::Down if mode == Mode::Commands => self.move_row(1),
            gdk::Key::Up if mode == Mode::Commands => self.move_row(-1),
            gdk::Key::n if ctrl && mode == Mode::Commands => self.move_row(1),
            gdk::Key::p if ctrl && mode == Mode::Commands => self.move_row(-1),
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => match mode {
                Mode::Commands => self.move_row(if key == gdk::Key::Tab { 1 } else { -1 }),
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
