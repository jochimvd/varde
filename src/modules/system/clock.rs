use std::{cell::Cell, rc::Rc};

use gtk::{glib, prelude::*};

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const SHORT_MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn date() -> gtk::Label {
    let label = label("date");
    let alternate = Rc::new(Cell::new(false));
    update_date(&label, alternate.get());
    glib::timeout_add_seconds_local(1, {
        let label = label.clone();
        let alternate = alternate.clone();
        move || {
            update_date(&label, alternate.get());
            glib::ControlFlow::Continue
        }
    });

    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released({
        let label = label.clone();
        let alternate = alternate.clone();
        move |_, _, _, _| {
            alternate.set(!alternate.get());
            update_date(&label, alternate.get());
        }
    });
    label.add_controller(click);

    label
}

pub fn time() -> gtk::Label {
    let label = label("time");
    let alternate = Rc::new(Cell::new(false));

    update_time(&label, alternate.get());
    glib::timeout_add_seconds_local(1, {
        let label = label.clone();
        let alternate = alternate.clone();
        move || {
            update_time(&label, alternate.get());
            glib::ControlFlow::Continue
        }
    });

    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released({
        let label = label.clone();
        move |_, _, _, _| {
            alternate.set(!alternate.get());
            update_time(&label, alternate.get());
        }
    });
    label.add_controller(click);

    label
}

fn label(name: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_widget_name(name);
    label.add_css_class("clock");
    label.add_css_class(name);
    label
}

fn update_date(label: &gtk::Label, alternate: bool) {
    let now = glib::DateTime::now_local().expect("local time is available");
    label.set_text(&format_date(&now, alternate));
}

fn update_time(label: &gtk::Label, alternate: bool) {
    let now = glib::DateTime::now_local().expect("local time is available");
    label.set_text(
        &now.format(if alternate { "%H:%M:%S (%Z)" } else { "%H:%M" })
            .expect("valid time format"),
    );
    label.set_tooltip_markup(Some(&time_tooltip()));
}

fn format_date(date: &glib::DateTime, alternate: bool) -> String {
    if alternate {
        format!(
            "{:02}/{:02}/{:04}",
            date.day_of_month(),
            date.month(),
            date.year()
        )
    } else {
        format!(
            "{} {:02} {}",
            WEEKDAYS[(date.day_of_week() - 1) as usize],
            date.day_of_month(),
            short_month_name(date.month())
        )
    }
}

fn short_month_name(month: i32) -> &'static str {
    SHORT_MONTHS[(month - 1) as usize]
}

fn time_tooltip() -> String {
    let clocks = [
        ("Brussels", "Europe/Brussels"),
        ("Kolkata", "Asia/Kolkata"),
        ("New York", "America/New_York"),
        ("UTC", "Etc/UTC"),
    ];
    let rows = clocks
        .into_iter()
        .filter_map(|(name, zone)| {
            let timezone = glib::TimeZone::new(Some(zone));
            glib::DateTime::now(&timezone)
                .ok()
                .and_then(|time| time.format("%H:%M").ok())
                .map(|time| format!("{name:<8}  {time}"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("<b>World time</b>\n{rows}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_en_gb_date() {
        let date = glib::DateTime::from_local(2026, 8, 4, 12, 0, 0.0).unwrap();
        assert_eq!(format_date(&date, false), "Tue 04 Aug");
        assert_eq!(format_date(&date, true), "04/08/2026");
    }
}
