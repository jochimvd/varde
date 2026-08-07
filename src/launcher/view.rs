use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::LayerShell;

use super::source::{
    Activation, Event, ImageKind, ImagePixels, Item, Items, LoadedItem, Outcome, Source, Visual,
};
use super::{Manager, Mode, search, source};

const LAUNCHER_NAME: &str = "shell-launcher";
const PANEL_WIDTH: i32 = 600;
const ROW_HEIGHT: i32 = 44;
const VISIBLE_ROWS: i32 = 10;
const PANEL_HEIGHT: i32 = ROW_HEIGHT * (VISIBLE_ROWS + 1);
const PREVIEW_AREA_HEIGHT: i32 = 300;
const PREVIEW_WIDTH: i32 = PANEL_WIDTH / 2;
const PREVIEW_HEIGHT: i32 = PREVIEW_AREA_HEIGHT;
const THUMBNAIL_WIDTH: i32 = 56;
const THUMBNAIL_HEIGHT: i32 = 36;
const POINTER_ACTIVATION_DISTANCE: f64 = 3.0;

pub(super) struct Launcher {
    window: gtk::ApplicationWindow,
    mode: Mode,
    entry: gtk::Entry,
    list: gtk::ListBox,
    stack: gtk::Stack,
    results: gtk::Overlay,
    message: gtk::Label,
    scroll: gtk::ScrolledWindow,
    preview: gtk::Box,
    preview_space: gtk::Box,
    preview_stack: gtk::Stack,
    preview_picture: gtk::Picture,
    preview_text: gtk::Label,
    preview_id: RefCell<Option<String>>,
    source: RefCell<Rc<dyn Source>>,
    items: RefCell<Vec<Item>>,
    visible: RefCell<Vec<usize>>,
    alphabetical: bool,
    limit: Option<usize>,
    generation: u64,
    loading: bool,
    activation_pending: Cell<bool>,
    events: async_channel::Sender<Event>,
    thumbnail_cache: RefCell<HashMap<String, Option<gdk::MemoryTexture>>>,
    thumbnail_pending: RefCell<HashSet<String>>,
    thumbnail_targets: RefCell<HashMap<String, glib::WeakRef<gtk::Picture>>>,
    preview_cache: RefCell<HashMap<String, Option<gdk::MemoryTexture>>>,
    preview_pending: RefCell<HashSet<String>>,
    text_cache: RefCell<HashMap<String, Option<String>>>,
    text_pending: RefCell<HashSet<String>>,
    hover_selection: Rc<Cell<bool>>,
    pointer_position: Rc<Cell<Option<(f64, f64)>>>,
}

impl Launcher {
    pub(super) fn new(app: &gtk::Application, manager: &Rc<Manager>) -> Self {
        let (events, event_receiver) = async_channel::unbounded();
        crate::background::listen(event_receiver, {
            let manager = Rc::downgrade(manager);
            move |event| {
                if let Some(manager) = manager.upgrade() {
                    manager.handle_event(event);
                }
            }
        });

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
        window.connect_map({
            let entry = entry.clone();
            move |_| {
                let entry = entry.clone();
                glib::idle_add_local_once(move || {
                    entry.grab_focus();
                });
            }
        });

        let list = gtk::ListBox::builder()
            .activate_on_single_click(true)
            .selection_mode(gtk::SelectionMode::Single)
            .build();
        list.add_css_class("launcher-results");
        let hover_selection = Rc::new(Cell::new(false));
        let pointer_position = Rc::new(Cell::new(None));

        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(ROW_HEIGHT * VISIBLE_ROWS)
            .propagate_natural_height(true)
            .build();
        scroll.vadjustment().connect_value_changed({
            let manager = Rc::downgrade(manager);
            move |_| {
                let manager = manager.clone();
                glib::idle_add_local_once(move || {
                    if let Some(manager) = manager.upgrade() {
                        manager.request_visible_thumbnails();
                    }
                });
            }
        });

        let message = gtk::Label::builder()
            .height_request(ROW_HEIGHT)
            .label("No results")
            .xalign(0.0)
            .build();
        message.add_css_class("launcher-message");

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.add_named(&scroll, Some("results"));
        stack.add_named(&message, Some("message"));

        let preview_picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Contain)
            .hexpand(true)
            .vexpand(true)
            .build();
        let preview_text = gtk::Label::builder()
            .hexpand(true)
            .vexpand(true)
            .xalign(0.0)
            .yalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build();
        let preview_text_scroll = gtk::ScrolledWindow::builder()
            .child(&preview_text)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .build();
        let preview_message = gtk::Label::builder()
            .label("Could not load image preview")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        preview_message.add_css_class("launcher-preview-message");
        let preview_stack = gtk::Stack::new();
        preview_stack.set_hexpand(true);
        preview_stack.set_vexpand(true);
        preview_stack.add_named(&preview_picture, Some("image"));
        preview_stack.add_named(&preview_text_scroll, Some("text"));
        preview_stack.add_named(&preview_message, Some("message"));
        let preview_panel = gtk::Box::builder()
            .vexpand(true)
            .valign(gtk::Align::Fill)
            .build();
        preview_panel.add_css_class("launcher-preview");
        preview_panel.append(&preview_stack);

