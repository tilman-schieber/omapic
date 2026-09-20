# Backlog

Ideas that fit omapic's scope but aren't built yet. The filter for anything
new: it either speeds up a step of *open → inspect → bin → arrange → act*,
or it is one more explicit action on a bin.

## Actions


## Polish

- **Pan with the keyboard** while at actual pixels (e.g. `Shift`+arrows).
- **Grid size** — `+` / `-`.
- **Packaging** — `.desktop` file with image MIME types, AUR `PKGBUILD`.
- **Animated GIF playback** in the preview, only if trivial with `gtk::MediaFile`.
- **Faster JPEG thumbnails** for files without embedded previews, e.g. DCT-scaled
  decoding through libjpeg-turbo (adds a dependency; measure first).

## Deliberately out

- Persistence of anything about images, including optional session files
  (the shell command history is the one stored thing). If bins must
  survive, yank / `--print` hands them to the shell.
- Editing, EXIF writing beyond the orientation value, ratings, recursive library scanning,
  config files, plugins, multiple windows or tabs.
- RAW decoding (heavy dependencies). Embedded previews may cover RAW+JPEG workflows.
