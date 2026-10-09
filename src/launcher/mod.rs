mod actions;
mod clipboard;
mod preview;
mod search;
mod source;
mod view;

use std::{cell::RefCell, rc::Rc};

use gtk::glib;

use source::{Activation, Event, Outcome, Source};
use view::{Launcher, ThumbnailCache};

const RESULT_LIMIT: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Apps,
    Actions,
    Clipboard,
    Dmenu,
}

pub struct Manager {
    launcher: RefCell<Option<Launcher>>,
    dmenu: RefCell<Option<DmenuSession>>,
    thumbnails: Rc<RefCell<ThumbnailCache>>,
}

impl Manager {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            launcher: RefCell::new(None),
            dmenu: RefCell::new(None),
            thumbnails: Rc::default(),
        })
    }

    pub fn toggle_apps(self: &Rc<Self>, app: &gtk::Application) {
        self.toggle_source(app, Mode::Apps);
    }

    pub fn toggle_actions(self: &Rc<Self>, app: &gtk::Application) {
        self.toggle_source(app, Mode::Actions);
    }

    pub fn toggle_clipboard(self: &Rc<Self>, app: &gtk::Application) {
        self.toggle_source(app, Mode::Clipboard);
    }

    fn toggle_source(self: &Rc<Self>, app: &gtk::Application, mode: Mode) {
        if self.dmenu.borrow().is_some() {
            // While the selector input is still being read nothing is shown,
            // so the request cancels the selector and opens as usual.
            let showing = self.is_open();
            self.close();
            if showing {
                return;
            }
        }
        if self.active_mode() == Some(mode) {
            self.close();
            return;
        }
        let (source, prompt, alphabetical) = match mode {
            Mode::Actions => (actions::source(), "Actions", true),
            Mode::Apps => (source::apps(), "Search", true),
            Mode::Clipboard => (source::clipboard(), "Clipboard", false),
            Mode::Dmenu => unreachable!(),
        };
        self.show(app, mode, source, prompt, alphabetical, Some(RESULT_LIMIT));
    }

    pub fn run_dmenu(
        self: &Rc<Self>,
        app: &gtk::Application,
        lines: impl Future<Output = Result<Vec<String>, String>> + 'static,
        prompt: &str,
    ) -> Result<Option<String>, String> {
        if self.dmenu.borrow().is_some() {
            return Err("A selector is already active".into());
        }
        self.close();

        let main_loop = glib::MainLoop::new(None, false);
        let result = Rc::new(RefCell::new(Ok(None)));
        self.dmenu.replace(Some(DmenuSession {
            main_loop: main_loop.clone(),
            result: Rc::clone(&result),
        }));
        let reader = glib::spawn_future_local({
            let manager = self.clone();
            let app = app.clone();
            let prompt = prompt.to_owned();
            let result = Rc::clone(&result);
            async move {
                let lines = lines.await;
                // The session may have been closed earlier in the same main
                // loop iteration that completed the read.
                let current = manager
                    .dmenu
                    .borrow()
                    .as_ref()
                    .is_some_and(|session| Rc::ptr_eq(&session.result, &result));
                if !current {
                    return;
                }
                match lines {
                    Ok(lines) => manager.show(
                        &app,
                        Mode::Dmenu,
                        source::dmenu(lines),
                        &prompt,
                        false,
                        None,
                    ),
                    Err(error) => {
                        *result.borrow_mut() = Err(error);
                        manager.close();
                    }
                }
            }
        });
        main_loop.run();
        reader.abort();
        self.dmenu.take();
        result.replace(Ok(None))
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
                    *session.result.borrow_mut() = Ok(Some(value));
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
    result: Rc<RefCell<Result<Option<String>, String>>>,
}
