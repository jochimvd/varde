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
        mode: String,
        frequency: u32,
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
            Self::Disconnected => "󰲛",
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
                mode,
                frequency,
                address,
                received,
                sent,
                ..
            } => format!(
                "{network} · {} · {}\n󰩠  {address}\n{}",
                wifi_generation(mode),
                wifi_band(*frequency),
                traffic(interface, *received, *sent, previous)
            ),
            Self::Ethernet {
                interface,
                address,
                received,
                sent,
            } => format!(
                "Ethernet · {interface}\n󰩠  {address}\n{}",
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

fn wifi_generation(mode: &str) -> &str {
    match mode {
        "802.11be" => "Wi-Fi 7",
        "802.11ax" => "Wi-Fi 6",
        "802.11ac" => "Wi-Fi 5",
        "802.11n" => "Wi-Fi 4",
        _ => mode,
    }
}

fn wifi_band(frequency: u32) -> &'static str {
    match frequency {
        0..=3000 => "2.4G",
        3001..=5924 => "5G",
        _ => "6G",
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
        return "  —    —".into();
    };
    if previous.interface != interface {
        return "  —    —".into();
    }
    let seconds = previous.at.elapsed().as_secs_f64();
    let sent = sent.saturating_sub(previous.sent) as f64 / seconds;
    let received = received.saturating_sub(previous.received) as f64 / seconds;
    format_rates(received, sent)
}

fn format_rates(received: f64, sent: f64) -> String {
    format!(
        "  {:.2}    {:.2} Mbps",
        received * 8.0 / 1_000_000.0,
        sent * 8.0 / 1_000_000.0
    )
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
            "  —    —"
        );
    }

    #[test]
    fn names_wifi_generations() {
        assert_eq!(wifi_generation("802.11ax"), "Wi-Fi 6");
        assert_eq!(wifi_generation("802.11g"), "802.11g");
    }

    #[test]
    fn names_wifi_bands() {
        assert_eq!(wifi_band(2412), "2.4G");
        assert_eq!(wifi_band(5640), "5G");
        assert_eq!(wifi_band(5955), "6G");
    }

    #[test]
    fn formats_both_rates_with_one_unit() {
        assert_eq!(format_rates(125_000.0, 62_500.0), "  1.00    0.50 Mbps");
        assert_eq!(format_rates(20.0, 10.0), "  0.00    0.00 Mbps");
    }
}
