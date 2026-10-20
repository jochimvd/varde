use std::fs;

use gtk::{glib, prelude::*};

use super::set_critical;
use crate::bar::{Msg, Updates, modules::open_floating};

const UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Samples CPU usage and memory.
pub fn watch(updates: &Updates) {
    updates.send(Msg::Cpu(CpuUsage::new(0)));
    if let Some(current) = read_memory() {
        updates.send(Msg::Memory(current));
    }
    let mut previous = read_cpu().unwrap_or_default();
    let updates = updates.clone();
    glib::timeout_add_local(UPDATE_INTERVAL, move || {
        if let Some(current) = read_cpu() {
            updates.send(Msg::Cpu(CpuUsage::new(cpu_usage(previous, current))));
            previous = current;
        }
        if let Some(current) = read_memory() {
            updates.send(Msg::Memory(current));
        }
        glib::ControlFlow::Continue
    });
}

pub fn cpu() -> gtk::Label {
    let label = label("cpu");
    launch_btop_on_click(&label);
    label
}

pub fn memory() -> gtk::Label {
    let label = label("memory");
    launch_btop_on_click(&label);
    label
}

fn label(name: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_widget_name(name);
    label.add_css_class(name);
    label
}

/// CPU usage, with the load average at the time.
#[derive(Clone)]
pub struct CpuUsage {
    percent: u8,
    tooltip: String,
}

impl CpuUsage {
    fn new(percent: u8) -> Self {
        Self {
            percent,
            tooltip: cpu_tooltip(),
        }
    }
}

pub fn show_cpu(label: &gtk::Label, usage: &CpuUsage) {
    label.set_text(&format!("󰍛 {:>2}%", usage.percent));
    label.set_tooltip_text(Some(&usage.tooltip));
    set_critical(label, usage.percent >= 90);
}

fn cpu_tooltip() -> String {
    load_average()
        .map(|load| format!("Load  {load}"))
        .unwrap_or_else(|| "Load  —".into())
}

fn load_average() -> Option<String> {
    let load = fs::read_to_string("/proc/loadavg").ok()?;
    let values: Vec<_> = load.split_whitespace().take(3).collect();
    (values.len() == 3).then(|| values.join("  "))
}

pub fn show_memory(label: &gtk::Label, memory: &Memory) {
    label.set_text(&format!("󰘚 {:>2}%", memory.percentage));
    label.set_tooltip_text(Some(&format!(
        "{:.1} / {:.1} GiB",
        memory.used as f64 / 1024.0 / 1024.0,
        memory.total as f64 / 1024.0 / 1024.0
    )));
    set_critical(label, memory.percentage >= 80);
}

fn launch_btop_on_click(label: &gtk::Label) {
    label.set_cursor_from_name(Some("pointer"));
    let click = gtk::GestureClick::new();
    click.connect_released(|_, _, _, _| {
        open_floating("$TERMINAL -e btop");
    });
    label.add_controller(click);
}

#[derive(Clone, Copy, Default)]
struct Cpu {
    total: u64,
    idle: u64,
}

fn read_cpu() -> Option<Cpu> {
    parse_cpu(&fs::read_to_string("/proc/stat").ok()?)
}

fn parse_cpu(stat: &str) -> Option<Cpu> {
    let fields: Vec<u64> = stat
        .lines()
        .next()?
        .split_whitespace()
        .skip(1)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if fields.len() < 4 {
        return None;
    }
    Some(Cpu {
        total: fields.iter().take(8).sum(),
        idle: fields[3] + fields.get(4).copied().unwrap_or_default(),
    })
}

fn cpu_usage(previous: Cpu, current: Cpu) -> u8 {
    let total = current.total.saturating_sub(previous.total);
    if total == 0 {
        return 0;
    }
    ((100 * (total.saturating_sub(current.idle.saturating_sub(previous.idle))) / total).min(100))
        as u8
}

#[derive(Clone)]
pub struct Memory {
    total: u64,
    used: u64,
    percentage: u8,
}

fn read_memory() -> Option<Memory> {
    parse_memory(&fs::read_to_string("/proc/meminfo").ok()?)
}

fn parse_memory(meminfo: &str) -> Option<Memory> {
    let mut total = None::<u64>;
    let mut available = None::<u64>;
    for line in meminfo.lines() {
        let mut fields = line.split_whitespace();
        let Some(key) = fields.next() else {
            continue;
        };
        match key {
            "MemTotal:" => total = fields.next().and_then(|value| value.parse().ok()),
            "MemAvailable:" => available = fields.next().and_then(|value| value.parse().ok()),
            _ => {}
        }
    }
    let total = total?;
    let used = total.saturating_sub(available?);
    Some(Memory {
        total,
        used,
        percentage: (used * 100 / total) as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_and_calculates_usage() {
        let before = parse_cpu("cpu  10 20 30 40 5 6 7 8\n").unwrap();
        let after = parse_cpu("cpu  20 30 40 50 10 12 14 16\n").unwrap();
        assert_eq!(cpu_usage(before, after), 77);
    }

    #[test]
    fn excludes_guest_time_from_cpu_total() {
        let before = parse_cpu("cpu  10 20 30 40 5 6 7 8 0 0\n").unwrap();
        let after = parse_cpu("cpu  10 20 30 40 5 6 7 8 900 800\n").unwrap();
        assert_eq!(before.total, 126);
        assert_eq!(after.total, before.total);
        assert_eq!(cpu_usage(before, after), 0);
    }

    #[test]
    fn parses_memory_using_available_memory() {
        let memory = parse_memory("MemTotal:       1000 kB\n\nMemAvailable:    250 kB\n").unwrap();
        assert_eq!(memory.used, 750);
        assert_eq!(memory.percentage, 75);
    }
}
