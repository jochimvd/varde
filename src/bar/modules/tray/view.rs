use std::{cell::Cell, rc::Rc};

use gtk::prelude::*;

use super::{
    model::{Event, ICON_SIZE, Item, MenuItem, ToggleKind, scale_pixmap},
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
        let shared = shared.clone();
        let open_menus = open_menus.clone();
        let menu_visibility = menu_visibility.clone();
        let mut items = Vec::new();
        move |event| {
            apply_event(&mut items, event);
            rebuild(&tray, &items, &shared, &open_menus, &menu_visibility);
        }
    });

    tray
}

fn apply_event(items: &mut Vec<Item>, event: Event) {
    match event {
        Event::Upsert(item) => {
            if item.status == "Passive" {
                items.retain(|current| current.id != item.id);
            } else if let Some(current) = items.iter_mut().find(|current| current.id == item.id) {
                *current = item;
            } else {
                items.push(item);
            }
        }
        Event::Remove(id) => items.retain(|item| item.id != id),
    }
}

fn rebuild(
    tray: &gtk::Box,
    items: &[Item],
    shared: &watcher::SharedConnection,
    open_menus: &Rc<Cell<u32>>,
    menu_visibility: &Rc<dyn Fn(bool)>,
) {
    while let Some(child) = tray.first_child() {
        tray.remove(&child);
    }
    for item in items {
        let target = gtk::Box::builder()
            .focusable(false)
            .valign(gtk::Align::Center)
            .build();
        target.set_cursor_from_name(Some("pointer"));
        target.add_css_class("tray-icon");
        target.set_tooltip_text(item.tooltip.as_deref());
        target.append(&icon(item));

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
        let (menu_sender, menu_receiver) = async_channel::unbounded();
        background::listen(menu_receiver, {
            let menu = menu.clone();
            let shared = shared.clone();
            let id = item.id.clone();
            let path = item.menu_path.clone().unwrap_or_default();
            move |items| show_menu(&menu, &shared, &id, &path, items)
        });

        let id = item.id.clone();
        let shared_click = shared.clone();
        let item_is_menu = item.item_is_menu;
        let menu_path = item.menu_path.clone();
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.connect_released(move |gesture, _, x, y| match gesture.current_button() {
            1 if item_is_menu => {
                if let Some(path) = &menu_path {
                    watcher::request_menu(&shared_click, &id, path, menu_sender.clone());
                }
            }
            1 => watcher::call_item(
                &shared_click,
                &id,
                "Activate",
                pointer_position(gesture, x, y),
            ),
            2 => watcher::call_item(
                &shared_click,
                &id,
                "SecondaryActivate",
                pointer_position(gesture, x, y),
            ),
            3 => {
                if let Some(path) = &menu_path {
                    watcher::request_menu(&shared_click, &id, path, menu_sender.clone());
                }
            }
            _ => {}
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
        tray.append(&target);
    }
}

fn show_menu(
    popover: &gtk::Popover,
    shared: &watcher::SharedConnection,
    id: &super::model::ItemId,
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
    id: &super::model::ItemId,
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
    id: &super::model::ItemId,
    path: &str,
    root: &gtk::Popover,
) -> gtk::Button {
    let button = gtk::Button::builder().sensitive(item.enabled).build();
    button.add_css_class("flat");
    button.set_child(Some(&menu_row(&item, false)));
    let shared = shared.clone();
    let id = id.clone();
    let path = path.to_string();
    let root = root.clone();
    button.connect_clicked(move |_| {
        root.popdown();
        watcher::call_menu_item(&shared, &id, &path, item.id);
    });
    button
}

fn submenu_button(
    mut item: MenuItem,
    shared: &watcher::SharedConnection,
    id: &super::model::ItemId,
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

fn icon(item: &Item) -> gtk::Image {
    let image = gtk::Image::new();
    image.set_pixel_size(ICON_SIZE);
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
    image
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
