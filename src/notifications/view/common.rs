use std::path::Path;

use gio::prelude::*;
use gtk::{gdk, glib, prelude::*};

use super::super::model::{Group, Notification};

pub(super) fn set_picture(image: &gtk::Image, notification: &Notification) -> bool {
    if let Some(data) = &notification.image_data {
        let format = if data.has_alpha {
            gdk::MemoryFormat::R8g8b8a8
        } else {
            gdk::MemoryFormat::R8g8b8
        };
        let bytes = glib::Bytes::from_owned(data.bytes.clone());
        let texture =
            gdk::MemoryTexture::new(data.width, data.height, format, &bytes, data.rowstride);
        image.set_from_gicon(&texture);
        return true;
    }
    if let Some(icon) = notification.image.as_deref().map(notification_icon) {
        image.set_from_gicon(&icon);
        return true;
    }
    false
}

pub(super) fn progress_bar(value: u8) -> gtk::ProgressBar {
    let bar = gtk::ProgressBar::builder()
        .fraction(f64::from(value) / 100.0)
        .hexpand(true)
        .build();
    bar.add_css_class("notification-progress");
    bar
}

pub(super) fn application(group: &Group) -> (String, Option<gio::Icon>) {
    if let Some(entry) = group.desktop_entry.as_deref().and_then(desktop_info) {
        return (entry.display_name().to_string(), entry.icon());
    }
    let icon = group.icon.as_deref().map(notification_icon);
    (group.name.clone(), icon)
}

fn notification_icon(icon: &str) -> gio::Icon {
    if Path::new(icon).is_absolute() {
        gio::FileIcon::new(&gio::File::for_path(icon)).upcast()
    } else if icon.starts_with("file://") {
        gio::FileIcon::new(&gio::File::for_uri(icon)).upcast()
    } else {
        gio::ThemedIcon::new(icon).upcast()
    }
}

pub(super) fn notification_time(timestamp: Option<i64>) -> Option<String> {
    glib::DateTime::from_unix_local(timestamp?)
        .ok()?
        .format("%H:%M")
        .ok()
        .map(|time| time.to_string())
}

fn desktop_info(id: &str) -> Option<gio::DesktopAppInfo> {
    gio::DesktopAppInfo::new(id).or_else(|| {
        (!id.ends_with(".desktop"))
            .then(|| gio::DesktopAppInfo::new(&format!("{id}.desktop")))
            .flatten()
    })
}

pub(super) fn message(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    label.add_css_class(class);
    label
}
