//! The only module that modifies files. Every operation is planned and
//! checked for collisions in full before the first file is touched.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Move,
    Copy,
    /// Symbolic links in the folder, pointing at the originals.
    Symlink,
    /// Hard links: second names for the same data, same filesystem only.
    Hardlink,
    /// Rename in place, each file staying in its own directory.
    Rename,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub src: PathBuf,
    pub dst: PathBuf,
}

#[derive(Debug)]
pub struct Plan {
    pub op: Op,
    pub steps: Vec<Step>,
}

/// `001_`-style prefix width: enough digits for `count`, at least three.
pub fn prefix_width(count: usize) -> usize {
    count.to_string().len().max(3)
}

/// Drop a previous order prefix (three or more digits and an underscore),
/// so re-exporting doesn't stack prefixes.
pub fn strip_order_prefix(name: &str) -> &str {
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    match name[digits..].strip_prefix('_') {
        Some(rest) if digits >= 3 && !rest.is_empty() => rest,
        _ => name,
    }
}

/// Build the plan for `files` (in display order) and verify it can run
/// without overwriting anything. `dest` is ignored for `Op::Rename`.
pub fn plan(op: Op, files: &[PathBuf], dest: &Path, order_prefix: bool) -> Result<Plan, String> {
    if files.is_empty() {
        return Err("no images in this workspace".into());
    }
    let width = prefix_width(files.len());
    let mut steps = Vec::with_capacity(files.len());
    for (i, src) in files.iter().enumerate() {
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("unusable file name: {}", src.display()))?;
        let name = if order_prefix {
            format!("{:0width$}_{}", i + 1, strip_order_prefix(name))
        } else {
            name.to_string()
        };
        let dir = match op {
            Op::Rename => src.parent().unwrap_or(Path::new("/")),
            _ => dest,
        };
        steps.push(Step { src: src.clone(), dst: dir.join(name) });
    }
    check(op, &steps)?;
    Ok(Plan { op, steps })
}

fn check(op: Op, steps: &[Step]) -> Result<(), String> {
    let mut problems = Vec::new();
    let sources: HashSet<&Path> = steps.iter().map(|s| s.src.as_path()).collect();
    let mut targets: HashMap<&Path, &Path> = HashMap::new();
    for step in steps {
        if !step.src.is_file() {
            problems.push(format!("missing: {}", step.src.display()));
        }
        if let Some(other) = targets.insert(&step.dst, &step.src) {
            problems.push(format!(
                "{} and {} would both become {}",
                other.display(),
                step.src.display(),
                step.dst.display()
            ));
        }
        if step.dst == step.src {
            if op != Op::Rename {
                problems.push(format!("{} is already there", step.src.display()));
            }
            continue;
        }
        // A rename may land on a name that another step is about to vacate.
        let vacated = op == Op::Rename && sources.contains(step.dst.as_path());
        if step.dst.symlink_metadata().is_ok() && !vacated {
            problems.push(format!("exists: {}", step.dst.display()));
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    let more = problems.len().saturating_sub(3);
    let mut msg = problems.into_iter().take(3).collect::<Vec<_>>().join("; ");
    if more > 0 {
        msg.push_str(&format!("; and {more} more"));
    }
    Err(msg)
}

pub struct Outcome {
    /// Steps that completed, as (index into plan.steps). On error the rest are untouched.
    pub done: Vec<usize>,
    pub error: Option<String>,
}

pub fn execute(plan: &Plan) -> Outcome {
    let mut done = Vec::new();
    let error = run(plan, &mut done).err().map(|e| e.to_string());
    Outcome { done, error }
}

fn run(plan: &Plan, done: &mut Vec<usize>) -> io::Result<()> {
    if plan.op != Op::Rename {
        if let Some(dir) = plan.steps.first().and_then(|s| s.dst.parent()) {
            fs::create_dir_all(dir)?;
        }
    }
    let sources: HashSet<&Path> = plan.steps.iter().map(|s| s.src.as_path()).collect();
    let overlapping = plan.op == Op::Rename
        && plan.steps.iter().any(|s| s.dst != s.src && sources.contains(s.dst.as_path()));
    if overlapping {
        return rename_two_phase(plan, done);
    }
    for (i, step) in plan.steps.iter().enumerate() {
        if step.src != step.dst {
            match plan.op {
                Op::Copy => copy_new(&step.src, &step.dst)?,
                // Both refuse to replace an existing name.
                Op::Symlink => std::os::unix::fs::symlink(&step.src, &step.dst)?,
                Op::Hardlink => fs::hard_link(&step.src, &step.dst)?,
                Op::Move | Op::Rename => move_new(&step.src, &step.dst)?,
            }
        }
        done.push(i);
    }
    Ok(())
}

/// Targets overlap sources (e.g. re-numbering): park everything under
/// temporary names first. If parking fails it is rolled back.
fn rename_two_phase(plan: &Plan, done: &mut Vec<usize>) -> io::Result<()> {
    let pid = std::process::id();
    let mut parked: Vec<(usize, PathBuf)> = Vec::new();
    for (i, step) in plan.steps.iter().enumerate() {
        if step.src == step.dst {
            continue;
        }
        let tmp = step.src.with_file_name(format!(".omapic-{pid}-{i}.tmp"));
        if let Err(e) = move_new(&step.src, &tmp) {
            for (j, tmp) in parked.iter().rev() {
                let _ = fs::rename(tmp, &plan.steps[*j].src);
            }
            return Err(e);
        }
        parked.push((i, tmp));
    }
    let mut first_error = None;
    for (i, tmp) in parked {
        let step = &plan.steps[i];
        match move_new(&tmp, &step.dst) {
            Ok(()) => done.push(i),
            Err(e) => {
                let _ = fs::rename(&tmp, &step.src); // put it back under its old name
                first_error.get_or_insert(e);
            }
        }
    }
    done.extend(plan.steps.iter().enumerate().filter(|(_, s)| s.src == s.dst).map(|(i, _)| i));
    first_error.map_or(Ok(()), Err)
}

fn exists_error(path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, format!("exists: {}", path.display()))
}

/// Copy without ever overwriting: the target is created with `create_new`.
fn copy_new(src: &Path, dst: &Path) -> io::Result<()> {
    let mut input = fs::File::open(src)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .map_err(|e| if e.kind() == io::ErrorKind::AlreadyExists { exists_error(dst) } else { e })?;
    let copied = io::copy(&mut input, &mut output).and_then(|_| {
        let meta = input.metadata()?;
        output.set_permissions(meta.permissions())?;
        output.set_modified(meta.modified()?)
    });
    if copied.is_err() {
        drop(output);
        let _ = fs::remove_file(dst); // our own partial file
    }
    copied
}

fn move_new(src: &Path, dst: &Path) -> io::Result<()> {
    if dst.symlink_metadata().is_ok() {
        return Err(exists_error(dst));
    }
    match fs::rename(src, dst) {
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            copy_new(src, dst)?;
            fs::remove_file(src)
        }
        other => other,
    }
}

