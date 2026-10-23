use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use gtk::prelude::*;

use super::{
    model::{ICON_SIZE, Item, ItemId, MenuItem, Pixmap, ToggleKind, scale_pixmap, select_pixmap},
    watcher,
};
use crate::{
    background,
    bar::{Msg, Updates},
};

const MENU_OFFSET: i32 = 12;

/// The tray items' connection, which every bar's tray acts through.
pub struct Tray {
    shared: watcher::SharedConnection,
}

impl Tray {
    pub fn new(updates: &Updates) -> Self {
        let (sender, receiver) = async_channel::unbounded();
        let shared = watcher::SharedConnection::default();
        let watcher_connection = shared.clone();
        background::spawn("tray-watcher", move || {
            watcher::run(sender, watcher_connection)
        });
        updates.forward(receiver, Msg::Tray);
        Self { shared }
    }

    pub fn widget(&self, menu_visibility: impl Fn(bool) + 'static) -> Widget {
        let root = gtk::Box::builder().valign(gtk::Align::Center).build();
        root.set_widget_name("tray");
        root.add_css_class("tray");
        Widget {
            root,
            entries: Vec::new(),
            shared: self.shared.clone(),
            open_menus: Rc::default(),
            menu_visibility: Rc::new(menu_visibility),
        }
    }
}

/// One bar's tray icons.
pub struct Widget {
    pub root: gtk::Box,
    entries: Vec<Entry>,
    shared: watcher::SharedConnection,
    open_menus: Rc<Cell<u32>>,
    menu_visibility: Rc<dyn Fn(bool)>,
}

impl Widget {
    /// Shows `items`, refreshing the entry of the `changed` one; the other
    /// entries are kept as they are so their open menus survive.
    pub fn show(&mut self, items: &[Item], changed: Option<&ItemId>) {
        let (kept, gone) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition::<Vec<_>, _>(|entry| items.iter().any(|item| item.id == entry.id));
        for entry in gone {
            entry.destroy(&self.root);
        }
        self.entries = kept;
        for item in items {
            match self.entries.iter().find(|entry| entry.id == item.id) {
                Some(entry) if changed == Some(&item.id) => entry.update(item),
                Some(_) => {}
                None => {
                    let entry =
                        Entry::new(item, &self.shared, &self.open_menus, &self.menu_visibility);
                    self.root.append(&entry.target);
                    self.entries.push(entry);
                }
            }
        }
    }
}

/// The widgets for one tray item, kept across updates so an open menu
/// survives changes to the item or its neighbours.
struct Entry {
    id: ItemId,
    target: gtk::Box,
    image: gtk::Image,
    menu: gtk::Popover,
    item_is_menu: Rc<Cell<bool>>,
    menu_path: Rc<RefCell<Option<String>>>,
    pixmaps: Rc<RefCell<Arc<[Pixmap]>>>,
}

