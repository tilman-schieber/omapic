//! Contact sheets via ImageMagick's `magick montage`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

/// The system monospace font, so labels match the rest of the desktop.
pub fn label_font() -> Option<PathBuf> {
    let out = Command::new("fc-match").args(["-f", "%{file}", "monospace"]).output().ok()?;
    let path = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    path.is_file().then_some(path)
}

pub struct ContactSheet {
    pub output: PathBuf,
    pub columns: u32,
    pub thumb_size: u32,
    pub labels: bool,
    pub background: String,
    pub foreground: String,
    /// Font file for the labels; ImageMagick's default when `None`.
    pub font: Option<PathBuf>,
}

impl ContactSheet {
    pub fn args(&self, files: &[PathBuf]) -> Vec<OsString> {
        let size = self.thumb_size;
        let mut args: Vec<OsString> = vec!["montage".into()];
        let mut push = |s: String| args.push(s.into());
        // Let the JPEG decoder downscale while reading; full-size decodes of
        // hundreds of photos would be slow and memory hungry.
        push("-define".into());
        push(format!("jpeg:size={}x{}", size * 2, size * 2));
        push("-auto-orient".into());
        push("-background".into());
        push(self.background.clone());
        push("-fill".into());
        push(self.foreground.clone());
        if let Some(font) = &self.font {
            args.push("-font".into());
            args.push(font.clone().into_os_string());
        }
        let mut push = |s: String| args.push(s.into());
        push("-label".into());
        push(if self.labels { "%t".into() } else { String::new() });
        // Absolute paths, so no file name can be mistaken for an option.
        args.extend(files.iter().map(|f| f.clone().into_os_string()));
        let mut push = |s: String| args.push(s.into());
        push("-tile".into());
        push(format!("{}x", self.columns));
        push("-geometry".into());
        push(format!("{size}x{size}+6+6"));
        args.push(self.output.clone().into_os_string());
        args
    }

    /// Blocking; call from a worker thread.
    pub fn run(&self, files: &[PathBuf]) -> Result<(), String> {
        if files.is_empty() {
            return Err("no images to put on the sheet".into());
        }
        if self.output.symlink_metadata().is_ok() {
            return Err(format!("exists: {}", self.output.display()));
        }
        let out = Command::new("magick")
            .args(self.args(files))
            .output()
            .map_err(|e| format!("cannot run magick: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        let complaint = stderr.lines().next().unwrap_or("magick failed");
        if self.output.is_file() {
            // montage skips images it cannot read but still exits non-zero
            return Err(format!("sheet written, but {complaint}"));
        }
        Err(complaint.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv() {
        let sheet = ContactSheet {
            output: "/out/sheet.jpg".into(),
            columns: 5,
            thumb_size: 200,
            labels: true,
            background: "#000".into(),
            foreground: "#fff".into(),
            font: None,
        };
        let args = sheet.args(&["/a/1.jpg".into(), "/a/2.jpg".into()]);
        let args: Vec<_> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args[0], "montage");
        let label = args.iter().position(|a| *a == "-label").unwrap();
        let first = args.iter().position(|a| *a == "/a/1.jpg").unwrap();
        assert!(label < first, "label must precede the images");
        assert_eq!(args[label + 1], "%t");
        assert!(args.windows(2).any(|w| w == ["-tile", "5x"]));
        assert!(args.windows(2).any(|w| w == ["-geometry", "200x200+6+6"]));
        assert_eq!(*args.last().unwrap(), "/out/sheet.jpg");
    }
}
