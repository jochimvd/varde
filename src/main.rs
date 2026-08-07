mod background;
mod bar;
mod launcher;
mod notifications;

use gio::prelude::*;
use gtk::glib;
use std::env;

const APPLICATION_ID: &str = "be.jochim.shell";

fn main() -> glib::ExitCode {
    let launcher = launcher::Manager::new();
    let notifications = notifications::Manager::new();
    let application_id =
        env::var("SHELL_DEV_APP_ID").unwrap_or_else(|_| APPLICATION_ID.to_string());
    let app = gtk::Application::builder()
        .application_id(&application_id)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.connect_startup({
        let launcher = launcher.clone();
        let notifications = notifications.clone();
        move |app| {
            load_styles(app);
            notifications.start(app);
            launcher.install_actions(app, {
                let notifications = notifications.clone();
                move || notifications.close()
            });
            notifications.install_action(app, {
                let launcher = launcher.clone();
                move || launcher.close()
            });
        }
    });
    app.connect_activate({
        let notifications = notifications.clone();
        move |app| bar::show(app, &notifications)
    });
    app.connect_command_line({
        let launcher = launcher.clone();
        let notifications = notifications.clone();
        move |app, command_line| {
            notifications.close();
            launcher.handle_command_line(app, command_line)
        }
    });
    app.run()
}

fn load_styles(_: &gtk::Application) {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("style.css"));

    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("a display is required"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
