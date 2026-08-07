mod clipboard;
mod command;
mod preview;
mod search;
mod source;
mod view;

use std::{cell::RefCell, rc::Rc};

use gio::prelude::*;
use gtk::glib;

use source::{Activation, Event, Outcome, Source};
use view::Launcher;

const RESULT_LIMIT: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Apps,
    Clipboard,
    Dmenu,
}

pub struct Manager {
    launcher: RefCell<Option<Launcher>>,
    dmenu: RefCell<Option<DmenuSession>>,
}

impl Manager {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            launcher: RefCell::new(None),
            dmenu: RefCell::new(None),
        })
    }

    pub fn install_actions(
        self: &Rc<Self>,
        app: &gtk::Application,
        before_open: impl Fn() + 'static,
    ) {
        let before_open: Rc<dyn Fn()> = Rc::new(before_open);
        self.add_action(app, "launcher", Mode::Apps, Rc::clone(&before_open));
        self.add_action(app, "clipboard", Mode::Clipboard, before_open);
    }

    fn add_action(
        self: &Rc<Self>,
        app: &gtk::Application,
        name: &str,
        mode: Mode,
        before_open: Rc<dyn Fn()>,
    ) {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate({
            let app = app.clone();
            let manager = self.clone();
            move |_, _| {
                app.activate();
                before_open();
                manager.toggle_source(&app, mode);
            }
        });
        app.add_action(&action);
    }

    pub fn handle_command_line(
        self: &Rc<Self>,
        app: &gtk::Application,
        command_line: &gio::ApplicationCommandLine,
    ) -> glib::ExitCode {
        match command::parse(&command_line.arguments()) {
            Ok(command::Request::Activate) => {
                app.activate();
                0.into()
            }
            Ok(command::Request::Launcher) => {
                app.activate();
                self.toggle_source(app, Mode::Apps);
                0.into()
            }
            Ok(command::Request::Clipboard) => {
                app.activate();
                self.toggle_source(app, Mode::Clipboard);
                0.into()
            }
            Ok(command::Request::Dmenu { prompt }) => match command::read_lines(command_line)
                .and_then(|lines| self.run_dmenu(app, lines, &prompt))
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
            },
            Err(error) => {
                command_line.printerr_literal(&format!("varde: {error}\n{}\n", command::USAGE));
                2.into()
            }
        }
    }

    fn toggle_source(self: &Rc<Self>, app: &gtk::Application, mode: Mode) {
        if self.dmenu.borrow().is_some() {
            self.close();
            return;
        }
        if self.active_mode() == Some(mode) {
            self.close();
            return;
        }
        let (source, prompt, alphabetical) = match mode {
            Mode::Apps => (source::apps(), "Search", true),
            Mode::Clipboard => (source::clipboard(), "Clipboard", false),
            Mode::Dmenu => unreachable!(),
        };
        self.show(app, mode, source, prompt, alphabetical, Some(RESULT_LIMIT));
    }

    pub fn run_dmenu(
        self: &Rc<Self>,
        app: &gtk::Application,
        lines: Vec<String>,
        prompt: &str,
    ) -> Result<Option<String>, String> {
        if self.dmenu.borrow().is_some() {
            return Err("A selector is already active".into());
        }
        if self.is_open() {
            self.close();
        }

        let main_loop = glib::MainLoop::new(None, false);
        let result = Rc::new(RefCell::new(None));
        self.dmenu.replace(Some(DmenuSession {
            main_loop: main_loop.clone(),
            result: Rc::clone(&result),
        }));
        self.show(app, Mode::Dmenu, source::dmenu(lines), prompt, false, None);
        main_loop.run();
        self.dmenu.take();
        let selected = result.borrow().clone();
        Ok(selected)
    }

    fn show(
        self: &Rc<Self>,
        app: &gtk::Application,
        mode: Mode,
        source: Rc<dyn Source>,
        prompt: &str,
        alphabetical: bool,
        limit: Option<usize>,
    ) {
        let mut launcher = self
            .launcher
            .take()
            .unwrap_or_else(|| Launcher::new(app, self));
        launcher.configure(mode, source, prompt, alphabetical, limit);
        launcher.present();
        self.launcher.replace(Some(launcher));
        let manager = self.clone();
        glib::idle_add_local_once(move || manager.request_visible_thumbnails());
    }

    pub fn close(&self) {
        self.hide();
        if let Some(session) = self.dmenu.take() {
            session.main_loop.quit();
        }
    }

    fn hide(&self) {
        if let Some(launcher) = self.launcher.take() {
            launcher.destroy();
        }
    }

    fn is_open(&self) -> bool {
        self.launcher.borrow().is_some()
    }

    fn active_mode(&self) -> Option<Mode> {
        self.launcher.borrow().as_ref().map(Launcher::mode)
    }

    fn update(&self) {
        if let Some(launcher) = self.launcher.borrow_mut().as_mut() {
            launcher.update();
        }
        self.request_visible_thumbnails();
    }

    fn request_visible_thumbnails(&self) {
        if let Some(launcher) = self.launcher.borrow().as_ref() {
            launcher.request_visible_thumbnails();
        }
    }

    fn handle_event(&self, event: Event) {
        match event {
            Event::Items { generation, items } => {
                if let Some(launcher) = self.launcher.borrow_mut().as_mut() {
                    launcher.items_loaded(generation, items);
                }
                self.request_visible_thumbnails();
            }
            Event::Activation {
                generation,
                outcome,
            } => {
                let outcome = {
                    let launcher = self.launcher.borrow();
                    launcher
                        .as_ref()
                        .and_then(|launcher| launcher.finish_activation(generation, outcome))
                };
                if let Some(outcome) = outcome {
                    self.handle_outcome(outcome);
                }
            }
            Event::Image {
                generation,
                id,
                kind,
                pixels,
            } => {
                if let Some(launcher) = self.launcher.borrow().as_ref() {
                    launcher.image_loaded(generation, id, kind, pixels);
                }
            }
            Event::Text {
                generation,
                id,
                text,
            } => {
                if let Some(launcher) = self.launcher.borrow().as_ref() {
                    launcher.text_loaded(generation, id, text);
                }
            }
        }
    }

    fn selection_changed(&self) {
        if let Some(launcher) = self.launcher.borrow().as_ref() {
            launcher.update_preview();
        }
    }

    fn move_selection(&self, offset: i32) {
        if let Some(launcher) = self.launcher.borrow().as_ref() {
            launcher.move_selection(offset);
        }
    }

    fn activate_selected(&self) {
        let activation = self
            .launcher
            .borrow()
            .as_ref()
            .and_then(Launcher::activate_selected);
        if let Some(activation) = activation {
            self.handle_activation(activation);
        }
    }

    fn activate(&self, row_index: i32) {
        let activation = self
            .launcher
            .borrow()
            .as_ref()
            .and_then(|launcher| launcher.activate(row_index));
        if let Some(activation) = activation {
            self.handle_activation(activation);
        }
    }

    fn handle_activation(&self, activation: Activation) {
        match activation {
            Activation::Ready(outcome) => self.handle_outcome(outcome),
            Activation::Pending => {
                if let Some(launcher) = self.launcher.borrow().as_ref() {
                    launcher.show_activation_pending();
                }
            }
        }
    }

    fn handle_outcome(&self, outcome: Result<Outcome, String>) {
        match outcome {
            Ok(Outcome::Done) => self.close(),
            Ok(Outcome::Return(value)) => {
                if let Some(session) = self.dmenu.take() {
                    session.result.replace(Some(value));
                    self.hide();
                    session.main_loop.quit();
                }
            }
            Err(error) => {
                if let Some(launcher) = self.launcher.borrow().as_ref() {
                    launcher.show_message(&error, true);
                }
            }
        }
    }
}

struct DmenuSession {
    main_loop: glib::MainLoop,
    result: Rc<RefCell<Option<String>>>,
}
