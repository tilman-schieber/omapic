mod cli;
mod fsops;
mod model;
mod montage;
mod quick;
mod theme;
mod thumbs;
mod ui;

use gtk::gio;
use gtk::prelude::*;

const USAGE: &str = "\
omapic — keyboard-driven image viewer and temporary organizer

  omapic               images of the current directory
  omapic image.jpg     images of that file's directory, image.jpg selected
  omapic DIR           images of DIR
  omapic *.jpg …       exactly the given files

  --print              on quit, print the paths of the images then shown
                       (one per line, in display order) — for pipelines:
                       omapic --print *.jpg | xargs -d '\\n' cp -t picked/
  --print0             the same, NUL-separated (for xargs -0)

Press ? inside omapic for the keys.";

fn main() -> gtk::glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return gtk::glib::ExitCode::SUCCESS;
    }

    let print = if args.iter().any(|a| a == "--print0") {
        Some('\0')
    } else {
        args.iter().any(|a| a == "--print").then_some('\n')
    };
    let args: Vec<String> = args.into_iter().filter(|a| a != "--print" && a != "--print0").collect();

    // Every invocation is its own throwaway session.
    let application = gtk::Application::builder()
        .application_id("org.omapic.Omapic")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.connect_activate(move |application| {
        ui::build(application, cli::resolve(&args), print);
    });
    application.run_with_args::<&str>(&[])
}
