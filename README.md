# omapic

A tiny keyboard-driven image viewer and *temporary* organizer for Omarchy / Arch Linux.
Rust + GTK4, themed from the active Omarchy theme.

open images → inspect quickly → throw them into numbered bins → optionally arrange a bin → run one explicit action on it

Nothing is persisted. Tagging and sorting live in memory; files change only when you run a
command from the palette and confirm it.

## Use

    omapic               # images of the current directory
    omapic image.jpg     # images of that directory, image.jpg selected
    omapic DIR
    omapic *.jpg …       # exactly these files

| Keys | |
|---|---|
| arrows, `h j k l`, `g`/`G` | move around |
| `Enter`, `Space` | enlarge preview |
| `1`…`9` / `0` | put image into workspace / take it out |
| `Alt+1`…`Alt+9` / `Alt+0` | show one workspace / all images |
| `s` | manual sorting on/off (per workspace) |
| `Shift+←/→`, `H`/`L`, drag | reorder (turns manual sorting on) |
| `:` or `Ctrl+K` | commands: move, copy, rename by order, contact sheet, open folder |
| `?` / `q` | keys / quit |

Move, copy and rename check the whole batch for collisions first and never overwrite.
A manually sorted workspace can be exported with `001_name.jpg` prefixes.
Contact sheets need ImageMagick (`magick`).

## Build

    sudo pacman -S --needed rust gtk4 imagemagick
    cargo build --release
    install -Dm755 target/release/omapic ~/.local/bin/omapic

## Layout

    src/model.rs     session state: workspaces, filter, ordering (no GTK, unit tested)
    src/cli.rs       arguments → image list, natural sort
    src/fsops.rs     the only code that touches files: plan → check → execute
    src/montage.rs   `magick montage` invocation
    src/thumbs.rs    off-thread decoding, thumbnail worker pool
    src/theme.rs     Omarchy colors.toml → GTK CSS (style.css), reloads on theme change
    src/ui/          window, grid cells, preview, palette, commands

Debug builds accept `OMAPIC_SCRIPT` (see `src/ui/debug.rs`) to drive the UI and
render snapshots for smoke tests.