/// Move files to the desktop's trash (recoverable). Returns the indices
/// that went and the first problem, if any; nothing is ever deleted outright.
pub fn trash(files: &[PathBuf]) -> Outcome {
    use gtk::gio::{self, prelude::*};
    let mut outcome = Outcome { done: Vec::new(), error: None };
    for (i, path) in files.iter().enumerate() {
        match gio::File::for_path(path).trash(gio::Cancellable::NONE) {
            Ok(()) => outcome.done.push(i),
            Err(e) => {
                outcome.error.get_or_insert(format!("{e} ({})", crate::cli::file_name(path)));
            }
        }
    }
    outcome
}

/// Rotate a JPEG losslessly by `quarters` clockwise: only its EXIF
/// orientation changes, the image data is not touched.
pub fn save_rotation(path: &Path, quarters: u8) -> Result<(), String> {
    use crate::exif::{Exif, minimal_segment, turned};
    let problem = |e: io::Error| format!("{}: {e}", path.display());
    let mut file = fs::OpenOptions::new().read(true).write(true).open(path).map_err(problem)?;
    let mut head = Vec::new();
    Read::by_ref(&mut file).take(128 * 1024).read_to_end(&mut head).map_err(problem)?;
    if !head.starts_with(&[0xff, 0xd8]) {
        return Err(format!("{}: only JPEG files can be rotated", path.display()));
    }
    let Some(exif) = Exif::find(&head) else {
        // No EXIF data at all: a minimal block, conventionally after the JFIF header.
        let jfif = head.get(2..4) == Some(&[0xff, 0xe0]) && head.len() >= 6;
        let at = if jfif { 4 + u16::from_be_bytes([head[4], head[5]]) as u64 } else { 2 };
        return splice(path, &mut file, at..at, &minimal_segment(turned(1, quarters))).map_err(problem);
    };
    if let Some((offset, little_endian, value)) = exif.orientation() {
        let new = turned(value, quarters);
        let bytes = if little_endian { new.to_le_bytes() } else { new.to_be_bytes() };
        file.seek(SeekFrom::Start(offset as u64)).map_err(problem)?;
        return file.write_all(&bytes).map_err(problem);
    }
    // EXIF data that never recorded an orientation: the block grows by a directory.
    let (old, segment) = exif
        .with_orientation(turned(1, quarters))
        .ok_or_else(|| format!("{}: no room for an orientation in its EXIF data", path.display()))?;
    splice(path, &mut file, old.start as u64..old.end as u64, &segment).map_err(problem)
}

