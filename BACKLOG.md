# Backlog

Ideas that fit omapic's scope but aren't built yet. The filter for anything
new: it either speeds up a step of *open → inspect → bin → arrange → act*,
or it is one more explicit action on a bin.

Everything that was here is built. New ideas go below.

## Ideas

- Keep the clipboard alive after quitting (`y` currently needs omapic open
  or a clipboard manager).
- Pick up files a shell command created (e.g. `{.}_web.jpg`) without restarting.
- Publish `pkg/PKGBUILD` to the AUR.

## Deliberately out

- Persistence of anything about images, including optional session files
  (the shell command history is the one stored thing). If bins must
  survive, yank / `--print` hands them to the shell.
- Editing, EXIF writing beyond the orientation value, ratings, recursive library scanning,
  config files, plugins, multiple windows or tabs.
- RAW decoding (heavy dependencies). Embedded previews may cover RAW+JPEG workflows.
