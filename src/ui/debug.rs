//! Debug builds only: drive the UI from `OMAPIC_SCRIPT` for smoke tests.
//!
//! Space-separated steps, run 400 ms apart:
//!   `j`, `alt+1`, `shift+Right`, `colon` …  a key press (GDK key names)
//!   `text:TEXT`                            put TEXT into the palette entry (`_` = space)
//!   `type:TEXT`                            the same, and accept
//!   `drop:PATH`                            as if PATH were dropped on the window
//!   `snap:FILE.png`                        render the window into a PNG
//!
//! `OMAPIC_SCRIPT_START` (ms, default 2500) delays the first step.

use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib, graphene};

use super::App;

pub fn run_script(app: &Rc<App>) {
    let Ok(script) = std::env::var("OMAPIC_SCRIPT") else { return };
    let app = app.clone();
    glib::spawn_future_local(async move {
        let start = std::env::var("OMAPIC_SCRIPT_START").ok().and_then(|s| s.parse().ok()).unwrap_or(2500);
        glib::timeout_future(Duration::from_millis(start)).await;
        for step in script.split_whitespace() {
            if let Some(path) = step.strip_prefix("snap:") {
                snapshot(&app, path);
            } else if let Some(path) = step.strip_prefix("drop:") {
                app.dropped(vec![path.into()]);
            } else if let Some(text) = step.strip_prefix("text:") {
                app.palette.debug_text(&text.replace('_', " "));
            } else if let Some(text) = step.strip_prefix("type:") {
                app.palette.debug_answer(&text.replace('_', " "));
            } else {
                press(&app, step);
            }
            glib::timeout_future(Duration::from_millis(400)).await;
        }
    });
}

fn press(app: &Rc<App>, step: &str) {
    let mut state = gdk::ModifierType::empty();
    let mut name = step;
    for (prefix, modifier) in [
        ("alt+", gdk::ModifierType::ALT_MASK),
        ("shift+", gdk::ModifierType::SHIFT_MASK),
        ("ctrl+", gdk::ModifierType::CONTROL_MASK),
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            state |= modifier;
            name = rest;
        }
    }
    match gdk::Key::from_name(name) {
        Some(key) if app.palette.is_open() => app.palette.debug_key(key, state),
        Some(key) => {
            app.key(key, state);
        }
        None => eprintln!("omapic script: unknown key {name}"),
    }
}

fn snapshot(app: &Rc<App>, path: &str) {
    let (width, height) = (app.window.width() as f32, app.window.height() as f32);
    let paintable = gtk::WidgetPaintable::new(Some(&app.window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let (Some(node), Some(renderer)) = (snapshot.to_node(), app.window.renderer()) else {
        return eprintln!("omapic script: nothing to render");
    };
    let bounds = graphene::Rect::new(0.0, 0.0, width, height);
    match renderer.render_texture(&node, Some(&bounds)).save_to_png(path) {
        Ok(()) => eprintln!("omapic script: wrote {path}"),
        Err(e) => eprintln!("omapic script: {e}"),
    }
}
