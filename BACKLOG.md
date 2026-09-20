# Backlog

Ideas that fit omapic's scope but aren't built yet. The filter for anything
new: it either speeds up a step of *open → inspect → bin → arrange → act*,
or it is one more explicit action on a bin.

## Actions

- **Yank paths** — `y` copies the selected/visible paths to the clipboard;
  `omapic --print` writes the shown bin's paths to stdout on quit, so omapic
  composes with the shell (`omapic *.jpg --print | xargs …`).
- **Run a shell command on the workspace** — `:!` with `{}` substitution
  (`mogrify -resize 50% {}`), confirmed like every other file operation.
- **Move workspace to trash** — via `gio trash`; recoverable, so it fits the
  safety rules. Enables "bin 9 = rejects → trash".

## Polish

- **Rotation for EXIF blocks without an orientation entry** (needs IFD rewriting)
  and honouring pending rotations in contact sheets.
- **Pan with the keyboard** while at actual pixels (e.g. `Shift`+arrows).
- **Rotate EXIF-oriented dimensions** in the caption (shows sensor orientation today).
- **Sort keys for the natural order** — `o` cycles name / mtime / size;
  session-only. EXIF date since an EXIF reader exists now.
- **Image info** — `i` toggles file size, date and a few EXIF fields in the caption.
- **Grid size** — `+` / `-`.
- **Packaging** — `.desktop` file with image MIME types, AUR `PKGBUILD`.
- **Animated GIF playback** in the preview, only if trivial with `gtk::MediaFile`.
- **Faster JPEG thumbnails** for files without embedded previews, e.g. DCT-scaled
  decoding through libjpeg-turbo (adds a dependency; measure first).

## Deliberately out

- Persistence of any kind, including optional session files. If bins must
  survive, yank / `--print` hands them to the shell.
- Editing, EXIF writing beyond the orientation value, ratings, recursive library scanning,
  config files, plugins, multiple windows or tabs.
- RAW decoding (heavy dependencies). Embedded previews may cover RAW+JPEG workflows.
