use std::{cell::Cell, process::Command, rc::Rc, sync::Arc, time::Duration};

use gtk::glib;
use gtk::prelude::*;

use crate::background;

#[derive(Clone)]
pub(super) struct Refresh(async_channel::Sender<()>);

impl Refresh {
    pub(super) fn request(&self) {
        let _ = self.0.try_send(());
    }
}

pub(super) fn module(name: &str) -> (gtk::Button, gtk::Label) {
    let button = gtk::Button::builder().focusable(false).build();
    button.add_css_class("module");
    button.add_css_class(name);

    let label = gtk::Label::new(None);
    button.set_child(Some(&label));
    (button, label)
}

pub(super) fn set_state(button: &gtk::Button, state: &str) {
    for class in ["disabled", "disconnected", "muted", "critical"] {
        button.remove_css_class(class);
    }
    if !state.is_empty() {
        button.add_css_class(state);
    }
}

pub(super) fn on_click(button: &gtk::Button, action: impl Fn(u32) + 'static) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(0);
    gesture.connect_released(move |gesture, _, _, _| action(gesture.current_button()));
    button.add_controller(gesture);
}

pub(super) fn watch<T, Fetch, Update>(
    interval: Duration,
    fetch: Fetch,
    mut update: Update,
) -> Refresh
where
    T: Send + 'static,
    Fetch: Fn() -> T + Send + Sync + 'static,
    Update: FnMut(T) + 'static,
{
    let (result_sender, result_receiver) = async_channel::unbounded();
    let (refresh_sender, refresh_receiver) = async_channel::unbounded();
    let fetch = Arc::new(fetch);
    let running = Rc::new(Cell::new(false));
    let pending = Rc::new(Cell::new(false));
    let launch: Rc<dyn Fn()> = Rc::new({
        let fetch = Arc::clone(&fetch);
        let result_sender = result_sender.clone();
        let running = Rc::clone(&running);
        move || {
            running.set(true);
            let fetch = Arc::clone(&fetch);
            let result_sender = result_sender.clone();
            background::spawn("module-refresh", move || {
                let _ = result_sender.send_blocking(fetch());
            });
        }
    });

    background::listen(refresh_receiver, {
        let launch = Rc::clone(&launch);
        let running = Rc::clone(&running);
        let pending = Rc::clone(&pending);
        move |_| {
            if running.get() {
                pending.set(true);
            } else {
                launch();
            }
        }
    });

    background::listen(result_receiver, {
        let launch = Rc::clone(&launch);
        move |value| {
            update(value);
            running.set(false);
            if pending.replace(false) {
                launch();
            }
        }
    });

    let refresh = Refresh(refresh_sender);
    refresh.request();
    let periodic_refresh = refresh.clone();
    glib::timeout_add_local(interval, move || {
        periodic_refresh.request();
        glib::ControlFlow::Continue
    });

    refresh
}

pub(super) fn spawn_shell(command: &str) {
    let command = command.to_string();
    background::spawn("shell-command", move || {
        let _ = Command::new("sh").args(["-c", &command]).status();
    });
}

pub(super) fn spawn_shell_then_refresh(command: &str, refresh: Refresh) {
    let command = command.to_string();
    background::spawn("shell-command", move || {
        let _ = Command::new("sh").args(["-c", &command]).status();
        refresh.request();
    });
}

pub(super) fn command(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| strip_ansi(&String::from_utf8_lossy(&output.stdout)))
}

pub(super) fn strip_ansi(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars();

    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.next() == Some('[') {
            for character in chars.by_ref() {
                if ('@'..='~').contains(&character) {
                    break;
                }
            }
        } else {
            result.push(character);
        }
    }

    result
}
