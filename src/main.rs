mod app;

use adw::prelude::*;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id("dev.parquetta.Parquetta")
        .build();

    app.connect_activate(app::build_ui);
    app.run()
}
