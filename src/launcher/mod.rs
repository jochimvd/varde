mod command;
mod search;
mod source;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gio::prelude::*;
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::LayerShell;

use source::{Item, Outcome, Source};

const LAUNCHER_NAME: &str = "shell-launcher";
const PANEL_WIDTH: i32 = 600;
const ROW_HEIGHT: i32 = 44;
const VISIBLE_ROWS: i32 = 10;
const PANEL_HEIGHT: i32 = ROW_HEIGHT * (VISIBLE_ROWS + 1);
const APP_RESULT_LIMIT: usize = 200;

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

    pub fn install_action(
        self: &Rc<Self>,
        app: &gtk::Application,
        before_open: impl Fn() + 'static,
    ) {
        let action = gio::SimpleAction::new("launcher", None);
        action.connect_activate({
            let app = app.clone();
            let manager = self.clone();
            move |_, _| {
                app.activate();
                before_open();
                manager.toggle_apps(&app);
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
                self.toggle_apps(app);
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
                    command_line.printerr_literal(&format!("shell: {error}\n"));
                    2.into()
                }
            },
            Err(error) => {
                command_line.printerr_literal(&format!("shell: {error}\n{}\n", command::USAGE));
                2.into()
            }
        }
    }

    pub fn toggle_apps(self: &Rc<Self>, app: &gtk::Application) {
        if self.is_visible() {
            self.close();
        } else {
            self.show(app, source::apps(), "Search", true, Some(APP_RESULT_LIMIT));
        }
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
        if self.is_visible() {
            self.close();
        }

        let main_loop = glib::MainLoop::new(None, false);
        let result = Rc::new(RefCell::new(None));
        self.dmenu.replace(Some(DmenuSession {
            main_loop: main_loop.clone(),
            result: Rc::clone(&result),
        }));
        self.show(app, source::dmenu(lines), prompt, false, None);
        main_loop.run();
        self.dmenu.take();
        let selected = result.borrow().clone();
        Ok(selected)
    }

    fn show(
        self: &Rc<Self>,
        app: &gtk::Application,
        source: Rc<dyn Source>,
        prompt: &str,
        alphabetical: bool,
        limit: Option<usize>,
    ) {
        if self.launcher.borrow().is_none() {
            self.launcher.replace(Some(Launcher::new(app, self)));
        }
        let mut launcher = self.launcher.borrow_mut();
        let launcher = launcher.as_mut().expect("launcher was just constructed");
        launcher.configure(source, prompt, alphabetical, limit);
        launcher.window.present();
        launcher.entry.grab_focus();
    }

    pub fn close(&self) {
        if let Some(launcher) = self.launcher.borrow().as_ref() {
            launcher.window.set_visible(false);
        }
        if let Some(session) = self.dmenu.take() {
            session.main_loop.quit();
        }
    }

    fn is_visible(&self) -> bool {
        self.launcher
            .borrow()
            .as_ref()
            .is_some_and(|launcher| launcher.window.is_visible())
    }

    fn update(&self) {
        if let Some(launcher) = self.launcher.borrow_mut().as_mut() {
            launcher.update();
        }
    }

    fn move_selection(&self, offset: i32) {
        let launcher = self.launcher.borrow();
        let Some(launcher) = launcher.as_ref() else {
            return;
        };
        let count = launcher.visible.borrow().len() as i32;
        if count == 0 {
            return;
        }
        let current = launcher
            .list
            .selected_row()
            .map_or(if offset > 0 { -1 } else { 0 }, |row| row.index());
        let next = (current + offset).rem_euclid(count);
        launcher.select(next);
    }

    fn activate_selected(&self) {
        let index = self
            .launcher
            .borrow()
            .as_ref()
            .and_then(|launcher| launcher.list.selected_row())
            .map(|row| row.index());
        if let Some(index) = index {
            self.activate(index);
        }
    }

    fn activate(&self, row_index: i32) {
        let activation = {
            let launcher = self.launcher.borrow();
            let Some(launcher) = launcher.as_ref() else {
                return;
            };
            let visible = launcher.visible.borrow();
            let Some(item_index) = visible.get(row_index as usize) else {
                return;
            };
            let items = launcher.items.borrow();
            let item = &items[*item_index];
            launcher.source.borrow().activate(&item.id)
        };

        match activation {
            Ok(Outcome::Done) => self.close(),
            Ok(Outcome::Return(value)) => {
                if let Some(session) = self.dmenu.take() {
                    session.result.replace(Some(value));
                    if let Some(launcher) = self.launcher.borrow().as_ref() {
                        launcher.window.set_visible(false);
                    }
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

struct Launcher {
    window: gtk::ApplicationWindow,
    entry: gtk::Entry,
    list: gtk::ListBox,
    stack: gtk::Stack,
    message: gtk::Label,
    scroll: gtk::ScrolledWindow,
    source: RefCell<Rc<dyn Source>>,
    items: RefCell<Vec<Item>>,
    visible: RefCell<Vec<usize>>,
    alphabetical: Cell<bool>,
    limit: Cell<Option<usize>>,
}

impl Launcher {
    fn new(app: &gtk::Application, manager: &Rc<Manager>) -> Self {
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_cursor_blink(true);
            settings.set_gtk_cursor_blink_time(1_000);
        }

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .name(LAUNCHER_NAME)
            .build();
        window.add_css_class("launcher");
        window.init_layer_shell();
        window.set_namespace(Some(LAUNCHER_NAME));
        window.set_layer(gtk4_layer_shell::Layer::Overlay);
        for edge in [
            gtk4_layer_shell::Edge::Top,
            gtk4_layer_shell::Edge::Bottom,
            gtk4_layer_shell::Edge::Left,
            gtk4_layer_shell::Edge::Right,
        ] {
            window.set_anchor(edge, true);
        }
        window.set_exclusive_zone(-1);
        window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::Exclusive);

        let backdrop = gtk::Box::builder().hexpand(true).vexpand(true).build();
        backdrop.add_css_class("launcher-backdrop");
        let backdrop_click = gtk::GestureClick::new();
        backdrop_click.connect_released({
            let manager = Rc::downgrade(manager);
            move |_, _, _, _| {
                if let Some(manager) = manager.upgrade() {
                    manager.close();
                }
            }
        });
        backdrop.add_controller(backdrop_click);

        let entry = gtk::Entry::builder()
            .has_frame(false)
            .height_request(ROW_HEIGHT)
            .placeholder_text("Search")
            .build();
        entry.add_css_class("launcher-input");

        let list = gtk::ListBox::builder()
            .activate_on_single_click(true)
            .selection_mode(gtk::SelectionMode::Single)
            .build();
        list.add_css_class("launcher-results");

        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(ROW_HEIGHT * VISIBLE_ROWS)
            .propagate_natural_height(true)
            .build();

        let message = gtk::Label::builder()
            .height_request(ROW_HEIGHT)
            .label("No results")
            .xalign(0.0)
            .build();
        message.add_css_class("launcher-message");

        let stack = gtk::Stack::new();
        stack.add_named(&scroll, Some("results"));
        stack.add_named(&message, Some("message"));

        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(PANEL_WIDTH)
            .valign(gtk::Align::Start)
            .build();
        panel.add_css_class("launcher-panel");
        panel.append(&entry);
        panel.append(&stack);

        let position = gtk::Box::builder()
            .width_request(PANEL_WIDTH)
            .height_request(PANEL_HEIGHT)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        position.append(&panel);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&backdrop));
        overlay.add_overlay(&position);
        window.set_child(Some(&overlay));

        entry.connect_changed({
            let manager = Rc::downgrade(manager);
            move |_| {
                if let Some(manager) = manager.upgrade() {
                    manager.update();
                }
            }
        });
        list.connect_row_activated({
            let manager = Rc::downgrade(manager);
            move |_, row| {
                if let Some(manager) = manager.upgrade() {
                    manager.activate(row.index());
                }
            }
        });
        window.connect_close_request({
            let manager = Rc::downgrade(manager);
            move |_| {
                if let Some(manager) = manager.upgrade() {
                    manager.close();
                }
                glib::Propagation::Stop
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let manager = Rc::downgrade(manager);
            move |_, key, _, _| {
                let Some(manager) = manager.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                match key {
                    gdk::Key::Escape => manager.close(),
                    gdk::Key::Down => manager.move_selection(1),
                    gdk::Key::Up => manager.move_selection(-1),
                    gdk::Key::Return | gdk::Key::KP_Enter => manager.activate_selected(),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        window.add_controller(keys);

        Self {
            window,
            entry,
            list,
            stack,
            message,
            scroll,
            source: RefCell::new(source::apps()),
            items: RefCell::new(Vec::new()),
            visible: RefCell::new(Vec::new()),
            alphabetical: Cell::new(true),
            limit: Cell::new(Some(APP_RESULT_LIMIT)),
        }
    }

    fn configure(
        &mut self,
        source: Rc<dyn Source>,
        prompt: &str,
        alphabetical: bool,
        limit: Option<usize>,
    ) {
        self.source.replace(source);
        self.alphabetical.set(alphabetical);
        self.limit.set(limit);
        self.entry.set_placeholder_text(Some(prompt));
        self.entry.set_text("");
        self.scroll.vadjustment().set_value(0.0);
        let loaded = self.source.borrow().items();
        match loaded {
            Ok(items) => {
                self.items.replace(items);
                self.update();
            }
            Err(error) => {
                self.items.borrow_mut().clear();
                self.visible.borrow_mut().clear();
                self.show_message(&error, true);
            }
        }
    }

    fn update(&mut self) {
        let mut visible = search::rank(
            &self.items.borrow(),
            self.entry.text().as_str(),
            self.alphabetical.get(),
        );
        if let Some(limit) = self.limit.get() {
            visible.truncate(limit);
        }
        self.visible.replace(visible);
        self.rebuild_rows();
    }

    fn rebuild_rows(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let items = self.items.borrow();
        for item_index in self.visible.borrow().iter().copied() {
            let item = &items[item_index];
            let content = gtk::Box::builder()
                .spacing(10)
                .height_request(ROW_HEIGHT)
                .build();
            content.add_css_class("launcher-row");
            if let Some(icon) = &item.icon {
                let image = gtk::Image::from_gicon(icon);
                image.set_pixel_size(24);
                content.append(&image);
            }
            let label = gtk::Label::builder()
                .label(&item.title)
                .hexpand(true)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            content.append(&label);

            let row = gtk::ListBoxRow::builder().child(&content).build();
            let hover = gtk::EventControllerMotion::new();
            hover.connect_enter({
                let list = self.list.clone();
                let row = row.clone();
                move |_, _, _| list.select_row(Some(&row))
            });
            row.add_controller(hover);
            self.list.append(&row);
        }

        if self.visible.borrow().is_empty() {
            self.show_message("No results", false);
        } else {
            self.message.remove_css_class("error");
            self.stack.set_visible_child_name("results");
            self.select(0);
        }
    }

    fn show_message(&self, text: &str, error: bool) {
        self.message.set_text(text);
        if error {
            self.message.add_css_class("error");
        } else {
            self.message.remove_css_class("error");
        }
        self.stack.set_visible_child_name("message");
    }

    fn select(&self, index: i32) {
        let Some(row) = self.list.row_at_index(index) else {
            return;
        };
        self.list.select_row(Some(&row));
        let adjustment = self.scroll.vadjustment();
        let top = f64::from(index * ROW_HEIGHT);
        let bottom = top + f64::from(ROW_HEIGHT);
        if top < adjustment.value() {
            adjustment.set_value(top);
        } else if bottom > adjustment.value() + adjustment.page_size() {
            adjustment.set_value(bottom - adjustment.page_size());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_height_is_one_to_ten_rows() {
        let height = |count: usize| ROW_HEIGHT * count.clamp(1, VISIBLE_ROWS as usize) as i32;
        assert_eq!(height(0), ROW_HEIGHT);
        assert_eq!(height(4), ROW_HEIGHT * 4);
        assert_eq!(height(20), ROW_HEIGHT * VISIBLE_ROWS);
    }
}
