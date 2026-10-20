use gtk::prelude::*;
use zbus::blocking::{Connection, Proxy};

use crate::{
    background,
    bar::{Msg, Updates},
};

/// Whether idle is inhibited, and why it could not be.
#[derive(Clone, Default)]
pub struct Inhibition {
    active: bool,
    error: Option<String>,
}

pub struct Idle {
    toggles: async_channel::Sender<()>,
}

impl Idle {
    pub fn new(updates: &Updates) -> Self {
        let (toggles, requests) = async_channel::unbounded();
        let (results, inhibitions) = async_channel::unbounded();
        updates.forward(inhibitions, Msg::Idle);
        // The screensaver owner may be slow to activate, so never toggle on the main thread.
        background::spawn("idle-inhibitor", move || {
            let mut state = IdleInhibitor::default();
            let result = state.activate();
            if results
                .send_blocking(Inhibition {
                    active: state.cookie.is_some(),
                    error: result.err(),
                })
                .is_err()
            {
                return;
            }

            while requests.recv_blocking().is_ok() {
                let result = if state.cookie.is_some() {
                    state.deactivate()
                } else {
                    state.activate()
                };
                if results
                    .send_blocking(Inhibition {
                        active: state.cookie.is_some(),
                        error: result.err(),
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self { toggles }
    }

    pub fn widget(&self) -> gtk::Button {
        let button = gtk::Button::builder()
            .focusable(false)
            .valign(gtk::Align::Center)
            .build();
        button.set_cursor_from_name(Some("pointer"));
        button.set_widget_name("idle_inhibitor");
        button.add_css_class("module");
        button.add_css_class("idle-inhibitor");
        button.set_child(Some(&gtk::Label::new(Some("󰇘"))));
        let toggles = self.toggles.clone();
        button.connect_clicked(move |_| {
            let _ = toggles.try_send(());
        });

        button
    }
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
                    &("varde", "Idle inhibition requested from the status bar"),
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

pub fn show(button: &gtk::Button, inhibition: &Inhibition) {
    button.remove_css_class("activated");
    button.remove_css_class("deactivated");
    button.add_css_class(if inhibition.active {
        "activated"
    } else {
        "deactivated"
    });
    let tooltip = match (inhibition.active, &inhibition.error) {
        (true, _) => "Idle inhibition is active".to_string(),
        (false, Some(error)) => format!("Idle inhibition unavailable: {error}"),
        (false, None) => "Idle inhibition is inactive".to_string(),
    };
    button.set_tooltip_text(Some(&tooltip));
}