impl Entry {
    fn new(
        item: &Item,
        shared: &watcher::SharedConnection,
        open_menus: &Rc<Cell<u32>>,
        menu_visibility: &Rc<dyn Fn(bool)>,
    ) -> Self {
        let target = gtk::Box::builder()
            .focusable(false)
            .valign(gtk::Align::Center)
            .build();
        target.set_cursor_from_name(Some("pointer"));
        target.add_css_class("tray-icon");
        let image = gtk::Image::new();
        image.set_pixel_size(ICON_SIZE);
        target.append(&image);
        let pixmaps = Rc::new(RefCell::new(Arc::clone(&item.pixmaps)));
        image.connect_scale_factor_notify({
            let pixmaps = Rc::clone(&pixmaps);
            move |image| show_pixmap(image, &pixmaps.borrow())
        });

        let menu = gtk::Popover::builder()
            .autohide(true)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .build();
        menu.add_css_class("tray-menu");
        menu.set_offset(0, MENU_OFFSET);
        menu.set_parent(&target);
        menu.connect_visible_notify({
            let open_menus = open_menus.clone();
            let menu_visibility = menu_visibility.clone();
            let was_visible = Cell::new(false);
            move |menu| {
                let visible = menu.is_visible();
                if was_visible.replace(visible) == visible {
                    return;
                }
                if let Some(target) = menu.parent() {
                    // GTK moves a visible tooltip onto any of the shell's popovers
                    // that gets a pointer event, and then blocks until Hyprland
                    // configures it. A menu Hyprland has just dismissed never
                    // gets that configure, which froze the whole shell.
                    target.set_has_tooltip(!visible && target.tooltip_text().is_some());
                    target.trigger_tooltip_query();
                }
                let count = if visible {
                    open_menus.get() + 1
                } else {
                    open_menus.get().saturating_sub(1)
                };
                open_menus.set(count);
                menu_visibility(count > 0);
            }
        });

        let item_is_menu = Rc::new(Cell::new(false));
        let menu_path = Rc::new(RefCell::new(None::<String>));
        let (menu_sender, menu_receiver) = async_channel::unbounded();
        background::listen(menu_receiver, {
            let menu = menu.clone();
            let shared = shared.clone();
            let id = item.id.clone();
            let menu_path = menu_path.clone();
            move |items| {
                // A reply can arrive after the item was removed.
                if menu.parent().is_none() {
                    return;
                }
                let path = menu_path.borrow().clone().unwrap_or_default();
                show_menu(&menu, &shared, &id, &path, items);
            }
        });

        let request_menu = {
            let shared = shared.clone();
            let id = item.id.clone();
            let menu_path = menu_path.clone();
            move || {
                if let Some(path) = menu_path.borrow().as_deref() {
                    watcher::request_menu(&shared, &id, path, menu_sender.clone());
                }
            }
        };
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.connect_released({
            let shared = shared.clone();
            let id = item.id.clone();
            let item_is_menu = item_is_menu.clone();
            move |gesture, _, x, y| match gesture.current_button() {
                1 if item_is_menu.get() => request_menu(),
                1 => watcher::call_item(&shared, &id, "Activate", pointer_position(gesture, x, y)),
                2 => watcher::call_item(
                    &shared,
                    &id,
                    "SecondaryActivate",
                    pointer_position(gesture, x, y),
                ),
                3 => request_menu(),
                _ => {}
            }
        });
        target.add_controller(click);

        let id = item.id.clone();
        let shared_scroll = shared.clone();
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scroll.connect_scroll(move |_, dx, dy| {
            let (delta, orientation) = if dy.abs() >= dx.abs() {
                ((-dy * 120.0).round() as i32, "vertical")
            } else {
                ((-dx * 120.0).round() as i32, "horizontal")
            };
            if delta != 0 {
                watcher::call_scroll(&shared_scroll, &id, delta, orientation);
            }
            gtk::glib::Propagation::Stop
        });
        target.add_controller(scroll);

        let entry = Self {
            id: item.id.clone(),
            target,
            image,
            menu,
            item_is_menu,
            menu_path,
            pixmaps,
        };
        entry.update(item);
        entry
    }

    fn update(&self, item: &Item) {
        self.target.set_tooltip_text(item.tooltip.as_deref());
        self.target
            .set_has_tooltip(item.tooltip.is_some() && !self.menu.is_visible());
        self.pixmaps.replace(Arc::clone(&item.pixmaps));
        set_icon(&self.image, item);
        self.item_is_menu.set(item.item_is_menu);
        self.menu_path.replace(item.menu_path.clone());
    }

    fn destroy(self, tray: &gtk::Box) {
        // Popping down first keeps the open menu count balanced.
        self.menu.popdown();
        self.menu.unparent();
        tray.remove(&self.target);
    }
}

fn show_menu(
    popover: &gtk::Popover,
    shared: &watcher::SharedConnection,
    id: &ItemId,
    path: &str,
    items: Vec<MenuItem>,
) {
    let content = menu_content(items, shared, id, path, popover);
    if content.first_child().is_none() {
        return;
    }
    popover.set_child(Some(&content));
    popover.popup();
}

fn menu_content(
    items: Vec<MenuItem>,
    shared: &watcher::SharedConnection,
    id: &ItemId,
    path: &str,
    root: &gtk::Popover,
) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for item in items.into_iter().filter(|item| item.visible) {
        if item.separator {
            content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        } else if item.children.is_empty() {
            content.append(&menu_button(item, shared, id, path, root));
        } else {
            content.append(&submenu_button(item, shared, id, path, root));
        }
    }
    content
}

