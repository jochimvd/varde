use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use gtk::glib;
use gtk::prelude::*;

use crate::background;

const REFRESH_TIMEOUT: Duration = Duration::from_secs(5);

thread_local! {
    /// Every module refreshes on a thread of its own, so the current refresh's
    /// deadline can live there and bound the whole refresh rather than each
    /// command it happens to run.
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

#[derive(Clone)]
pub(super) struct Refresh(async_channel::Sender<()>);

impl Refresh {
    pub(super) fn request(&self) {
        let _ = self.0.try_send(());
    }
}

/// What a module's button shows, the same on every bar.
#[derive(Clone)]
pub struct Shown {
    pub(super) class: &'static str,
    pub(super) text: &'static str,
    pub(super) tooltip: String,
    pub(super) visible: bool,
}

/// One bar's button for a module.
pub struct Button {
    pub button: gtk::Button,
    label: gtk::Label,
    class: &'static str,
}

impl Button {
    pub(super) fn new(name: &str) -> Self {
        let button = gtk::Button::builder()
            .focusable(false)
            .valign(gtk::Align::Center)
            .build();
        button.set_cursor_from_name(Some("pointer"));
        button.add_css_class("module");
        button.add_css_class(name);
        let label = gtk::Label::new(None);
        button.set_child(Some(&label));
        Self {
            button,
            label,
            class: "",
        }
    }

    pub fn show(&mut self, shown: &Shown) {
        if shown.class != self.class {
            if !self.class.is_empty() {
                self.button.remove_css_class(self.class);
            }
            if !shown.class.is_empty() {
                self.button.add_css_class(shown.class);
            }
            self.class = shown.class;
        }
        self.label.set_text(shown.text);
        self.button.set_tooltip_text(Some(&shown.tooltip));
        self.button.set_visible(shown.visible);
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

pub(super) fn watch<T, Fetch, Update>(interval: Duration, fetch: Fetch, update: Update) -> Refresh
where
    T: Send + 'static,
    Fetch: Fn() -> T + Send + 'static,
    Update: FnMut(T) + 'static,
{
    let (result_sender, result_receiver) = async_channel::unbounded();
    let (refresh_sender, refresh_receiver) = async_channel::unbounded();
    background::spawn("module-refresh", move || {
        refresh_worker(refresh_receiver, result_sender, fetch)
    });
    background::listen(result_receiver, update);

    let refresh = Refresh(refresh_sender);
    refresh.request();
    let periodic_refresh = refresh.clone();
    glib::timeout_add_local(interval, move || {
        periodic_refresh.request();
        glib::ControlFlow::Continue
    });

    refresh
}

fn refresh_worker<T>(
    requests: async_channel::Receiver<()>,
    results: async_channel::Sender<T>,
    fetch: impl Fn() -> T,
) {
    while requests.recv_blocking().is_ok() {
        // Requests made while the previous refresh ran are all served by this one.
        while requests.try_recv().is_ok() {}
        DEADLINE.set(Some(Instant::now() + REFRESH_TIMEOUT));
        if results.send_blocking(fetch()).is_err() {
            break;
        }
    }
}

pub(super) fn spawn_shell_then_refresh(command: &str, refresh: Refresh) {
    if let Err(error) = background::run(&["sh", "-c", command], move |_| refresh.request()) {
        eprintln!("varde: could not run {command:?}: {error}");
    }
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
    let mut chars = text.chars().peekable();

    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            result.push(character);
        } else if chars.next_if_eq(&'[').is_some() {
            for character in chars.by_ref() {
                if ('@'..='~').contains(&character) {
                    break;
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_sequences_and_keeps_the_text_around_them() {
        assert_eq!(
            strip_ansi("\u{1b}[0;94m[bluetooth]\u{1b}[0m# ok"),
            "[bluetooth]# ok"
        );
        assert_eq!(strip_ansi("a\u{1b}b"), "ab");
        assert_eq!(strip_ansi("end\u{1b}"), "end");
    }

    #[test]
    fn coalesces_queued_refreshes_and_sets_their_deadline() {
        let (requests, request_receiver) = async_channel::unbounded();
        let (results, result_receiver) = async_channel::unbounded();
        for _ in 0..3 {
            requests.try_send(()).unwrap();
        }
        drop(requests);
        let fetches = Cell::new(0);

        refresh_worker(request_receiver, results, || {
            fetches.set(fetches.get() + 1);
            DEADLINE.get().is_some()
        });

        assert_eq!(fetches.get(), 1);
        assert_eq!(result_receiver.try_recv(), Ok(true));
        assert!(result_receiver.try_recv().is_err());
    }
}