/// Rebuild `path` with the bytes in `replace` exchanged for `insert`. The new
/// file is written next to the original and then put in its place.
fn splice(path: &Path, file: &mut fs::File, replace: std::ops::Range<u64>, insert: &[u8]) -> io::Result<()> {
    let tmp = path.with_file_name(format!(".omapic-{}.tmp", std::process::id()));
    let mut out = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let written = (|| {
        file.seek(SeekFrom::Start(0))?;
        io::copy(&mut Read::by_ref(file).take(replace.start), &mut out)?;
        out.write_all(insert)?;
        file.seek(SeekFrom::Start(replace.end))?;
        io::copy(file, &mut out)?;
        out.set_permissions(file.metadata()?.permissions())?;
        out.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Expand a leading `~` and make the path absolute.
pub fn expand(input: &str) -> PathBuf {
    let input = input.trim();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let path = match input.strip_prefix('~') {
        Some("") => home,
        Some(rest) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(input),
    };
    std::path::absolute(&path).unwrap_or(path)
}

/// Tab completion for directory prompts: longest common completion of `input`.
pub fn complete_dir(input: &str) -> Option<String> {
    let (head, partial) = input.rsplit_once('/')?;
    let dir = expand(&format!("{head}/"));
    let mut matches: Vec<String> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(partial) && (partial.starts_with('.') || !n.starts_with('.')))
        .collect();
    matches.sort();
    let first = matches.first()?.clone();
    let common = matches.iter().fold(first.len(), |len, m| {
        first.bytes().zip(m.bytes()).take(len).take_while(|(a, b)| a == b).count()
    });
    let mut end = common;
    while !first.is_char_boundary(end) {
        end -= 1;
    }
    let slash = if matches.len() == 1 { "/" } else { "" };
    Some(format!("{head}/{}{slash}", &first[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omapic-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, name).unwrap();
        p
    }

    #[test]
    fn prefixes() {
        assert_eq!(prefix_width(1), 3);
        assert_eq!(prefix_width(999), 3);
        assert_eq!(prefix_width(1000), 4);
        assert_eq!(strip_order_prefix("001_a.jpg"), "a.jpg");
        assert_eq!(strip_order_prefix("0012_a.jpg"), "a.jpg");
        assert_eq!(strip_order_prefix("01_a.jpg"), "01_a.jpg");
        assert_eq!(strip_order_prefix("2024_"), "2024_");
        assert_eq!(strip_order_prefix("holiday.jpg"), "holiday.jpg");
    }

    #[test]
    fn copy_with_order_prefix() {
        let dir = tmp("copy");
        let files = vec![touch(&dir, "b.jpg"), touch(&dir, "a.jpg")];
        let dest = dir.join("out/nested");
        let plan = plan(Op::Copy, &files, &dest, true).unwrap();
        let out = execute(&plan);
        assert!(out.error.is_none());
        assert_eq!(fs::read_to_string(dest.join("001_b.jpg")).unwrap(), "b.jpg");
        assert_eq!(fs::read_to_string(dest.join("002_a.jpg")).unwrap(), "a.jpg");
        assert!(files[0].exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn collisions_are_found_before_anything_moves() {
        let dir = tmp("collide");
        let files = vec![touch(&dir, "a.jpg"), touch(&dir, "b.jpg")];
        let dest = dir.join("out");
        fs::create_dir(&dest).unwrap();
        touch(&dest, "b.jpg");
        let err = plan(Op::Move, &files, &dest, false).unwrap_err();
        assert!(err.contains("exists"), "{err}");
        assert!(files[0].exists() && files[1].exists());

        // Same name from two directories.
        let other = touch(&dest, "a.jpg");
        let err = plan(Op::Copy, &[files[0].clone(), other], &dir.join("new"), false).unwrap_err();
        assert!(err.contains("both become"), "{err}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn links_leave_originals_alone() {
        let dir = tmp("links");
        let files = vec![touch(&dir, "a.jpg"), touch(&dir, "b.jpg")];
        let soft = dir.join("soft");
        assert!(execute(&plan(Op::Symlink, &files, &soft, true).unwrap()).error.is_none());
        assert_eq!(fs::read_link(soft.join("001_a.jpg")).unwrap(), files[0]);
        assert_eq!(fs::read_to_string(soft.join("002_b.jpg")).unwrap(), "b.jpg");

        let hard = dir.join("hard");
        assert!(execute(&plan(Op::Hardlink, &files, &hard, false).unwrap()).error.is_none());
        assert!(!hard.join("a.jpg").symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(hard.join("a.jpg")).unwrap(), "a.jpg");

        // A second run collides and is refused up front.
        assert!(plan(Op::Symlink, &files, &soft, true).is_err());
        assert!(files.iter().all(|f| f.exists()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn move_updates_files() {
        let dir = tmp("move");
        let files = vec![touch(&dir, "a.jpg")];
        let dest = dir.join("out");
        let out = execute(&plan(Op::Move, &files, &dest, false).unwrap());
        assert_eq!(out.done, vec![0]);
        assert!(!files[0].exists() && dest.join("a.jpg").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn renumbering_overlapping_names() {
        let dir = tmp("renumber");
        // Reverse an already-prefixed pair: each target is the other's source-ish name.
        let files = vec![touch(&dir, "002_x.jpg"), touch(&dir, "001_x.jpg")];
        let plan = plan(Op::Rename, &files, Path::new(""), true).unwrap();
        assert_eq!(plan.steps[0].dst, dir.join("001_x.jpg"));
        let out = execute(&plan);
        assert!(out.error.is_none(), "{:?}", out.error);
        assert_eq!(fs::read_to_string(dir.join("001_x.jpg")).unwrap(), "002_x.jpg");
        assert_eq!(fs::read_to_string(dir.join("002_x.jpg")).unwrap(), "001_x.jpg");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rename_refuses_foreign_collision() {
        let dir = tmp("foreign");
        let files = vec![touch(&dir, "a.jpg")];
        touch(&dir, "001_a.jpg");
        assert!(plan(Op::Rename, &files, Path::new(""), true).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rotation_rewrites_only_the_orientation() {
        use crate::exif::Exif;
        let dir = tmp("rotate");
        let orientation = |p: &Path| {
            let bytes = fs::read(p).unwrap();
            Exif::find(&bytes).and_then(|e| e.orientation()).map_or(0, |(_, _, value)| value)
        };

        // No EXIF yet: JFIF header, then image data.
        let plain = dir.join("plain.jpg");
        let body = [0xff, 0xd8, 0xff, 0xe0, 0, 4, b'J', b'F', 0xff, 0xda, 0, 2, 9, 9, 0xff, 0xd9];
        fs::write(&plain, body).unwrap();
        save_rotation(&plain, 1).unwrap();
        assert_eq!(orientation(&plain), 6);
        let rebuilt = fs::read(&plain).unwrap();
        assert_eq!(rebuilt.len(), body.len() + 36);
        assert_eq!(rebuilt[..8], body[..8]); // JFIF stays in front
        assert_eq!(rebuilt[8 + 36..], body[8..]); // image data untouched

        // Now there is a tag: it is patched in place.
        save_rotation(&plain, 1).unwrap();
        assert_eq!(orientation(&plain), 3);
        assert_eq!(fs::read(&plain).unwrap().len(), rebuilt.len());
        save_rotation(&plain, 2).unwrap();
        assert_eq!(orientation(&plain), 1);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1); // no temp file left

        // EXIF data without an orientation entry gets one; the rest survives.
        let bare = dir.join("bare.jpg");
        let thumb = [0xff, 0xd8, 7, 7, 0xff, 0xd9];
        fs::write(&bare, crate::exif::tests::sample_jpeg(None, &thumb, true)).unwrap();
        save_rotation(&bare, 3).unwrap();
        assert_eq!(orientation(&bare), 8);
        let bytes = fs::read(&bare).unwrap();
        assert_eq!(&bytes[Exif::find(&bytes).unwrap().thumbnail().unwrap()], thumb);
        save_rotation(&bare, 1).unwrap();
        assert_eq!(orientation(&bare), 1);
        fs::remove_file(&bare).unwrap();

        let png = touch(&dir, "x.png");
        assert!(save_rotation(&png, 1).unwrap_err().contains("only JPEG"));
        assert_eq!(fs::read_to_string(&png).unwrap(), "x.png");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn completion() {
        let dir = tmp("complete");
        fs::create_dir(dir.join("alpha")).unwrap();
        fs::create_dir(dir.join("alpine")).unwrap();
        fs::create_dir(dir.join("beta")).unwrap();
        let base = dir.to_str().unwrap();
        assert_eq!(complete_dir(&format!("{base}/a")), Some(format!("{base}/alp")));
        assert_eq!(complete_dir(&format!("{base}/b")), Some(format!("{base}/beta/")));
        assert_eq!(complete_dir(&format!("{base}/z")), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
