use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gtk::prelude::*;

use super::{
    model::{Event, ICON_SIZE, Item, ItemId, MenuItem, ToggleKind, scale_pixmap},
    watcher,
};
use crate::background;

const MENU_OFFSET: i32 = 12;

pub fn widget(menu_visibility: impl Fn(bool) + 'static) -> gtk::Box {
    let tray = gtk::Box::builder().valign(gtk::Align::Center).build();
    tray.set_widget_name("tray");
    tray.add_css_class("tray");

    let (sender, receiver) = async_channel::unbounded();
    let shared = watcher::SharedConnection::default();
    let open_menus = Rc::new(Cell::new(0_u32));
    let menu_visibility: Rc<dyn Fn(bool)> = Rc::new(menu_visibility);
    let watcher_connection = shared.clone();
    background::spawn("tray-watcher", move || {
        watcher::run(sender, watcher_connection)
    });
    background::listen(receiver, {
        let tray = tray.clone();
        let mut entries = Vec::<Entry>::new();
        move |event| match event {
            Event::Upsert(item) => {
                let index = entries.iter().position(|entry| entry.id == item.id);
                if item.status == "Passive" {
                    if let Some(index) = index {
                        entries.remove(index).destroy(&tray);
                    }
                } else if let Some(index) = index {
                    entries[index].update(&item);
                } else {
                    let entry = Entry::new(&item, &shared, &open_menus, &menu_visibility);
                    tray.append(&entry.target);
                    entries.push(entry);
                }
            }
            Event::Remove(id) => {
                if let Some(index) = entries.iter().position(|entry| entry.id == id) {
                    entries.remove(index).destroy(&tray);
                }
            }
        }
    });

    tray
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
        };
        entry.update(item);
        entry
    }

    fn update(&self, item: &Item) {
        self.target.set_tooltip_text(item.tooltip.as_deref());
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
    if let Some(pixmap) = &item.pixmap {
        let pixmap = scale_pixmap(pixmap, ICON_SIZE);
        let texture = gtk::gdk::MemoryTexture::new(
            pixmap.width,
            pixmap.height,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &gtk::glib::Bytes::from(&pixmap.rgba),
            pixmap.width as usize * 4,
        );
        image.set_paintable(Some(&texture));
    } else if !item.icon_name.is_empty() {
        image.set_icon_name(Some(&item.icon_name));
    } else {
        image.set_icon_name(Some("image-missing"));
    }
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
