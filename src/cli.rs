//! Turn command-line arguments into the list of images to open.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "tif", "tiff"];

pub struct Input {
    pub paths: Vec<PathBuf>,
    /// Index into `paths` to select initially.
    pub select: Option<usize>,
}

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

pub fn is_jpeg(path: &Path) -> bool {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg")
}

/// * no argument: images of the current directory
/// * one file: all images of its directory, that file selected
/// * several paths: exactly those files (directories contribute their images)
pub fn resolve(args: &[String]) -> Input {
    let absolute = |p: &str| std::path::absolute(p).unwrap_or_else(|_| PathBuf::from(p));
    match args {
        [] => Input { paths: scan_dir(&absolute(".")), select: None },
        [one] => {
            let path = absolute(one);
            if path.is_dir() {
                return Input { paths: scan_dir(&path), select: None };
            }
            let dir = path.parent().unwrap_or(Path::new("/")).to_path_buf();
            let mut paths = scan_dir(&dir);
            if !paths.contains(&path) && path.is_file() {
                paths.push(path.clone()); // unknown extension, but asked for explicitly
            }
            let select = paths.iter().position(|p| *p == path);
            Input { paths, select }
        }
        many => {
            let mut paths = Vec::new();
            for arg in many {
                let path = absolute(arg);
                if path.is_dir() {
                    paths.extend(scan_dir(&path));
                } else if path.is_file() && is_image(&path) {
                    paths.push(path);
                }
            }
            let mut seen = std::collections::HashSet::new();
            paths.retain(|p| seen.insert(p.clone()));
            Input { paths, select: None }
        }
    }
}

pub fn scan_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_image(p) && p.is_file())
        .collect();
    paths.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    paths
}

pub fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// Case-insensitive comparison where digit runs compare by value: img2 < img10.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.next_if(|c| c.is_ascii_digit()) {
                        s.push(c);
                    }
                    s.trim_start_matches('0').to_string()
                };
                let (na, nb) = (take(&mut ai), take(&mut bi));
                let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(&nb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["img10.jpg", "IMG2.jpg", "img1.jpg", "a.png", "img02.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a.png", "img1.jpg", "IMG2.jpg", "img02.jpg", "img10.jpg"]);
    }

    #[test]
    fn extensions() {
        assert!(is_image(Path::new("/x/a.JPG")));
        assert!(is_image(Path::new("b.webp")));
        assert!(!is_image(Path::new("notes.txt")));
        assert!(!is_image(Path::new("jpg")));
    }
}
