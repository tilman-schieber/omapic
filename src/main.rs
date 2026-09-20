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

Press ? inside omapic for the keys.";

fn main() -> gtk::glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return gtk::glib::ExitCode::SUCCESS;
    }

    // Every invocation is its own throwaway session.
    let application = gtk::Application::builder()
        .application_id("org.omapic.Omapic")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.connect_activate(move |application| {
        ui::build(application, cli::resolve(&args));
    });
    application.run_with_args::<&str>(&[])
}
