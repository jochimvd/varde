use std::time::Instant;

#[derive(Debug, Clone)]
pub(super) struct TrafficSample {
    interface: String,
    received: u64,
    sent: u64,
    at: Instant,
}

pub(super) enum State {
    Disconnected,
    Wifi {
        interface: String,
        network: String,
        frequency_mhz: u32,
        signal: u8,
        address: String,
        received: u64,
        sent: u64,
    },
    Ethernet {
        interface: String,
        address: String,
        received: u64,
        sent: u64,
    },
}

impl State {
    pub(super) fn class(&self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            _ => "",
        }
    }

    pub(super) fn icon(&self) -> &'static str {
        match self {
            Self::Disconnected => "󰤭",
            Self::Ethernet { .. } => "󰈀",
            Self::Wifi { signal, .. } => wifi_icon(*signal),
        }
    }

    pub(super) fn tooltip(&self, previous: Option<&TrafficSample>) -> String {
        match self {
            Self::Disconnected => "Disconnected".into(),
            Self::Wifi {
                interface,
                network,
                frequency_mhz,
                signal,
                address,
                received,
                sent,
            } => format!(
                "󰣸  {network} ({} GHz, {signal}%)\n󰛳  {address}\n{}",
                *frequency_mhz as f32 / 1000.0,
                traffic(interface, *received, *sent, previous)
            ),
            Self::Ethernet {
                interface,
                address,
                received,
                sent,
            } => format!(
                "  {interface}\n󰛳  {address}\n{}",
                traffic(interface, *received, *sent, previous)
            ),
        }
    }

    pub(super) fn traffic_sample(&self) -> Option<TrafficSample> {
        match self {
            Self::Wifi {
                interface,
                received,
                sent,
                ..
            }
            | Self::Ethernet {
                interface,
                received,
                sent,
                ..
            } => Some(TrafficSample {
                interface: interface.clone(),
                received: *received,
                sent: *sent,
                at: Instant::now(),
            }),
            Self::Disconnected => None,
        }
    }
}

fn wifi_icon(signal: u8) -> &'static str {
    match signal {
        0..=20 => "󰤯",
        21..=40 => "󰤟",
        41..=60 => "󰤢",
        61..=80 => "󰤥",
        _ => "󰤨",
    }
}

fn traffic(interface: &str, received: u64, sent: u64, previous: Option<&TrafficSample>) -> String {
    let Some(previous) = previous else {
        return "  —     —".into();
    };
    if previous.interface != interface {
        return "  —     —".into();
    }
    let seconds = previous.at.elapsed().as_secs_f64();
    let sent = sent.saturating_sub(previous.sent) as f64 / seconds;
    let received = received.saturating_sub(previous.received) as f64 / seconds;
    format!(
        "  {}/s     {}/s",
        format_bytes(sent),
        format_bytes(received)
    )
}

fn format_bytes(bytes: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_compare_traffic_across_interfaces() {
        let previous = TrafficSample {
            interface: "wlan0".into(),
            received: 1,
            sent: 1,
            at: Instant::now(),
        };
        assert_eq!(
            traffic("eth0", 10_000, 10_000, Some(&previous)),
            "  —     —"
        );
    }
}
