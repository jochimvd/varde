use gtk::prelude::*;

use super::{
    model::{Event, ICON_SIZE, Item, scale_pixmap},
    watcher,
};
use crate::background;

const ICON_GAP: i32 = 2;

pub fn widget() -> gtk::Box {
    let tray = gtk::Box::builder()
        .spacing(ICON_GAP)
        .valign(gtk::Align::Center)
        .build();
    tray.set_widget_name("tray");
    tray.add_css_class("tray");

    let (sender, receiver) = async_channel::unbounded();
    background::spawn("tray-watcher", move || watcher::run(sender));
    background::listen(receiver, {
        let tray = tray.clone();
        let mut items = Vec::new();
        move |event| {
            apply_event(&mut items, event);
            rebuild(&tray, &items);
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

fn rebuild(tray: &gtk::Box, items: &[Item]) {
    while let Some(child) = tray.first_child() {
        tray.remove(&child);
    }
    for item in items {
        let button = gtk::Button::builder().focusable(false).build();
        button.add_css_class("tray-icon");
        button.set_tooltip_text(item.tooltip.as_deref());
        button.set_child(Some(&icon(item)));

        let id = item.id.clone();
        let item_is_menu = item.item_is_menu;
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.connect_released(move |gesture, _, x, y| match gesture.current_button() {
            1 if item_is_menu => {
                watcher::call_item(&id, "ContextMenu", pointer_position(gesture, x, y))
            }
            1 => watcher::call_item(&id, "Activate", pointer_position(gesture, x, y)),
            3 => watcher::call_item(&id, "ContextMenu", pointer_position(gesture, x, y)),
            _ => {}
        });
        button.add_controller(click);
        tray.append(&button);
    }
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
