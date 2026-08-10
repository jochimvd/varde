use std::{
    cell::Cell,
    process::Command,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gtk::glib;
use gtk::prelude::*;

use crate::background;

const REFRESH_TIMEOUT: Duration = Duration::from_secs(5);

thread_local! {
    /// Every refresh runs on a thread of its own, so its deadline can live there
    /// and bound the whole refresh rather than each command it happens to run.
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

#[derive(Clone)]
pub(super) struct Refresh(async_channel::Sender<()>);

impl Refresh {
    pub(super) fn request(&self) {
        let _ = self.0.try_send(());
    }
}

pub(super) fn module(name: &str) -> (gtk::Button, gtk::Label) {
    let button = gtk::Button::builder()
        .focusable(false)
        .valign(gtk::Align::Center)
        .build();
    button.set_cursor_from_name(Some("pointer"));
    button.add_css_class("module");
    button.add_css_class(name);

    let label = gtk::Label::new(None);
    button.set_child(Some(&label));
    (button, label)
}

/// Applies a module's current state as a CSS class, removing the previous one.
pub(super) struct StateClass {
    button: gtk::Button,
    current: String,
}

impl StateClass {
    pub(super) fn new(button: &gtk::Button) -> Self {
        Self {
            button: button.clone(),
            current: String::new(),
        }
    }

    pub(super) fn set(&mut self, state: &str) {
        if state == self.current {
            return;
        }
        if !self.current.is_empty() {
            self.button.remove_css_class(&self.current);
        }
        if !state.is_empty() {
            self.button.add_css_class(state);
        }
        self.current = state.into();
    }
}

pub(super) fn on_click(button: &gtk::Button, action: impl Fn(u32) + 'static) {
    let action: Rc<dyn Fn(u32)> = Rc::new(action);
    button.connect_clicked({
        let action = Rc::clone(&action);
        move |_| action(1)
    });
    for mouse_button in [2, 3] {
        let gesture = gtk::GestureClick::new();
        gesture.set_button(mouse_button);
        gesture.connect_released({
            let action = Rc::clone(&action);
            move |_, _, _, _| action(mouse_button)
        });
        button.add_controller(gesture);
    }
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
            let fetch = Arc::clone(&fetch);
            let result_sender = result_sender.clone();
            // A failed spawn never reports back, so only mark the module busy once it started.
            let started = background::spawn("module-refresh", move || {
                DEADLINE.set(Some(Instant::now() + REFRESH_TIMEOUT));
                let _ = result_sender.send_blocking(fetch());
            });
            running.set(started);
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
    let output = background::command_output(program, args, remaining_time()?)?;
    Some(strip_ansi(&String::from_utf8_lossy(&output)))
}

/// What is left of the current refresh's budget, or `None` once it is spent.
fn remaining_time() -> Option<Duration> {
    let Some(deadline) = DEADLINE.get() else {
        return Some(REFRESH_TIMEOUT);
    };
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
}

/// Reads `name`'s value out of the line-per-property output these tools print.
pub(super) fn property(text: &str, name: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(name).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
