mod app;

use adw::prelude::*;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id("dev.parquetta.Parquetta")
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_activate(|app| app::build_ui(app, None));
    app.connect_open(|app, files, _| {
        app::build_ui(app, files.first().and_then(|file| file.path()));
    });
    app.run()
}