        let preview_offset = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        preview_offset.set_can_target(false);
        let preview = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        preview.set_homogeneous(true);
        preview.set_can_target(false);
        preview.set_hexpand(true);
        preview.set_vexpand(true);
        preview.append(&preview_offset);
        preview.append(&preview_panel);
        preview.set_visible(false);

        let preview_space = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        preview_space.set_visible(false);
        let columns = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        columns.set_homogeneous(true);
        columns.append(&stack);
        columns.append(&preview_space);

        let results = gtk::Overlay::new();
        results.set_child(Some(&columns));
        results.add_overlay(&preview);
        results.set_measure_overlay(&preview, false);
        results.set_clip_overlay(&preview, true);

        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(PANEL_WIDTH)
            .valign(gtk::Align::Start)
            .build();
        panel.add_css_class("launcher-panel");
        panel.set_overflow(gtk::Overflow::Hidden);
        panel.append(&entry);
        panel.append(&results);

        let panel_viewport = gtk::ScrolledWindow::builder()
            .child(&panel)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(PANEL_WIDTH)
            .max_content_width(PANEL_WIDTH)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .build();

        let position = gtk::Box::builder()
            .width_request(PANEL_WIDTH)
            .height_request(PANEL_HEIGHT)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        position.append(&panel_viewport);

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
        list.connect_row_selected({
            let manager = Rc::downgrade(manager);
            move |_, _| {
                let manager = manager.clone();
                glib::idle_add_local_once(move || {
                    if let Some(manager) = manager.upgrade() {
                        manager.selection_changed();
                    }
                });
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
            let hover_selection = Rc::clone(&hover_selection);
            let list = list.clone();
            move |_, key, _, _| {
                let Some(manager) = manager.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                hover_selection.set(false);
                list.remove_css_class("pointer-selection");
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
            mode: Mode::Apps,
            entry,
            list,
            stack,
            results,
            message,
            scroll,
            preview,
            preview_space,
            preview_stack,
            preview_picture,
            preview_text,
            preview_id: RefCell::new(None),
            source: RefCell::new(source::apps()),
            items: RefCell::new(Vec::new()),
            visible: RefCell::new(Vec::new()),
            alphabetical: true,
            limit: None,
            generation: 0,
            loading: false,
            activation_pending: Cell::new(false),
            events,
            thumbnail_cache: RefCell::new(HashMap::new()),
            thumbnail_pending: RefCell::new(HashSet::new()),
            thumbnail_targets: RefCell::new(HashMap::new()),
            preview_cache: RefCell::new(HashMap::new()),
            preview_pending: RefCell::new(HashSet::new()),
            text_cache: RefCell::new(HashMap::new()),
            text_pending: RefCell::new(HashSet::new()),
            hover_selection,
            pointer_position,
        }
    }

    pub(super) fn present(&self) {
        self.window.present();
    }

    pub(super) fn destroy(self) {
        self.window.destroy();
    }

    pub(super) fn mode(&self) -> Mode {
        self.mode
    }

    pub(super) fn finish_activation(
        &self,
        generation: u64,
        outcome: Result<Outcome, String>,
    ) -> Option<Result<Outcome, String>> {
        if generation != self.generation {
            return None;
        }
        self.activation_pending.set(false);
        Some(outcome)
    }

    pub(super) fn move_selection(&self, offset: i32) {
        let count = self.visible.borrow().len() as i32;
        if count == 0 {
            return;
        }
        let current = self
            .list
            .selected_row()
            .map_or(if offset > 0 { -1 } else { 0 }, |row| row.index());
        self.select((current + offset).rem_euclid(count));
    }

    pub(super) fn activate_selected(&self) -> Option<Activation> {
        self.list
            .selected_row()
            .and_then(|row| self.activate(row.index()))
    }

    pub(super) fn activate(&self, row_index: i32) -> Option<Activation> {
        if self.activation_pending.get() {
            return None;
        }
        let visible = self.visible.borrow();
        let item_index = visible.get(row_index as usize)?;
        let items = self.items.borrow();
        let item = &items[*item_index];
        Some(
            self.source
                .borrow()
                .activate(&item.id, self.generation, self.events.clone()),
        )
    }

    pub(super) fn show_activation_pending(&self) {
        self.activation_pending.set(true);
        self.show_message("Restoring clipboard…", false);
    }

    pub(super) fn configure(
        &mut self,
        mode: Mode,
        source: Rc<dyn Source>,
        prompt: &str,
        alphabetical: bool,
        limit: Option<usize>,
    ) {
        let generation = self.generation.wrapping_add(1);
        self.source.borrow().set_generation(generation);
        self.mode = mode;
        self.source.replace(source);
        self.generation = generation;
        self.source.borrow().set_generation(generation);
        self.loading = false;
        self.activation_pending.set(false);
        self.hover_selection.set(false);
        self.pointer_position.set(None);
        self.list.remove_css_class("pointer-selection");
        self.thumbnail_cache.borrow_mut().clear();
        self.thumbnail_pending.borrow_mut().clear();
        self.thumbnail_targets.borrow_mut().clear();
        self.preview_cache.borrow_mut().clear();
        self.preview_pending.borrow_mut().clear();
        self.text_cache.borrow_mut().clear();
        self.text_pending.borrow_mut().clear();
        self.hide_preview();
        self.alphabetical = alphabetical;
        self.limit = limit;
        self.entry.set_placeholder_text(Some(prompt));
        self.entry.set_text("");
        self.scroll.vadjustment().set_value(0.0);
        let loaded = self.source.borrow().items(generation, self.events.clone());
        match loaded {
            Items::Ready(items) => self.set_items(items),
            Items::Pending => {
                self.loading = true;
                self.items.borrow_mut().clear();
                self.visible.borrow_mut().clear();
                self.show_message("Loading…", false);
            }
        }
    }

    pub(super) fn items_loaded(&mut self, generation: u64, items: Result<Vec<LoadedItem>, String>) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        self.set_items(items.map(|items| items.into_iter().map(Item::from).collect()));
    }

    fn set_items(&mut self, items: Result<Vec<Item>, String>) {
        match items {
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

    pub(super) fn update(&mut self) {
        if self.loading {
            return;
        }
        let mut visible = search::rank(
            &self.items.borrow(),
            self.entry.text().as_str(),
            self.alphabetical,
        );
        if let Some(limit) = self.limit {
            visible.truncate(limit);
        }
        self.visible.replace(visible);
        self.rebuild_rows();
    }

    fn rebuild_rows(&self) {
        self.thumbnail_targets.borrow_mut().clear();
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
            match &item.visual {
                Visual::None => {}
                Visual::Icon(icon) => {
                    let image = gtk::Image::from_gicon(icon);
                    image.set_pixel_size(24);
                    content.append(&image);
                }
                Visual::Image => {
                    let image = gtk::Picture::builder()
                        .can_shrink(true)
                        .content_fit(gtk::ContentFit::Cover)
                        .hexpand(true)
                        .vexpand(true)
                        .build();
                    let placeholder = gtk::Box::builder()
                        .width_request(THUMBNAIL_WIDTH)
                        .height_request(THUMBNAIL_HEIGHT)
                        .build();
                    let thumbnail = gtk::Overlay::builder()
                        .halign(gtk::Align::Start)
                        .valign(gtk::Align::Center)
                        .child(&placeholder)
                        .build();
                    thumbnail.set_overflow(gtk::Overflow::Hidden);
                    thumbnail.add_css_class("launcher-thumbnail");
                    thumbnail.add_overlay(&image);
                    if let Some(Some(texture)) = self.thumbnail_cache.borrow().get(&item.id) {
                        image.set_paintable(Some(texture));
                    } else if !self.thumbnail_cache.borrow().contains_key(&item.id) {
                        self.thumbnail_targets
                            .borrow_mut()
                            .insert(item.id.clone(), image.downgrade());
                    }
                    content.append(&thumbnail);
                }
                Visual::Text => {}
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
                let hover_selection = Rc::clone(&self.hover_selection);
                let pointer_position = Rc::clone(&self.pointer_position);
                move |controller, _, _| {
                    select_from_pointer(
                        controller,
                        &list,
                        &row,
                        &hover_selection,
                        &pointer_position,
                    );
                }
            });
            hover.connect_motion({
                let list = self.list.clone();
                let row = row.clone();
                let hover_selection = Rc::clone(&self.hover_selection);
                let pointer_position = Rc::clone(&self.pointer_position);
                move |controller, _, _| {
                    select_from_pointer(
                        controller,
                        &list,
                        &row,
                        &hover_selection,
                        &pointer_position,
                    );
                }
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
        self.update_preview();
    }

    pub(super) fn request_visible_thumbnails(&self) {
        let adjustment = self.scroll.vadjustment();
        let visible = self.visible.borrow();
        let range = visible_row_range(adjustment.value(), adjustment.page_size(), visible.len());
        let items = self.items.borrow();
        let generation = self.generation;
        for item_index in visible[range].iter().copied() {
            let item = &items[item_index];
            if !matches!(item.visual, Visual::Image)
                || self.thumbnail_cache.borrow().contains_key(&item.id)
                || !self.thumbnail_pending.borrow_mut().insert(item.id.clone())
            {
                continue;
            }
            if !self.source.borrow().request_image(
                &item.id,
                ImageKind::Thumbnail,
                THUMBNAIL_WIDTH,
                THUMBNAIL_HEIGHT,
                generation,
                self.events.clone(),
            ) {
                self.thumbnail_pending.borrow_mut().remove(&item.id);
                self.thumbnail_cache
                    .borrow_mut()
                    .insert(item.id.clone(), None);
            }
        }
    }

    pub(super) fn update_preview(&self) {
        let item = self
            .list
            .selected_row()
            .and_then(|row| self.visible.borrow().get(row.index() as usize).copied())
            .and_then(|index| self.items.borrow().get(index).cloned());
        let Some(item) = item else {
            self.hide_preview();
            return;
        };
        match item.visual {
            Visual::Image => self.update_image_preview(&item.id),
            Visual::Text => self.update_text_preview(&item.id),
            Visual::None | Visual::Icon(_) => self.hide_preview(),
        }
    }

    fn update_image_preview(&self, id: &str) {
        self.show_preview(id, "image");
        match self.preview_cache.borrow().get(id).cloned() {
            Some(Some(texture)) => {
                self.preview_picture.set_paintable(Some(&texture));
                return;
            }
            Some(None) => {
                self.show_preview_error(id);
                return;
            }
            None => {}
        }
        self.preview_picture.set_paintable(gdk::Paintable::NONE);
        if !self.preview_pending.borrow_mut().insert(id.to_string()) {
            return;
        }
        if !self.source.borrow().request_image(
            id,
            ImageKind::Preview,
            PREVIEW_WIDTH,
            PREVIEW_HEIGHT,
            self.generation,
            self.events.clone(),
        ) {
            self.preview_pending.borrow_mut().remove(id);
            self.preview_cache.borrow_mut().insert(id.to_string(), None);
            self.show_preview_error(id);
        }
    }

    fn update_text_preview(&self, id: &str) {
        if let Some(text) = self.text_cache.borrow().get(id) {
            if let Some(text) = text.as_deref().filter(|text| is_multiline(text)) {
                self.preview_text.set_text(text);
                self.show_preview(id, "text");
            } else {
                self.hide_preview();
            }
            return;
        }
        self.hide_preview();
        self.preview_id.replace(Some(id.to_string()));
        if self.text_pending.borrow_mut().insert(id.to_string()) {
            let requested =
                self.source
                    .borrow()
                    .request_text(id, self.generation, self.events.clone());
            if !requested {
                self.text_pending.borrow_mut().remove(id);
                self.text_cache.borrow_mut().insert(id.to_string(), None);
                self.hide_preview();
            }
        }
    }

    fn show_preview(&self, id: &str, child: &str) {
        self.preview_id.replace(Some(id.to_string()));
        self.preview_stack.set_visible_child_name(child);
        self.preview_space.set_visible(true);
        self.preview.set_visible(true);
        self.results.set_height_request(PREVIEW_AREA_HEIGHT);
    }

    fn show_preview_error(&self, id: &str) {
        self.show_preview(id, "message");
    }

    fn hide_preview(&self) {
        self.preview_id.take();
        self.preview.set_visible(false);
        self.preview_space.set_visible(false);
        self.results.set_height_request(-1);
    }

    pub(super) fn image_loaded(
        &self,
        generation: u64,
        id: String,
        kind: ImageKind,
        pixels: Option<ImagePixels>,
    ) {
        if generation != self.generation {
            return;
        }
        let texture = pixels.map(image_texture);
        match kind {
            ImageKind::Thumbnail => {
                self.thumbnail_pending.borrow_mut().remove(&id);
                self.thumbnail_cache
                    .borrow_mut()
                    .insert(id.clone(), texture.clone());
                let target = self
                    .thumbnail_targets
                    .borrow()
                    .get(&id)
                    .and_then(glib::WeakRef::upgrade);
                if let (Some(target), Some(texture)) = (target, texture) {
                    target.set_paintable(Some(&texture));
                }
            }
            ImageKind::Preview => {
                self.preview_pending.borrow_mut().remove(&id);
                self.preview_cache
                    .borrow_mut()
                    .insert(id.clone(), texture.clone());
                if self.preview_id.borrow().as_deref() == Some(id.as_str()) {
                    if let Some(texture) = texture {
                        self.preview_picture.set_paintable(Some(&texture));
                    } else {
                        self.show_preview_error(&id);
                    }
                }
            }
        }
    }

    pub(super) fn text_loaded(&self, generation: u64, id: String, text: Option<String>) {
        if generation != self.generation {
            return;
        }
        self.text_pending.borrow_mut().remove(&id);
        self.text_cache.borrow_mut().insert(id.clone(), text);
        if self.preview_id.borrow().as_deref() == Some(id.as_str()) {
            self.update_text_preview(&id);
        }
    }

    pub(super) fn show_message(&self, text: &str, error: bool) {
        self.hide_preview();
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

fn image_texture(pixels: ImagePixels) -> gdk::MemoryTexture {
    let bytes = glib::Bytes::from_owned(pixels.rgba);
    gdk::MemoryTexture::new(
        pixels.width,
        pixels.height,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        pixels.stride,
    )
}

fn select_from_pointer(
    controller: &gtk::EventControllerMotion,
    list: &gtk::ListBox,
    row: &gtk::ListBoxRow,
    hover_selection: &Cell<bool>,
    pointer_position: &Cell<Option<(f64, f64)>>,
) {
    let Some(position) = controller
        .current_event()
        .and_then(|event| event.position())
    else {
        return;
    };
    let Some(previous) = pointer_position.get() else {
        pointer_position.set(Some(position));
        return;
    };
    if !hover_selection.get() && !pointer_moved_enough(previous, position) {
        return;
    }
    pointer_position.set(Some(position));
    if !hover_selection.get() {
        hover_selection.set(true);
        list.add_css_class("pointer-selection");
    }
    list.select_row(Some(row));
}

fn pointer_moved_enough(previous: (f64, f64), current: (f64, f64)) -> bool {
    let distance_squared = (current.0 - previous.0).powi(2) + (current.1 - previous.1).powi(2);
    distance_squared >= POINTER_ACTIVATION_DISTANCE * POINTER_ACTIVATION_DISTANCE
}

fn is_multiline(text: &str) -> bool {
    text.contains('\n') || text.contains('\r')
}

fn visible_row_range(value: f64, page_size: f64, count: usize) -> std::ops::Range<usize> {
    let start = ((value / f64::from(ROW_HEIGHT)).floor() as usize).min(count);
    let rows = if page_size > 0.0 {
        (page_size / f64::from(ROW_HEIGHT)).ceil() as usize + 1
    } else {
        VISIBLE_ROWS as usize
    };
    start..(start + rows).min(count)
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

    #[test]
    fn thumbnail_range_follows_the_scroll_viewport() {
        assert_eq!(visible_row_range(0.0, 0.0, 200), 0..10);
        assert_eq!(visible_row_range(88.0, 440.0, 200), 2..13);
        assert_eq!(visible_row_range(8_800.0, 440.0, 200), 200..200);
    }

    #[test]
    fn previews_only_multiline_text() {
        assert!(!is_multiline("one line"));
        assert!(is_multiline("first\nsecond"));
        assert!(is_multiline("first\r\nsecond"));
    }

    #[test]
    fn pointer_selection_requires_real_movement() {
        assert!(!pointer_moved_enough((100.0, 100.0), (100.0, 100.0)));
        assert!(!pointer_moved_enough((100.0, 100.0), (102.0, 100.0)));
        assert!(pointer_moved_enough((100.0, 100.0), (103.0, 100.0)));
    }
}
