mod background;
mod bar;
mod input;
mod launcher;
mod notifications;
mod title;

use gio::prelude::*;
use gtk::glib;
use std::env;
use varde_cli::command;

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
    let title = title::Manager::new();
    let application_id = varde_cli::application_id();
    let app = gtk::Application::builder()
        .application_id(&application_id)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.connect_startup({
        let notifications = notifications.clone();
        let title = title.clone();
        move |app| {
            title::add_actions(app, &title);
            load_styles();
            if let Some(settings) = gtk::Settings::default() {
                settings.set_gtk_cursor_blink(true);
                settings.set_gtk_cursor_blink_time(1_000);
            }
            notifications.start(app);
        }
    });
    app.connect_activate({
        let notifications = notifications.clone();
        let title = title.clone();
        let started = std::cell::Cell::new(false);
        move |app| {
            if !started.replace(true) {
                bar::start(app, &notifications, &title);
            }
        }
    });
    app.connect_command_line({
        let launcher = launcher.clone();
        let notifications = notifications.clone();
        move |app, command_line| {
            handle_command_line(app, command_line, &launcher, &notifications, &title)
        }
    });
    // systemd and dev-restart stop the shell with SIGTERM; quitting through
    // the main loop lets the notifications be saved for the next start.
    for signal in [libc::SIGTERM, libc::SIGINT] {
        glib::unix_signal_add_local(signal, {
            let app = app.clone();
            move || {
                app.quit();
                glib::ControlFlow::Break
            }
        });
    }
    let status = app.run();
    notifications.save();
    status
}

fn handle_command_line(
    app: &gtk::Application,
    command_line: &gio::ApplicationCommandLine,
    launcher: &std::rc::Rc<launcher::Manager>,
    notifications: &std::rc::Rc<notifications::Manager>,
    title: &std::rc::Rc<title::Manager>,
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
        command::Request::Actions => {
            app.activate();
            notifications.close();
            launcher.toggle_actions(app);
        }
        command::Request::Clipboard => {
            app.activate();
            notifications.close();
            launcher.toggle_clipboard(app);
        }
        command::Request::Notifications => {
            app.activate();
            launcher.close();
            notifications.toggle(None);
        }
        command::Request::InvokeNotification => {
            return invoke_notification(app, command_line, launcher, notifications, false);
        }
        command::Request::PickNotificationAction => {
            return invoke_notification(app, command_line, launcher, notifications, true);
        }
        command::Request::DismissNotification => {
            if !notifications.dismiss_latest() {
                return 1.into();
            }
        }
        command::Request::ClearNotifications => notifications.clear(),
        command::Request::ToggleDnd => notifications.toggle_dnd(),
        command::Request::TitleSet {
            seq,
            json: Some(json),
        } => return set_title(command_line, title, &json, seq),
        command::Request::TitleSet { seq, json: None } => {
            let Some(input) = command_line.stdin() else {
                command_line.printerr_literal("varde: title input is unavailable\n");
                return 2.into();
            };
            // The client waits for its command line object to be released,
            // so holding it keeps the exit status open until the input is read.
            let command_line = command_line.clone();
            let title = title.clone();
            glib::spawn_future_local(async move {
                let status = match input::read_all(input).await {
                    Ok(json) => set_title(&command_line, &title, &json, seq),
                    Err(error) => {
                        command_line.printerr_literal(&format!("varde: {error}\n"));
                        2.into()
                    }
                };
                command_line.set_exit_status(i32::from(status));
            });
        }
        command::Request::TitleClear { seq, id } => title.clear(&id, seq),
        command::Request::Dmenu { prompt } => {
            notifications.close();
            let result = command_line
                .stdin()
                .ok_or_else(|| "selector input is unavailable".to_string())
                .and_then(|input| launcher.run_dmenu(app, input::read_lines(input), &prompt));
            return match result {
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

fn invoke_notification(
    app: &gtk::Application,
    command_line: &gio::ApplicationCommandLine,
    launcher: &std::rc::Rc<launcher::Manager>,
    notifications: &std::rc::Rc<notifications::Manager>,
    pick: bool,
) -> glib::ExitCode {
    let Some(target) = notifications.latest_actionable() else {
        command_line.printerr_literal("varde: no notification has actions\n");
        return 1.into();
    };
    if !pick && target.has_default() {
        notifications.invoke(&target, None);
        return 0.into();
    }
    notifications.close();
    let lines = std::future::ready(Ok(target.labels()));
    match launcher.run_dmenu(app, lines, "Action") {
        Ok(Some(label)) if notifications.invoke(&target, Some(&label)) => 0.into(),
        Ok(_) => 1.into(),
        Err(error) => {
            command_line.printerr_literal(&format!("varde: {error}\n"));
            2.into()
        }
    }
}

fn set_title(
    command_line: &gio::ApplicationCommandLine,
    title: &std::rc::Rc<title::Manager>,
    json: &str,
    seq: Option<u64>,
) -> glib::ExitCode {
    match title::parse(json) {
        Ok(overlay) => {
            title.set(overlay, seq);
            0.into()
        }
        Err(error) => {
            command_line.printerr_literal(&format!("varde: {error}\n"));
            2.into()
        }
    }
}

fn load_styles() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));

    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("a display is required"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
