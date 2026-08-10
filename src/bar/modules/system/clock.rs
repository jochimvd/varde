use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gtk::{glib, prelude::*};

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const POPOVER_TOP: i32 = 18;
const CALENDAR_STATS_WIDTH: i32 = 29;
const WORLD_CLOCKS: [(&str, &str); 4] = [
    ("Oslo", "Europe/Oslo"),
    ("Kolkata", "Asia/Kolkata"),
    ("New York", "America/New_York"),
    ("UTC", "Etc/UTC"),
];
const SHORT_MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn date() -> gtk::Label {
    let label = label("date");
    let alternate = Rc::new(Cell::new(false));
    let calendar = calendar_popover(&label);
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
    click.connect_released(move |_, _, _, _| toggle_popover(&calendar));
    label.add_controller(click);

    let alternate_click = gtk::GestureClick::new();
    alternate_click.set_button(3);
    alternate_click.connect_released({
        let label = label.clone();
        move |_, _, _, _| {
            alternate.set(!alternate.get());
            update_date(&label, alternate.get());
        }
    });
    label.add_controller(alternate_click);

    label
}

pub fn time() -> gtk::Label {
    let label = label("time");
    let alternate = Rc::new(Cell::new(false));
    let world_clocks = world_clock_popover(&label);

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
    click.connect_released(move |_, _, _, _| toggle_popover(&world_clocks));
    label.add_controller(click);

    let alternate_click = gtk::GestureClick::new();
    alternate_click.set_button(3);
    alternate_click.connect_released({
        let label = label.clone();
        move |_, _, _, _| {
            alternate.set(!alternate.get());
            update_time(&label, alternate.get());
        }
    });
    label.add_controller(alternate_click);

    label
}

fn label(name: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_cursor_from_name(Some("pointer"));
    label.set_widget_name(name);
    label.add_css_class("clock");
    label.add_css_class(name);
    label
}

fn update_date(label: &gtk::Label, alternate: bool) {
    let now = glib::DateTime::now_local().expect("local time is available");
    label.set_text(&format_date(&now, alternate));
}

fn calendar_popover(label: &gtk::Label) -> gtk::Popover {
    let stats = gtk::Label::builder()
        .width_chars(CALENDAR_STATS_WIDTH)
        .xalign(0.0)
        .build();
    stats.add_css_class("calendar-stats");
    let calendar = Rc::new(RefCell::new(new_calendar(&stats)));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
    content.add_css_class("bar-popover-panel");
    content.add_css_class("date-calendar");
    content.append(&*calendar.borrow());
    content.append(&stats);

    let popover = module_popover(label, &content);
    popover.add_css_class("date-popover");
    popover.connect_visible_notify({
        let calendar = calendar.clone();
        move |popover| {
            if popover.is_visible() {
                let today = glib::DateTime::now_local().expect("local time is available");
                calendar.borrow().select_day(&today);
            }
        }
    });
    popover.connect_closed({
        let calendar = calendar.clone();
        let content = content.clone();
        let stats = stats.clone();
        move |_| {
            content.remove(&*calendar.borrow());
            let next = new_calendar(&stats);
            content.prepend(&next);
            calendar.replace(next);
        }
    });
    popover
}

fn new_calendar(stats: &gtk::Label) -> gtk::Calendar {
    let calendar = gtk::Calendar::new();
    calendar.set_show_day_names(true);
    calendar.set_show_heading(true);
    calendar.set_show_week_numbers(false);
    update_calendar_stats(&calendar.date(), stats);
    calendar.connect_day_selected({
        let stats = stats.clone();
        move |calendar| update_calendar_stats(&calendar.date(), &stats)
    });
    calendar
}

struct WorldClockRow {
    time: gtk::Label,
    day: gtk::Label,
    offset: gtk::Label,
    timezone: glib::TimeZone,
}

