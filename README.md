# omapic

A tiny, fast, keyboard-driven image viewer and *temporary* organizer for
[Omarchy](https://omarchy.org) / Arch Linux. Native Rust + GTK4, themed from
the active Omarchy theme.

<img src="docs/screenshot.png" alt="omapic: thumbnail grid with workspace badges, preview pane and status line">

The whole workflow:

    open images → inspect quickly → throw the interesting ones into numbered bins
                → optionally arrange a bin → run one explicit action on it

omapic is **not** a photo library, DAM, editor, EXIF tool or tagging system.
Nothing is persisted: bins and ordering live in memory and vanish when you
quit. Files are touched only when you run a command from the palette and
confirm it.

## Install

    sudo pacman -S --needed rust gtk4 libjpeg-turbo imagemagick wl-clipboard
    git clone https://github.com/tilman-schieber/omapic
    cd omapic
    cargo install --path .        # → ~/.cargo/bin/omapic

Or as a package, which also installs the desktop entry ("Open with omapic"
in file managers) and icon:

    cd pkg && makepkg -si      # add -d if your cargo comes from rustup/mise rather than pacman

Without the package, the desktop entry can be installed by hand:

    install -Dm644 data/org.omapic.Omapic.desktop -t ~/.local/share/applications/
    install -Dm644 data/org.omapic.Omapic.svg -t ~/.local/share/icons/hicolor/scalable/apps/

ImageMagick (`magick`) is only needed for contact sheets. Animated GIFs play in
the preview. JPEG, PNG, WebP,
GIF and TIFF are loaded through gdk-pixbuf/glycin, which GTK4 already pulls in.

## Usage

    omapic               # images of the current directory
    omapic image.jpg     # all images of that directory, image.jpg selected
    omapic DIR           # images of DIR
    omapic *.jpg …       # exactly the given files, in the given order

With `--print` (or `--print0`), omapic writes the paths of the images shown
at the moment you quit to stdout, in display order — so a bin can be handed
to the shell without touching anything:

    omapic --print *.jpg | xargs -d '\n' cp -t picked/    # quit while showing bin 1

Hover or move the selection to preview. The right end of the status line
always hints at the keys that matter in the current context; `?` shows all.

### Keys

| Key | Action |
|---|---|
| arrows, `h` `j` `k` `l` | move around the grid |
| `Home` `End`, `g` `G` | first / last image |
| `n` / `N` | next / previous binned image |
| `Enter`, `Space` | enlarge preview (`Esc` to go back) |
| `r` / `R` | rotate right / left — in the session only, until saved from the palette |
| `z` | actual pixels at the pointer; drag or `Shift`+arrows / `H J K L` to pan; the position is kept from image to image |
| `1` … `9`, `0` | put the selected image into that workspace; the same key again takes it out |
| `m` | mark / unmark — a flag independent of the workspaces |
| `v`, shift-click | select a range; a workspace key or `m` then applies to all of it (one undo step) |
| `a` | auto-advance on/off: binning moves on to the next image |
| `Alt+1` … `Alt+9`, `Alt+0` | show only that workspace |
| `Alt+M` | show only the marked images |
| `Alt+A`, `Esc` | show all images |
| `Alt+U`, ``Alt+` `` | show only what isn't binned yet — a worklist that empties as you go |
| `o` | order unsorted views by: natural → date taken → date modified → size → name |
| `s` | manual sorting on / off for the current view |
| `Shift+←` `Shift+→`, `H` `L` | move the image backward / forward |
| drag a thumbnail | reorder |
| `u` / `U`, `Ctrl+R` | undo / redo binning, ordering, rotating and sort toggles (never file operations) |
| `y` / `Y` | copy the path of the selected image(s) / of everything shown to the clipboard (kept after quitting via `wl-copy`, if installed) |
| `+` / `-` | larger / smaller thumbnails |
| `f` | file names under thumbnails |
| `i` | file and camera info (size, dates, camera, exposure) under the preview |
| `:` or `Ctrl+K` | command palette |
| `?` | key sheet |
| `q` | quit |

### Workspaces

Ten numbered bins, `1`–`9` and `0`. An image is in at most one; pressing
its bin's key again takes it out, and assigning it elsewhere moves
it, and it leaves the current view immediately if that view no longer
matches. Tagged thumbnails carry a numbered badge, and the status line lists
the non-empty bins with their counts.

On top of the bins there is one non-exclusive set: **marks** (`m`, shown as
a dot on the thumbnail). A mark doesn't care which bin an image is in, so it
serves for "the best across all bins" and as a hand-picked selection: show
the marked images with `Alt+M` and every command acts on exactly those, in
an order of their own if you arrange them. There is deliberately only one
such flag — no ratings, colours or tags.

Every view (each bin, the marked images, and "all") is either in natural order — the order the
images were given or found, natural-sorted by name — or sorted by hand.
Reordering turns manual sorting on; `s` switches back to natural order and
remembers your arrangement in case you return. Reordering never renames
anything.

### Commands

All commands act on the images **currently shown**, in the order shown.

| Command | |
|---|---|
| Move workspace to folder… | asks for a folder (Tab completes, `~` works, created if missing) |
| Copy workspace to folder… | same, leaving the originals |
| Symlink / Hard-link workspace into folder… | a selection folder without duplicating the data |
| Rename files in workspace according to current order… | in place: `001_name.jpg`, `002_…` |
| Create contact sheet… | output file, columns, thumbnail size, labels on/off → `magick montage` |
| Run shell command on workspace… (`!`) | anything, per file or over all files — see below |
| Move workspace to trash… | the desktop trash, so it is recoverable — e.g. bin 9 for rejects |
| Save rotations to files… | writes pending `r`/`R` rotations of the shown JPEGs |
| Open containing folder | of the selected image |
| Remove selected image from workspace | same as pressing its workspace key again |

When a manually sorted view is moved or copied, omapic offers to persist the
order as numeric prefixes. The width fits the image count (at least three
digits), and an existing `NNN_` prefix is replaced rather than stacked.

### Shell commands

`!` (or the palette) runs a shell command over the images shown, in display
order. Placeholders, already quoted for the shell:

| | |
|---|---|
| `{}` | the file — the command runs once per file, in the file's folder |
| `{.}` `{/}` `{/.}` `{//}` | path without extension · file name · name without extension · folder |
| `{+}` | all files at once — the command runs a single time, in the first file's folder |

    magick {} -resize '1600x1600>' {.}_web.jpg
    magick {+} images.pdf

Under the prompt omapic offers your earlier commands and a set of common
ImageMagick lines; type to narrow them down, pick one with the arrows and
`Enter` to get it into the prompt for editing, `Enter` again to go on. The
expanded commands are shown before anything runs. Afterwards thumbnails are
refreshed, images whose files are gone leave the session, and image files
the command created in those folders (`{.}_web.jpg`, `strip.jpg`, …) join it,
unbinned.

The history (`~/.local/state/omapic/shell-history`) is the only thing omapic
ever stores, and it holds commands, not anything about your images.

### Rotation

`r` and `R` turn the selected image (or marked range) like everything else in
omapic: on screen, in memory, undoable. The status line counts unsaved
rotations. *Save rotations to files…* makes them permanent, losslessly: only
the JPEG's EXIF orientation value is changed (two bytes, in place); a JPEG
without EXIF data gets a minimal EXIF block, and EXIF data that never
recorded an orientation gets the entry added without moving any of the
existing data. Image data is never re-encoded. JPEG only. Contact sheets
show pending rotations; move, copy and shell commands work on the files as
they are on disk.

### Safety

- Browsing, tagging and sorting never write anything, anywhere.
- Each file operation is planned in full and checked first: existing targets,
  two files mapping to one name, or missing sources abort it before the first
  file is touched.
- Nothing is ever overwritten; copies are created exclusively.
- In-place renames whose targets overlap their sources go through temporary
  names, and are rolled back if parking fails.
- Every operation needs an explicit `y`.
- omapic itself changes file contents only in *Save rotations* (the
  orientation value, nothing else). What a shell command does is up to you;
  you see exactly what will run before it does.
- There is no delete; the closest thing is the (recoverable) trash.

## Theming

Colours come from `~/.local/state/omarchy/current/theme/colors.toml` (or
`~/.config/omarchy/current/…` on older installs) and follow theme switches
while running. Without Omarchy a dark fallback palette is used. The font is
the system `monospace`. Layout and styling live in `src/style.css`.

The window is undecorated, as suits a tiling compositor. For Hyprland rules,
the window class is `org.omapic.Omapic`.

## Development

    cargo test         # model, CLI, file operations, montage argv, theme parsing
    cargo run -- ~/Pictures

    src/model.rs     session state: workspaces, filter, ordering (no GTK)
    src/cli.rs       arguments → image list, natural sort
    src/fsops.rs     the only code that modifies files: plan → check → execute; EXIF rotation
    src/montage.rs   `magick montage` invocation
    src/shell.rs     shell command templates: placeholders, suggestions, running
    src/history.rs   shell command history file
    src/thumbs.rs    off-thread decoding, thumbnail worker pool
    src/quick.rs     ready-made thumbnails: XDG cache lookup, EXIF previews
    src/exif.rs      minimal EXIF reader; orientation entry writer
    src/theme.rs     Omarchy colors.toml → GTK CSS, live reload
    src/ui/          window, grid cells, preview, palette, commands

Thumbnails come from worker threads, nearest-to-viewport first. Each image
is first looked up in the freedesktop thumbnail cache (`~/.cache/thumbnails`,
read but never written) and in its own EXIF data; an embedded preview is
shown at once and replaced by a proper decode afterwards. JPEGs are decoded
by libjpeg-turbo directly at reduced size (about 4× faster than decoding in
full); everything else goes through gdk-pixbuf. Thumbnails live in
a bounded in-memory cache; full-size previews are decoded only on demand.

Debug builds can be driven by a script for smoke tests, including rendering
the window to a PNG without a visible screen:

    OMAPIC_SCRIPT="l 1 l 2 alt+1 snap:/tmp/shot.png" cargo run -- DIR

See `src/ui/debug.rs` for the step syntax.

## License

[MIT](LICENSE)
