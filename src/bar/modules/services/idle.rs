use gtk::prelude::*;
use zbus::blocking::{Connection, Proxy};

use crate::background;

pub fn widget() -> gtk::Button {
    let button = gtk::Button::builder().focusable(false).build();
    button.set_widget_name("idle_inhibitor");
    button.add_css_class("module");
    button.add_css_class("idle-inhibitor");
    button.set_child(Some(&gtk::Label::new(Some("󰇘"))));
    update(&button, false, None);

    let (toggles, requests) = async_channel::unbounded();
    let (results, updates) = async_channel::unbounded();
    background::listen(updates, {
        let button = button.clone();
        move |(active, error)| update(&button, active, error)
    });
    // The screensaver owner may be slow to activate, so never toggle on the main thread.
    background::spawn("idle-inhibitor", move || {
        let mut state = IdleInhibitor::default();
        while requests.recv_blocking().is_ok() {
            let result = if state.cookie.is_some() {
                state.deactivate()
            } else {
                state.activate()
            };
            if results
                .send_blocking((state.cookie.is_some(), result.err()))
                .is_err()
            {
                break;
            }
        }
    });
    button.connect_clicked(move |_| {
        let _ = toggles.try_send(());
    });

    button
}

#[derive(Default)]
struct IdleInhibitor {
    connection: Option<Connection>,
    cookie: Option<u32>,
}

impl IdleInhibitor {
    fn activate(&mut self) -> Result<(), String> {
        let connection = Connection::session().map_err(|error| error.to_string())?;
        let cookie = {
            let proxy = screensaver_proxy(&connection)?;
            proxy
                .call(
                    "Inhibit",
                    &("shell", "Idle inhibition requested from the status bar"),
                )
                .map_err(|error| error.to_string())?
        };
        self.connection = Some(connection);
        self.cookie = Some(cookie);
        Ok(())
    }

    fn deactivate(&mut self) -> Result<(), String> {
        let Some(cookie) = self.cookie.take() else {
            return Ok(());
        };
        let Some(connection) = self.connection.take() else {
            return Ok(());
        };
        let proxy = screensaver_proxy(&connection)?;
        let _: () = proxy
            .call("UnInhibit", &(cookie,))
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

fn screensaver_proxy(connection: &Connection) -> Result<Proxy<'_>, String> {
    Proxy::new(
        connection,
        "org.freedesktop.ScreenSaver",
        "/org/freedesktop/ScreenSaver",
        "org.freedesktop.ScreenSaver",
    )
    .map_err(|error| error.to_string())
}

fn update(button: &gtk::Button, active: bool, error: Option<String>) {
    button.remove_css_class("activated");
    button.remove_css_class("deactivated");
    button.add_css_class(if active { "activated" } else { "deactivated" });
    let tooltip = match (active, error) {
        (true, _) => "Idle inhibition is active".to_string(),
        (false, Some(error)) => format!("Idle inhibition unavailable: {error}"),
        (false, None) => "Idle inhibition is inactive".to_string(),
    };
    button.set_tooltip_text(Some(&tooltip));
}