fn world_clock_popover(label: &gtk::Label) -> gtk::Popover {
    let title = gtk::Label::builder()
        .label("World time")
        .xalign(0.0)
        .build();
    title.add_css_class("world-clock-title");

    let grid = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(6)
        .build();
    grid.add_css_class("world-clock-grid");
    let mut rows = Vec::new();
    for (row, (name, identifier)) in WORLD_CLOCKS.into_iter().enumerate() {
        let Some(timezone) = glib::TimeZone::from_identifier(Some(identifier)) else {
            continue;
        };
        let name = gtk::Label::builder().label(name).xalign(0.0).build();
        name.add_css_class("world-clock-name");
        let time = gtk::Label::builder().width_chars(5).xalign(1.0).build();
        time.add_css_class("world-clock-time");
        if row == 0 {
            time.add_css_class("primary");
        }
        let day = gtk::Label::builder().width_chars(1).xalign(0.5).build();
        day.add_css_class("world-clock-day");
        let offset = gtk::Label::builder().width_chars(6).xalign(1.0).build();
        offset.add_css_class("world-clock-offset");
        grid.attach(&name, 0, row as i32, 1, 1);
        grid.attach(&time, 1, row as i32, 1, 1);
        grid.attach(&day, 2, row as i32, 1, 1);
        grid.attach(&offset, 3, row as i32, 1, 1);
        rows.push(WorldClockRow {
            time,
            day,
            offset,
            timezone,
        });
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.add_css_class("bar-popover-panel");
    content.add_css_class("world-clocks");
    content.append(&title);
    content.append(&grid);
    let popover = module_popover(label, &content);
    popover.add_css_class("time-popover");
    let rows = Rc::new(rows);
    let refresh = Rc::new(RefCell::new(None));
    popover.connect_visible_notify({
        let rows = rows.clone();
        let refresh = refresh.clone();
        move |popover| {
            if popover.is_visible() {
                update_world_clocks(&rows);
                let rows = rows.clone();
                *refresh.borrow_mut() = Some(glib::timeout_add_seconds_local(1, move || {
                    update_world_clocks(&rows);
                    glib::ControlFlow::Continue
                }));
            } else if let Some(refresh) = refresh.borrow_mut().take() {
                refresh.remove();
            }
        }
    });
    popover
}

fn module_popover(label: &gtk::Label, content: &gtk::Box) -> gtk::Popover {
    let popover = gtk::Popover::builder()
        .autohide(true)
        .has_arrow(false)
        .position(gtk::PositionType::Bottom)
        .child(content)
        .build();
    popover.add_css_class("bar-popover");
    popover.set_halign(gtk::Align::Center);
    popover.set_offset(0, POPOVER_TOP);
    popover.set_parent(label);
    popover
}

fn toggle_popover(popover: &gtk::Popover) {
    if popover.is_visible() {
        popover.popdown();
    } else {
        popover.popup();
    }
}

fn year_progress(date: &glib::DateTime) -> i32 {
    let days_in_year = if is_leap_year(date.year()) { 366 } else { 365 };
    date.day_of_year() * 100 / days_in_year
}

fn update_calendar_stats(date: &glib::DateTime, stats: &gtk::Label) {
    let today = glib::DateTime::now_local().expect("local time is available");
    stats.set_label(&format!(
        "{} · D {} · W {} · Y {}%",
        relative_date(&today, date),
        date.day_of_year(),
        date.week_of_year(),
        year_progress(date)
    ));
}

fn relative_date(today: &glib::DateTime, date: &glib::DateTime) -> String {
    let days = calendar_date(today).days_between(&calendar_date(date));
    match days {
        0 => "Today".into(),
        days if days < 0 => format!("−{}d", -days),
        days => format!("+{days}d"),
    }
}

fn calendar_date(date: &glib::DateTime) -> glib::Date {
    let month = match date.month() {
        1 => glib::DateMonth::January,
        2 => glib::DateMonth::February,
        3 => glib::DateMonth::March,
        4 => glib::DateMonth::April,
        5 => glib::DateMonth::May,
        6 => glib::DateMonth::June,
        7 => glib::DateMonth::July,
        8 => glib::DateMonth::August,
        9 => glib::DateMonth::September,
        10 => glib::DateMonth::October,
        11 => glib::DateMonth::November,
        12 => glib::DateMonth::December,
        _ => unreachable!("GLib returned an invalid month"),
    };
    glib::Date::from_dmy(
        date.day_of_month() as u8,
        month,
        date.year().try_into().expect("GLib year fits GDate"),
    )
    .expect("valid GLib date")
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn update_time(label: &gtk::Label, alternate: bool) {
    let now = glib::DateTime::now_local().expect("local time is available");
    label.set_text(
        &now.format(if alternate { "%H:%M:%S (%Z)" } else { "%H:%M" })
            .expect("valid time format"),
    );
}

fn update_world_clocks(rows: &[WorldClockRow]) {
    let local = glib::DateTime::now_local().expect("local time is available");
    for row in rows {
        let Ok(now) = glib::DateTime::now(&row.timezone) else {
            continue;
        };
        if let Ok(time) = now.format("%H:%M") {
            row.time.set_label(&time);
        }
        let (icon, class) = day_relation(&local, &now);
        row.day.set_label(icon);
        for class in ["yesterday", "today", "tomorrow"] {
            row.day.remove_css_class(class);
        }
        row.day.add_css_class(class);
        let offset = now.utc_offset().as_seconds() - local.utc_offset().as_seconds();
        row.offset.set_label(&format_offset(offset));
    }
}

fn day_relation(local: &glib::DateTime, remote: &glib::DateTime) -> (&'static str, &'static str) {
    let local = (local.year(), local.day_of_year());
    let remote = (remote.year(), remote.day_of_year());
    match remote.cmp(&local) {
        std::cmp::Ordering::Less => ("←", "yesterday"),
        std::cmp::Ordering::Equal => ("●", "today"),
        std::cmp::Ordering::Greater => ("→", "tomorrow"),
    }
}

fn format_offset(seconds: i64) -> String {
    if seconds == 0 {
        return "±0h".into();
    }
    let sign = if seconds < 0 { '−' } else { '+' };
    let minutes = seconds.abs() / 60;
    let hours = minutes / 60;
    let minutes = minutes % 60;
    if minutes == 0 {
        format!("{sign}{hours}h")
    } else {
        format!("{sign}{hours}:{minutes:02}")
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_en_gb_date() {
        let date = glib::DateTime::from_local(2026, 8, 4, 12, 0, 0.0).unwrap();
        assert_eq!(format_date(&date, false), "Tue 04 Aug");
        assert_eq!(format_date(&date, true), "04/08/2026");
    }

    #[test]
    fn year_progress_reaches_100_on_last_day() {
        let date = glib::DateTime::from_local(2024, 12, 31, 12, 0, 0.0).unwrap();
        assert_eq!(year_progress(&date), 100);
    }

    #[test]
    fn describes_selected_date_relative_to_today() {
        let today = glib::DateTime::from_local(2026, 12, 31, 12, 0, 0.0).unwrap();
        let tomorrow = glib::DateTime::from_local(2027, 1, 1, 12, 0, 0.0).unwrap();
        assert_eq!(relative_date(&today, &today), "Today");
        assert_eq!(relative_date(&today, &tomorrow), "+1d");
    }

    #[test]
    fn formats_world_clock_offsets() {
        assert_eq!(format_offset(0), "±0h");
        assert_eq!(format_offset(12_600), "+3:30");
        assert_eq!(format_offset(-21_600), "−6h");
    }
}
