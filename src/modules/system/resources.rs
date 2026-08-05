use std::{cell::Cell, fs, process::Command, rc::Rc};

use gtk::{glib, prelude::*};

const UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

pub fn cpu() -> gtk::Label {
    let label = label("cpu");
    let previous = Rc::new(Cell::new(read_cpu().unwrap_or_default()));
    update_cpu(&label, 0);
    glib::timeout_add_local(UPDATE_INTERVAL, {
        let label = label.clone();
        let previous = previous.clone();
        move || {
            if let Some(current) = read_cpu() {
                let usage = cpu_usage(previous.get(), current);
                previous.set(current);
                update_cpu(&label, usage);
            }
            glib::ControlFlow::Continue
        }
    });
    launch_btop_on_click(&label);
    label
}

pub fn memory() -> gtk::Label {
    let label = label("memory");
    update_memory(&label);
    glib::timeout_add_local(UPDATE_INTERVAL, {
        let label = label.clone();
        move || {
            update_memory(&label);
            glib::ControlFlow::Continue
        }
    });
    launch_btop_on_click(&label);
    label
}

fn label(name: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_widget_name(name);
    label.add_css_class(name);
    label
}

fn update_cpu(label: &gtk::Label, usage: u8) {
    label.set_text(&format!("󰍛 {usage:>2}%"));
    label.set_tooltip_text(Some(&cpu_tooltip(usage)));
    set_critical(label, usage >= 90);
}

fn cpu_tooltip(usage: u8) -> String {
    let mut lines = vec![format!("CPU: {usage}%")];
    if let Some(load) = load_average() {
        lines.push(format!("Load: {load}"));
    }

    let threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    lines.push(format!("Threads: {threads}"));
    if let Some(ghz) = average_cpu_frequency() {
        lines.push(format!("Frequency: {ghz:.2} GHz"));
    }
    lines.join("\n")
}

fn load_average() -> Option<String> {
    let load = fs::read_to_string("/proc/loadavg").ok()?;
    let values: Vec<_> = load.split_whitespace().take(3).collect();
    (values.len() == 3).then(|| values.join("  "))
}

fn average_cpu_frequency() -> Option<f64> {
    let frequencies: Vec<u64> = fs::read_dir("/sys/devices/system/cpu")
        .ok()?
        .flatten()
        .filter_map(|entry| {
            fs::read_to_string(entry.path().join("cpufreq/scaling_cur_freq"))
                .ok()?
                .trim()
                .parse()
                .ok()
        })
        .collect();
    (!frequencies.is_empty())
        .then(|| frequencies.iter().sum::<u64>() as f64 / frequencies.len() as f64 / 1_000_000.0)
}

fn update_memory(label: &gtk::Label) {
    if let Some(memory) = read_memory() {
        label.set_text(&format!("󰘚 {:>2}%", memory.percentage));
        label.set_tooltip_text(Some(&format!(
            "Memory: {}%\n{:.1} / {:.1} GiB",
            memory.percentage,
            memory.used as f64 / 1024.0 / 1024.0,
            memory.total as f64 / 1024.0 / 1024.0
        )));
        set_critical(label, memory.percentage >= 80);
    }
}

fn set_critical(label: &gtk::Label, critical: bool) {
    if critical {
        label.add_css_class("critical");
    } else {
        label.remove_css_class("critical");
    }
}

fn launch_btop_on_click(label: &gtk::Label) {
    let click = gtk::GestureClick::new();
    click.connect_released(|_, _, _, _| {
        let _ = Command::new("hyprctl")
            .args([
                "dispatch",
                r#"hl.dsp.exec_cmd("$TERMINAL -e btop", { tag = "+floating-window" })"#,
            ])
            .spawn();
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

struct Memory {
    total: u64,
    used: u64,
    percentage: u8,
}

fn read_memory() -> Option<Memory> {
    parse_memory(&fs::read_to_string("/proc/meminfo").ok()?)
}

fn parse_memory(meminfo: &str) -> Option<Memory> {
    let mut total = None;
    let mut available = None;
    for line in meminfo.lines() {
        let mut fields = line.split_whitespace();
        match fields.next()? {
            "MemTotal:" => total = fields.next()?.parse::<u64>().ok(),
            "MemAvailable:" => available = fields.next()?.parse::<u64>().ok(),
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
        let memory = parse_memory("MemTotal:       1000 kB\nMemAvailable:    250 kB\n").unwrap();
        assert_eq!(memory.used, 750);
        assert_eq!(memory.percentage, 75);
    }
}
