mod app;
mod duckdb_service;

use gtk::prelude::*;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.parquetta.Parquetta")
        .build();

    app.connect_activate(app::build_ui);
    app.run()
}
