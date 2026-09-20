//! Shell commands over a set of images: placeholder expansion, a few
//! ready-made ImageMagick lines, and running the result through `sh`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Offered under the prompt. ImageMagick 7 syntax; `mogrify` works in place.
pub const SUGGESTIONS: &[(&str, &str)] = &[
    ("magick mogrify -resize 50% {}", "halve, in place"),
    ("magick mogrify -resize '2048x2048>' {}", "fit into 2048 px, in place (never enlarges)"),
    ("magick {} -resize '1600x1600>' -quality 85 {.}_web.jpg", "web-sized copy next to the original"),
    ("magick {} -thumbnail '400x400^' -gravity center -extent 400x400 {.}_thumb.jpg", "square 400 px thumbnail copy"),
    ("magick mogrify -quality 85 {}", "recompress JPEG, in place"),
    ("magick mogrify -strip {}", "remove all metadata (EXIF, GPS, profiles), in place"),
    ("magick mogrify -auto-orient {}", "bake the EXIF rotation into the pixels"),
    ("magick {} {.}.jpg", "convert to JPEG (copy)"),
    ("magick {} {.}.png", "convert to PNG (copy)"),
    ("magick {} {.}.webp", "convert to WebP (copy)"),
    ("magick mogrify -colorspace Gray {}", "black and white, in place"),
    ("magick mogrify -auto-level {}", "stretch contrast to the full range, in place"),
    ("magick mogrify -trim +repage {}", "cut off uniform borders, in place"),
    ("magick mogrify -gravity center -crop 1:1 +repage {}", "centre-crop to a square, in place"),
    ("magick mogrify -bordercolor white -border 5% {}", "add a white border, in place"),
    ("magick {+} +append strip.jpg", "all images side by side → strip.jpg"),
    ("magick {+} -append stack.jpg", "all images on top of each other → stack.jpg"),
    ("magick -delay 50 -loop 0 {+} animation.gif", "animated GIF in display order"),
    ("magick {+} images.pdf", "one PDF, a page per image"),
];

pub const PLACEHOLDERS: &str = "{} file · {.} no ext · {/} name · {/.} name, no ext · {//} folder · {+} all at once";

#[derive(Debug, PartialEq)]
pub struct Job {
    /// Ready for `sh -c`.
    pub line: String,
    pub cwd: PathBuf,
    /// Indices into the file list this job is about.
    pub files: Vec<usize>,
}

/// Quote for `sh`: safe for any path.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn without_extension(path: &Path) -> PathBuf {
    path.with_extension("")
}

fn fill(template: &str, file: &Path, all: &[PathBuf]) -> String {
    let text = |p: &Path| quote(&p.to_string_lossy());
    let name = |p: &Path| quote(&p.file_name().unwrap_or_default().to_string_lossy());
    let mut out = String::new();
    let mut rest = template;
    'scan: while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        let tokens: [(&str, &dyn Fn() -> String); 6] = [
            ("{/.}", &|| name(&without_extension(file))),
            ("{//}", &|| text(file.parent().unwrap_or(Path::new("/")))),
            ("{/}", &|| name(file)),
            ("{.}", &|| text(&without_extension(file))),
            ("{+}", &|| all.iter().map(|p| text(p)).collect::<Vec<_>>().join(" ")),
            ("{}", &|| text(file)),
        ];
        for (token, value) in tokens {
            if let Some(after) = rest.strip_prefix(token) {
                out.push_str(&value());
                rest = after;
                continue 'scan;
            }
        }
        out.push('{');
        rest = &rest[1..];
    }
    out + rest
}

/// One job per file, or a single job when the template uses `{+}`.
/// Placeholders are quoted for the shell; don't quote them again.
pub fn expand(template: &str, files: &[PathBuf]) -> Result<Vec<Job>, String> {
    let template = template.trim();
    let all_at_once = template.contains("{+}");
    let per_file = ["{}", "{.}", "{/}", "{/.}", "{//}"].iter().any(|t| template.contains(t));
    let dir = |p: &Path| p.parent().unwrap_or(Path::new("/")).to_path_buf();
    match (all_at_once, per_file) {
        _ if files.is_empty() => Err("no images to run this on".into()),
        (true, true) => Err("{+} (all at once) cannot be mixed with per-file placeholders".into()),
        (false, false) => Err("say where the files go: {} for each file, {+} for all at once".into()),
        (true, false) => Ok(vec![Job {
            line: fill(template, &files[0], files),
            cwd: dir(&files[0]),
            files: (0..files.len()).collect(),
        }]),
        (false, true) => Ok(files
            .iter()
            .enumerate()
            .map(|(i, file)| Job { line: fill(template, file, files), cwd: dir(file), files: vec![i] })
            .collect()),
    }
}

/// Blocking. `Err` carries the first line the command wrote to stderr.
pub fn run(job: &Job) -> Result<(), String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(&job.line)
        .current_dir(&job.cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run sh: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let complaint = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
    Err(if complaint.is_empty() { format!("exit status {}", output.status.code().unwrap_or(-1)) } else { complaint })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<PathBuf> {
        vec!["/pics/a b.jpg".into(), "/pics/it's.png".into()]
    }

    #[test]
    fn per_file_placeholders() {
        let jobs = expand("magick {} -resize 50% {.}_s.jpg # {/} {/.} {//}", &files()).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].line, "magick '/pics/a b.jpg' -resize 50% '/pics/a b'_s.jpg # 'a b.jpg' 'a b' '/pics'");
        assert_eq!(jobs[1].line.split(" -resize").next().unwrap(), r"magick '/pics/it'\''s.png'");
        assert_eq!((jobs[1].cwd.as_path(), jobs[1].files.as_slice()), (Path::new("/pics"), &[1][..]));
    }

    #[test]
    fn all_at_once() {
        let jobs = expand("magick {+} out.pdf", &files()).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].line, r"magick '/pics/a b.jpg' '/pics/it'\''s.png' out.pdf");
        assert_eq!(jobs[0].files, vec![0, 1]);
    }

    #[test]
    fn refuses_unclear_templates() {
        assert!(expand("ls", &files()).is_err());
        assert!(expand("cp {} {+}", &files()).is_err());
        assert!(expand("ls {}", &[]).is_err());
        // Braces that are no placeholder pass through.
        assert_eq!(expand("echo {a,b} ${x} {}", &files()[..1]).unwrap()[0].line, "echo {a,b} ${x} '/pics/a b.jpg'");
    }

    #[test]
    fn suggestions_expand() {
        for (template, _) in SUGGESTIONS {
            assert!(expand(template, &files()).is_ok(), "{template}");
        }
    }

    #[test]
    fn runs_and_reports() {
        let job = |line: &str| Job { line: line.into(), cwd: std::env::temp_dir(), files: vec![] };
        assert_eq!(run(&job("true")), Ok(()));
        assert_eq!(run(&job("echo oops >&2; false")), Err("oops".into()));
        assert_eq!(run(&job("exit 3")), Err("exit status 3".into()));
    }
}
