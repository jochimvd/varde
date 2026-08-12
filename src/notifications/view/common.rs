use std::path::Path;

use gio::prelude::*;
use gtk::{gdk, glib, prelude::*};

use super::super::model::{Group, Notification};
use super::super::state::Picture;

pub(super) fn activation_token(widget: &impl IsA<gtk::Widget>) -> Option<String> {
    widget
        .as_ref()
        .display()
        .app_launch_context()
        .startup_notify_id(None::<&gio::AppInfo>, &[])
        .map(Into::into)
}

pub(super) fn set_picture(image: &gtk::Image, notification: &Notification) -> bool {
    match notification.picture.as_ref() {
        Some(Picture::Pixels(data)) => {
            let bytes = glib::Bytes::from_owned(data.bytes.clone());
            let texture = gdk::MemoryTexture::new(
                data.width,
                data.height,
                gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                data.rowstride,
            );
            image.set_from_gicon(&texture);
            true
        }
        Some(Picture::Themed(icon))
            if gtk::IconTheme::for_display(&image.display()).has_icon(icon) =>
        {
            image.set_icon_name(Some(icon));
            true
        }
        _ => false,
    }
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
    let now = glib::DateTime::now_local().ok()?;
    format_notification_time(timestamp?, &now)
}

fn format_notification_time(timestamp: i64, now: &glib::DateTime) -> Option<String> {
    if now.to_unix() - timestamp < 60 {
        return Some("now".into());
    }
    let received = glib::DateTime::from_unix_local(timestamp).ok()?;
    let time = received.format("%H:%M").ok()?;
    if received.ymd() == now.ymd() {
        return Some(time.to_string());
    }
    if received.ymd() == now.add_days(-1).ok()?.ymd() {
        return Some(format!("Yesterday · {time}"));
    }
    if (2..now.day_of_week()).any(|days| {
        now.add_days(-days)
            .is_ok_and(|date| received.ymd() == date.ymd())
    }) {
        let weekday = received.format("%a").ok()?;
        return Some(format!("{weekday} · {time}"));
    }
    let date = received.format("%-d %b").ok()?;
    if received.year() == now.year() {
        Some(format!("{date} · {time}"))
    } else {
        Some(format!("{date} {} · {time}", received.year()))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn local(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> glib::DateTime {
        glib::DateTime::from_local(year, month, day, hour, minute, 0.0).unwrap()
    }

    #[test]
    fn formats_notification_times_by_calendar_distance() {
        let now = local(2026, 8, 13, 18, 0);
        let format = |date: glib::DateTime| format_notification_time(date.to_unix(), &now).unwrap();

        assert_eq!(format(local(2026, 8, 13, 18, 0)), "now");
        assert_eq!(format(local(2026, 8, 13, 17, 59)), "17:59");
        assert_eq!(format(local(2026, 8, 13, 9, 15)), "09:15");
        assert_eq!(format(local(2026, 8, 12, 23, 45)), "Yesterday · 23:45");
        assert_eq!(format(local(2026, 8, 10, 8, 5)), "Mon · 08:05");
        assert_eq!(format(local(2026, 8, 9, 8, 5)), "9 Aug · 08:05");
        assert_eq!(format(local(2026, 1, 3, 8, 5)), "3 Jan · 08:05");
        assert_eq!(format(local(2025, 12, 3, 8, 5)), "3 Dec 2025 · 08:05");
    }
}
