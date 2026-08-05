use std::time::Duration;

use gtk::prelude::*;
use serde::Deserialize;

use super::command::{command, on_click, set_state, spawn_shell_then_refresh, watch};

const INTERVAL: Duration = Duration::from_secs(2);
const DOT_SIZE: i32 = 5;
const DOT_RIGHT_OFFSET: i32 = 0;
const DOT_TOP: i32 = 4;

pub fn notification() -> gtk::Button {
    let button = gtk::Button::builder().focusable(false).build();
    button.add_css_class("module");
    button.add_css_class("notification");

    let label = gtk::Label::new(None);
    let dot = gtk::Box::builder()
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .can_target(false)
        .build();
    dot.add_css_class("notification-dot");
    dot.set_visible(false);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&label));
    overlay.add_overlay(&dot);
    overlay.set_clip_overlay(&dot, false);
    overlay.connect_get_child_position(|overlay, _| {
        Some(gtk::gdk::Rectangle::new(
            overlay.width() - DOT_SIZE + DOT_RIGHT_OFFSET,
            DOT_TOP,
            DOT_SIZE,
            DOT_SIZE,
        ))
    });
    button.set_overflow(gtk::Overflow::Visible);
    button.set_child(Some(&overlay));

    let widget = button.clone();
    let refresh = watch(INTERVAL, state, move |state| {
        set_state(&widget, &state.class);
        label.set_text(state.icon());
        dot.set_visible(state.has_notifications());
        widget.set_tooltip_text(Some(&state.tooltip));
    });
    on_click(&button, move |mouse_button| {
        let command = match mouse_button {
            1 => "dot-cmd-notify toggle",
            2 => "dot-cmd-notify clear",
            3 => "dot-cmd-notify dnd",
            _ => return,
        };
        spawn_shell_then_refresh(command, refresh.clone());
    });

    button
}

struct State {
    alt: String,
    class: String,
    tooltip: String,
}

impl State {
    fn icon(&self) -> &'static str {
        match self.alt.as_str() {
            "dnd-notification" | "dnd-none" => "󰂛",
            _ => "󰂚",
        }
    }

    fn has_notifications(&self) -> bool {
        matches!(self.alt.as_str(), "notification" | "dnd-notification")
    }
}

fn state() -> State {
    command("dot-cmd-notify", &["status"])
        .and_then(|output| parse_notification(&output))
        .unwrap_or(State {
            alt: "none".into(),
            class: "disabled".into(),
            tooltip: "Notifications unavailable".into(),
        })
}

#[derive(Deserialize)]
struct NotificationJson {
    alt: String,
    class: serde_json::Value,
    tooltip: String,
}

fn parse_notification(text: &str) -> Option<State> {
    let notification: NotificationJson = serde_json::from_str(text).ok()?;
    let class = match notification.class {
        serde_json::Value::String(class) => class,
        serde_json::Value::Array(classes) => classes
            .into_iter()
            .filter_map(|class| class.as_str().map(str::to_string))
            .next()
            .unwrap_or_default(),
        _ => String::new(),
    };
    Some(State {
        alt: notification.alt,
        class,
        tooltip: notification.tooltip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notification_json() {
        let state = parse_notification(
            r#"{"text":"2","alt":"dnd-notification","class":"notification","tooltip":"2 notification(s)"}"#,
        )
        .unwrap();
        assert_eq!(state.icon(), "󰂛");
        assert!(state.has_notifications());
        assert_eq!(state.tooltip, "2 notification(s)");
    }
}
