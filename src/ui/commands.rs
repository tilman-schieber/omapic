//! Palette commands. Each is a linear conversation: ask, plan, confirm, run.
//! Filesystem work happens in `fsops`/`montage`, off the main thread.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gio;

use super::App;
use crate::fsops::{self, Op};
use crate::montage::{self, ContactSheet};
use crate::theme::Theme;

const COMMANDS: &[&str] = &[
    "Move workspace to folder…",
    "Copy workspace to folder…",
    "Rename files in workspace according to current order…",
    "Create contact sheet…",
    "Open containing folder",
    "Remove selected image from workspace",
    "Keyboard shortcuts",
];

pub fn titles() -> Vec<&'static str> {
    COMMANDS.to_vec()
}

pub async fn run(app: Rc<App>, index: usize) {
    let result = match index {
        0 => transfer(&app, Op::Move).await,
        1 => transfer(&app, Op::Copy).await,
        2 => rename(&app).await,
        3 => contact_sheet(&app).await,
        4 => open_folder(&app),
        5 => {
            app.assign(None);
            Ok(None)
        }
        _ => {
            app.palette.open_help();
            Ok(None)
        }
    };
    match result {
        Ok(Some(done)) => app.say(&done, false),
        Ok(None) => {}
        Err(problem) => app.say(&problem, true),
    }
}

/// `Ok(None)` = cancelled or nothing to report.
type Outcome = Result<Option<String>, String>;

fn default_dir(files: &[(usize, PathBuf)]) -> String {
    let dir = files.first().and_then(|(_, p)| p.parent()).unwrap_or(Path::new("/"));
    format!("{}/", dir.display().to_string().trim_end_matches('/'))
}

async fn transfer(app: &Rc<App>, op: Op) -> Outcome {
    let files = app.visible_files();
    if files.is_empty() {
        return Err(format!("{} is empty", app.view_name()));
    }
    let (verb, done) = if op == Op::Move { ("Move", "moved") } else { ("Copy", "copied") };
    let title = format!("{verb} {} ({} files) to folder", app.view_name(), files.len());
    let Some(answer) = app.palette.ask(&title, &default_dir(&files), true).await else {
        return Ok(None);
    };
    if answer.trim().is_empty() {
        return Ok(None);
    }
    let dest = fsops::expand(&answer);
    if dest.exists() && !dest.is_dir() {
        return Err(format!("not a folder: {}", dest.display()));
    }

    let mut prefix = false;
    if app.session.borrow().manual_sort() {
        let question = "Keep the manual order with numeric prefixes (001_name.jpg)?";
        let Some(answer) = app.palette.ask_yes_no(question, true).await else { return Ok(None) };
        prefix = answer;
    }

    let paths: Vec<PathBuf> = files.iter().map(|(_, p)| p.clone()).collect();
    let plan = fsops::plan(op, &paths, &dest, prefix)?;
    let created = if dest.is_dir() { "" } else { " (will be created)" };
    let question = format!("{verb} {} files to {}{created}?", paths.len(), dest.display());
    if app.palette.ask_yes_no(&question, false).await != Some(true) {
        return Ok(None);
    }
    finish(app, plan, &files, done).await
}

async fn rename(app: &Rc<App>) -> Outcome {
    let files = app.visible_files();
    if files.is_empty() {
        return Err(format!("{} is empty", app.view_name()));
    }
    let paths: Vec<PathBuf> = files.iter().map(|(_, p)| p.clone()).collect();
    let plan = fsops::plan(Op::Rename, &paths, Path::new(""), true)?;
    let example = plan.steps[0].dst.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let order = if app.session.borrow().manual_sort() { "manual" } else { "natural" };
    let question = format!(
        "Rename {} files of {} in place, in {order} order ({example}, …)?",
        paths.len(),
        app.view_name()
    );
    if app.palette.ask_yes_no(&question, false).await != Some(true) {
        return Ok(None);
    }
    finish(app, plan, &files, "renamed").await
}

/// Run a checked plan and teach the session where the files went.
async fn finish(app: &Rc<App>, plan: fsops::Plan, files: &[(usize, PathBuf)], done: &str) -> Outcome {
    app.say("working…", false);
    let (plan, outcome) = gio::spawn_blocking(move || {
        let outcome = fsops::execute(&plan);
        (plan, outcome)
    })
    .await
    .map_err(|_| "file operation crashed".to_string())?;

    if plan.op != Op::Copy {
        let mut session = app.session.borrow_mut();
        for &i in &outcome.done {
            session.set_path(files[i].0, plan.steps[i].dst.clone());
        }
    }
    app.sync();
    let count = outcome.done.len();
    match outcome.error {
        Some(error) => Err(format!("{done} {count} of {}, then stopped: {error}", files.len())),
        None => {
            let place = plan.steps[0].dst.parent().map(|p| p.display().to_string()).unwrap_or_default();
            Ok(Some(format!("{done} {count} files → {place}")))
        }
    }
}

fn free_name(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    (1..)
        .map(|n| match n {
            1 => dir.join(format!("{stem}.{extension}")),
            n => dir.join(format!("{stem}-{n}.{extension}")),
        })
        .find(|p| p.symlink_metadata().is_err())
        .unwrap()
}

async fn contact_sheet(app: &Rc<App>) -> Outcome {
    let files = app.visible_files();
    if files.is_empty() {
        return Err(format!("{} is empty", app.view_name()));
    }
    let palette = &app.palette;
    let suggestion = free_name(Path::new(&default_dir(&files)), "contact-sheet", "jpg");
    let title = format!("Contact sheet of {} ({} images) — output file", app.view_name(), files.len());
    let Some(output) = palette.ask(&title, &suggestion.display().to_string(), true).await else {
        return Ok(None);
    };
    let output = fsops::expand(&output);
    if output.symlink_metadata().is_ok() {
        return Err(format!("exists: {}", output.display()));
    }
    let Some(columns) = palette.ask("Columns", "6", false).await else { return Ok(None) };
    let columns: u32 = columns.trim().parse().ok().filter(|c| (1..=100).contains(c)).ok_or("columns: 1–100")?;
    let Some(size) = palette.ask("Thumbnail size (px)", "256", false).await else { return Ok(None) };
    let size: u32 = size.trim().parse().ok().filter(|s| (16..=4096).contains(s)).ok_or("size: 16–4096 px")?;
    let Some(labels) = palette.ask_yes_no("Label thumbnails with file names?", true).await else {
        return Ok(None);
    };

    let theme = Theme::load();
    let sheet = ContactSheet {
        output: output.clone(),
        columns,
        thumb_size: size,
        labels,
        background: theme.color("background").to_string(),
        foreground: theme.color("foreground").to_string(),
        font: montage::label_font(),
    };
    let paths: Vec<PathBuf> = files.into_iter().map(|(_, p)| p).collect();
    app.say("magick montage…", false);
    gio::spawn_blocking(move || sheet.run(&paths))
        .await
        .map_err(|_| "montage crashed".to_string())??;
    Ok(Some(format!("contact sheet → {}", output.display())))
}

fn open_folder(app: &Rc<App>) -> Outcome {
    let session = app.session.borrow();
    let Some(id) = session.selected() else { return Err("no image selected".into()) };
    let file = gio::File::for_path(&session.image(id).path);
    gtk::FileLauncher::new(Some(&file)).open_containing_folder(
        Some(&app.window),
        gio::Cancellable::NONE,
        |_| {},
    );
    Ok(Some(format!("opened folder of {}", app.selected_name().unwrap_or_default())))
}
