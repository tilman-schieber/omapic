//! Palette commands. Each is a linear conversation: ask, plan, confirm, run.
//! Filesystem work happens in `fsops`/`montage`, off the main thread.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gio;

use super::App;
use crate::cli::file_name;
use crate::fsops::{self, Op};
use crate::montage::{self, ContactSheet};
use crate::{history, shell};
use crate::theme::Theme;

#[derive(Clone, Copy)]
enum Command {
    Transfer(Op),
    Rename,
    ContactSheet,
    SaveRotations,
    Shell,
    Trash,
    OpenFolder,
    Unbin,
    Help,
}

const COMMANDS: &[(&str, Command)] = &[
    ("Move workspace to folder…", Command::Transfer(Op::Move)),
    ("Copy workspace to folder…", Command::Transfer(Op::Copy)),
    ("Symlink workspace into folder…", Command::Transfer(Op::Symlink)),
    ("Hard-link workspace into folder…", Command::Transfer(Op::Hardlink)),
    ("Rename files in workspace according to current order…", Command::Rename),
    ("Create contact sheet…", Command::ContactSheet),
    ("Save rotations to files…", Command::SaveRotations),
    ("Run shell command on workspace…", Command::Shell),
    ("Move workspace to trash…", Command::Trash),
    ("Open containing folder", Command::OpenFolder),
    ("Remove selected image from workspace", Command::Unbin),
    ("Keyboard shortcuts", Command::Help),
];

pub fn titles() -> Vec<&'static str> {
    COMMANDS.iter().map(|(title, _)| *title).collect()
}

pub async fn run(app: Rc<App>, index: usize) {
    if let Some(&(_, command)) = COMMANDS.get(index) {
        execute(app, command).await;
    }
}

/// `!` goes straight to the shell prompt.
pub async fn run_shell(app: Rc<App>) {
    execute(app, Command::Shell).await;
}