fn menu_button(
    item: MenuItem,
    shared: &watcher::SharedConnection,
    id: &ItemId,
    path: &str,
    root: &gtk::Popover,
) -> gtk::Button {
    let button = gtk::Button::builder().sensitive(item.enabled).build();
    button.add_css_class("flat");
    button.set_child(Some(&menu_row(&item, false)));
    let shared = shared.clone();
    let id = id.clone();
    let path = path.to_string();
    // The button lives inside the root popover; a strong reference would form
    // a cycle that keeps the popover alive after its item is removed.
    let root = root.downgrade();
    button.connect_clicked(move |_| {
        if let Some(root) = root.upgrade() {
            root.popdown();
        }
        watcher::call_menu_item(&shared, &id, &path, item.id);
    });
    button
}

fn submenu_button(
    mut item: MenuItem,
    shared: &watcher::SharedConnection,
    id: &ItemId,
    path: &str,
    root: &gtk::Popover,
) -> gtk::MenuButton {
    let submenu = gtk::Popover::builder()
        .autohide(true)
        .has_arrow(false)
        .position(gtk::PositionType::Right)
        .build();
    submenu.add_css_class("tray-menu");
    let children = std::mem::take(&mut item.children);
    submenu.set_child(Some(&menu_content(children, shared, id, path, root)));

    let button = gtk::MenuButton::builder()
        .sensitive(item.enabled)
        .direction(gtk::ArrowType::Right)
        .build();
    button.add_css_class("flat");
    button.set_child(Some(&menu_row(&item, true)));
    button.set_popover(Some(&submenu));
    button
}

fn menu_row(item: &MenuItem, submenu: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if let Some(icon_name) = &item.icon_name {
        row.append(&gtk::Image::from_icon_name(icon_name));
    } else if let Some(toggle) = &item.toggle {
        let icon = match (&toggle.kind, toggle.active) {
            (ToggleKind::Checkmark, true) => "checkbox-checked-symbolic",
            (ToggleKind::Radio, true) => "radio-checked-symbolic",
            (ToggleKind::Checkmark, false) => "checkbox-symbolic",
            (ToggleKind::Radio, false) => "radio-symbolic",
        };
        row.append(&gtk::Image::from_icon_name(icon));
    }
    let label = gtk::Label::builder()
        .label(&item.label)
        .use_underline(true)
        .xalign(0.0)
        .hexpand(true)
        .build();
    row.append(&label);
    if submenu {
        row.append(&gtk::Image::from_icon_name("pan-end-symbolic"));
    }
    row
}

fn set_icon(image: &gtk::Image, item: &Item) {
    if !item.pixmaps.is_empty() {
        show_pixmap(image, &item.pixmaps);
    } else if !item.icon_name.is_empty() {
        image.set_icon_name(Some(&item.icon_name));
    } else {
        image.set_icon_name(Some("image-missing"));
    }
}

/// Renders the pixmap at device pixels; the image still shows it at
/// `ICON_SIZE` logical pixels.
fn show_pixmap(image: &gtk::Image, pixmaps: &[Pixmap]) {
    let size = ICON_SIZE * image.scale_factor();
    let Some(pixmap) = select_pixmap(pixmaps, size) else {
        return;
    };
    let pixmap = scale_pixmap(pixmap, size);
    let texture = gtk::gdk::MemoryTexture::new(
        pixmap.width,
        pixmap.height,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &gtk::glib::Bytes::from(&pixmap.rgba),
        pixmap.width as usize * 4,
    );
    image.set_paintable(Some(&texture));
}

fn pointer_position(gesture: &gtk::GestureClick, fallback_x: f64, fallback_y: f64) -> (i32, i32) {
    let Some(event) = gesture.current_event() else {
        return position_with_origin(fallback_x, fallback_y, (0, 0));
    };
    let Some((x, y)) = event.position() else {
        return position_with_origin(fallback_x, fallback_y, (0, 0));
    };
    let Some(surface) = event.surface() else {
        return position_with_origin(x, y, (0, 0));
    };
    let Some(monitor) = surface.display().monitor_at_surface(&surface) else {
        return position_with_origin(x, y, (0, 0));
    };
    let geometry = monitor.geometry();
    position_with_origin(x, y, (geometry.x(), geometry.y()))
}

fn position_with_origin(x: f64, y: f64, origin: (i32, i32)) -> (i32, i32) {
    (origin.0 + x.round() as i32, origin.1 + y.round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_tray_clicks_by_the_monitor_origin() {
        assert_eq!(
            position_with_origin(25.6, 14.4, (-1920, 1080)),
            (-1894, 1094)
        );
    }
}
