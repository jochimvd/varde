mod background;
mod bar;
mod command;
mod launcher;
mod notifications;

use gio::prelude::*;
use gtk::glib;
use std::env;

const APPLICATION_ID: &str = "org.varde.desktop";

fn main() -> glib::ExitCode {
    let arguments = env::args_os().collect::<Vec<_>>();
    match command::parse(&arguments) {
        Ok(command::Request::Help(help)) => {
            println!("{help}");
            return glib::ExitCode::SUCCESS;
        }
        Ok(command::Request::Version) => {
            println!("varde {}", env!("CARGO_PKG_VERSION"));
            return glib::ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("varde: {error}\n\nFor more information, try '--help'.");
            return 2.into();
        }
        _ => {}
    }

    let launcher = launcher::Manager::new();
    let notifications = notifications::Manager::new();
    let application_id =
        env::var("VARDE_DEV_APP_ID").unwrap_or_else(|_| APPLICATION_ID.to_string());
    let app = gtk::Application::builder()
        .application_id(&application_id)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.connect_startup({
        let notifications = notifications.clone();
        move |app| {
            load_styles(app);
            notifications.start(app);
        }
    });
    app.connect_activate({
        let notifications = notifications.clone();
        move |app| bar::show(app, &notifications)
    });
    app.connect_command_line({
        let launcher = launcher.clone();
        let notifications = notifications.clone();
        move |app, command_line| handle_command_line(app, command_line, &launcher, &notifications)
    });
    app.run()
}

fn handle_command_line(
    app: &gtk::Application,
    command_line: &gio::ApplicationCommandLine,
    launcher: &std::rc::Rc<launcher::Manager>,
    notifications: &std::rc::Rc<notifications::Manager>,
) -> glib::ExitCode {
    let request = match command::parse(&command_line.arguments()) {
        Ok(request) => request,
        Err(error) => {
            command_line.printerr_literal(&format!(
                "varde: {error}\n\nFor more information, try '--help'.\n"
            ));
            return 2.into();
        }
    };

    match request {
        command::Request::Help(help) => command_line.print_literal(&format!("{help}\n")),
        command::Request::Version => {
            command_line.print_literal(&format!("varde {}\n", env!("CARGO_PKG_VERSION")))
        }
        command::Request::Start => app.activate(),
        command::Request::Launcher => {
            app.activate();
            notifications.close();
            launcher.toggle_apps(app);
        }
        command::Request::Clipboard => {
            app.activate();
            notifications.close();
            launcher.toggle_clipboard(app);
        }
        command::Request::Notifications => {
            app.activate();
            launcher.close();
            notifications.toggle();
        }
        command::Request::Dmenu { prompt } => {
            notifications.close();
            return match command::read_lines(command_line)
                .and_then(|lines| launcher.run_dmenu(app, lines, &prompt))
            {
                Ok(Some(selected)) => {
                    command_line.print_literal(&format!("{selected}\n"));
                    0.into()
                }
                Ok(None) => 1.into(),
                Err(error) => {
                    command_line.printerr_literal(&format!("varde: {error}\n"));
                    2.into()
                }
            };
        }
    }
    0.into()
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