async fn execute(app: Rc<App>, command: Command) {
    let result = match command {
        Command::Shell => shell(&app).await,
        Command::Transfer(op) => transfer(&app, op).await,
        Command::Rename => rename(&app).await,
        Command::ContactSheet => contact_sheet(&app).await,
        Command::SaveRotations => save_rotations(&app).await,
        Command::Trash => trash(&app).await,
        Command::OpenFolder => open_folder(&app),
        Command::Unbin => {
            app.assign(None);
            Ok(None)
        }
        Command::Help => {
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

/// The first lines of a plan, for the confirmation prompt.
fn preview(plan: &fsops::Plan) -> String {
    const SHOWN: usize = 5;
    let mut lines: Vec<String> = plan
        .steps
        .iter()
        .take(SHOWN)
        .map(|step| format!("{}  →  {}", file_name(&step.src), file_name(&step.dst)))
        .collect();
    if plan.steps.len() > SHOWN {
        lines.push(format!("… and {} more", plan.steps.len() - SHOWN));
    }
    lines.join("\n")
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
    let (verb, done) = match op {
        Op::Move => ("Move", "moved"),
        Op::Symlink => ("Symlink", "symlinked"),
        Op::Hardlink => ("Hard-link", "hard-linked"),
        _ => ("Copy", "copied"),
    };
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
    if app.palette.confirm(&question, &preview(&plan), false).await != Some(true) {
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
    if app.palette.confirm(&question, &preview(&plan), false).await != Some(true) {
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

    if matches!(plan.op, Op::Move | Op::Rename) {
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

/// Write the pending rotations of the shown images into their files.
async fn save_rotations(app: &Rc<App>) -> Outcome {
    let jobs: Vec<(usize, PathBuf, u8)> = {
        let session = app.session.borrow();
        let turned = |id: usize| Some(session.image(id).rotation).filter(|&r| r != 0);
        app.visible_files().into_iter().filter_map(|(id, path)| Some((id, path, turned(id)?))).collect()
    };
    if jobs.is_empty() {
        return Err(format!("no unsaved rotations in {} (r / R rotate)", app.view_name()));
    }
    let question = format!(
        "Write the rotation of {} JPEGs in {} to the files (EXIF orientation, lossless)?",
        jobs.len(),
        app.view_name()
    );
    if app.palette.ask_yes_no(&question, false).await != Some(true) {
        return Ok(None);
    }
    let results = gio::spawn_blocking(move || {
        jobs.into_iter().map(|(id, path, turns)| (id, fsops::save_rotation(&path, turns))).collect::<Vec<_>>()
    })
    .await
    .map_err(|_| "rotation crashed".to_string())?;

    let saved: Vec<usize> = results.iter().filter(|(_, r)| r.is_ok()).map(|(id, _)| *id).collect();
    app.rotations_saved(&saved);
    match results.into_iter().find_map(|(_, r)| r.err()) {
        Some(problem) => Err(format!("saved {}, but {problem}", saved.len())),
        None => Ok(Some(format!("rotation of {} files saved", saved.len()))),
    }
}

/// Run a shell command over the shown images, per file or all at once.
async fn shell(app: &Rc<App>) -> Outcome {
    let files = app.visible_files();
    if files.is_empty() {
        return Err(format!("{} is empty", app.view_name()));
    }
    let history_file = history::default_path();
    let mut offers: Vec<(String, String)> = history_file
        .as_deref()
        .map(history::load)
        .unwrap_or_default()
        .into_iter()
        .map(|line| (line, "history".to_string()))
        .collect();
    let suggestions = shell::SUGGESTIONS.iter().filter(|(line, _)| offers.iter().all(|(h, _)| h != line));
    let suggestions: Vec<_> = suggestions.map(|(line, note)| (line.to_string(), note.to_string())).collect();
    offers.extend(suggestions);

    let title = format!("Shell command on {} ({} files, in display order)", app.view_name(), files.len());
    let Some(template) = app.palette.ask_with_offers(&title, shell::PLACEHOLDERS, offers).await else {
        return Ok(None);
    };
    if template.trim().is_empty() {
        return Ok(None);
    }
    let paths: Vec<PathBuf> = files.iter().map(|(_, p)| p.clone()).collect();
    let jobs = shell::expand(&template, &paths)?;

    const SHOWN: usize = 3;
    let mut lines: Vec<String> = jobs.iter().take(SHOWN).map(|job| job.line.clone()).collect();
    if jobs.len() > SHOWN {
        lines.push(format!("… and {} more", jobs.len() - SHOWN));
    }
    lines.push(format!("in {}", jobs[0].cwd.display()));
    let runs = if jobs.len() == 1 { "Run this command?".to_string() } else { format!("Run these {} commands?", jobs.len()) };
    if app.palette.confirm(&runs, &lines.join("\n"), false).await != Some(true) {
        return Ok(None);
    }
    if let Some(file) = &history_file {
        let _ = history::remember(file, &template); // convenience only
    }

    app.say("running…", false);
    let total = jobs.len();
    let failures = gio::spawn_blocking(move || {
        jobs.iter().filter_map(|job| shell::run(job).err()).collect::<Vec<String>>()
    })
    .await
    .map_err(|_| "shell command crashed".to_string())?;

    let ids: Vec<usize> = files.iter().map(|(id, _)| *id).collect();
    app.files_changed(&ids);
    match failures.first() {
        Some(first) => Err(format!("{} of {total} failed: {first}", failures.len())),
        None if total == 1 => Ok(Some("command finished".into())),
        None => Ok(Some(format!("{total} commands finished"))),
    }
}

/// Names only, for confirmations that have no target side.
fn name_list(files: &[(usize, PathBuf)]) -> String {
    const SHOWN: usize = 5;
    let mut lines: Vec<String> = files.iter().take(SHOWN).map(|(_, p)| file_name(p)).collect();
    if files.len() > SHOWN {
        lines.push(format!("… and {} more", files.len() - SHOWN));
    }
    lines.join("\n")
}

async fn trash(app: &Rc<App>) -> Outcome {
    let files = app.visible_files();
    if files.is_empty() {
        return Err(format!("{} is empty", app.view_name()));
    }
    let question = format!("Move the {} files of {} to the trash?", files.len(), app.view_name());
    if app.palette.confirm(&question, &name_list(&files), false).await != Some(true) {
        return Ok(None);
    }
    let paths: Vec<PathBuf> = files.iter().map(|(_, p)| p.clone()).collect();
    let outcome = gio::spawn_blocking(move || fsops::trash(&paths)).await.map_err(|_| "trash crashed".to_string())?;
    let gone: Vec<usize> = outcome.done.iter().map(|&i| files[i].0).collect();
    app.session.borrow_mut().remove(&gone);
    app.sync();
    match outcome.error {
        Some(error) => Err(format!("trashed {} of {}: {error}", gone.len(), files.len())),
        None => Ok(Some(format!("{} files moved to the trash", gone.len()))),
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
