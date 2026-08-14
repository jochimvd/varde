use serde::Deserialize;

use super::super::command::{command, property, strip_ansi};
use super::state::State;

pub(super) fn state() -> State {
    let Some(route) = command("ip", &["-j", "route", "show", "default"])
        .and_then(|output| parse_default_route(&output))
    else {
        return State::Disconnected;
    };
    let Some(interface) = command("ip", &["-j", "-s", "link", "show", "dev", &route.device])
        .and_then(|output| parse_interface(&output))
    else {
        return State::Disconnected;
    };
    let address = command("ip", &["-j", "address", "show", "dev", &route.device])
        .and_then(|output| parse_address(&output))
        .unwrap_or_else(|| route.address.unwrap_or_default());

    if let Some(station) = connected_station(&route.device) {
        return State::Wifi {
            interface: route.device,
            network: station.network,
            mode: station.mode,
            frequency: station.frequency,
            signal: station.signal,
            address,
            received: interface.received,
            sent: interface.sent,
        };
    }

    State::Ethernet {
        interface: route.device,
        address,
        received: interface.received,
        sent: interface.sent,
    }
}

fn connected_station(interface: &str) -> Option<WifiStation> {
    let stations = command("iwctl", &["station", "list"])?;
    if !parse_iwctl_stations(&stations)
        .iter()
        .any(|station| station == interface)
    {
        return None;
    }
    command("iwctl", &["station", interface, "show"]).and_then(|output| parse_wifi_station(&output))
}

struct WifiStation {
    network: String,
    mode: String,
    frequency: u32,
    signal: u8,
}

fn parse_iwctl_stations(text: &str) -> Vec<String> {
    strip_ansi(text)
        .lines()
        .filter_map(|line| {
            let columns = line.split_whitespace().collect::<Vec<_>>();
            (columns.len() >= 2 && columns[1] == "connected").then(|| columns[0].to_string())
        })
        .collect()
}

fn parse_wifi_station(text: &str) -> Option<WifiStation> {
    let text = strip_ansi(text);
    let network = property(&text, "Connected network")?;
    let rssi: i32 = property(&text, "AverageRSSI")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let frequency = property(&text, "Frequency")?.parse().ok()?;
    Some(WifiStation {
        network,
        mode: property(&text, "RxMode")?,
        frequency,
        signal: rssi_to_percent(rssi),
    })
}

#[derive(Deserialize)]
struct Route {
    dev: String,
    prefsrc: Option<String>,
}

struct DefaultRoute {
    device: String,
    address: Option<String>,
}

fn parse_default_route(text: &str) -> Option<DefaultRoute> {
    serde_json::from_str::<Vec<Route>>(text)
        .ok()?
        .into_iter()
        .next()
        .map(|route| DefaultRoute {
            device: route.dev,
            address: route.prefsrc,
        })
}

#[derive(Deserialize)]
struct Link {
    stats64: LinkStats,
}

#[derive(Deserialize)]
struct LinkStats {
    rx: ByteCount,
    tx: ByteCount,
}

#[derive(Deserialize)]
struct ByteCount {
    bytes: u64,
}

struct InterfaceStats {
    received: u64,
    sent: u64,
}

fn parse_interface(text: &str) -> Option<InterfaceStats> {
    let interface = serde_json::from_str::<Vec<Link>>(text)
        .ok()?
        .into_iter()
        .next()?;
    Some(InterfaceStats {
        received: interface.stats64.rx.bytes,
        sent: interface.stats64.tx.bytes,
    })
}

#[derive(Deserialize)]
struct Address {
    addr_info: Vec<AddressInfo>,
}

#[derive(Deserialize)]
struct AddressInfo {
    family: String,
    local: String,
    prefixlen: u8,
    scope: String,
}

fn parse_address(text: &str) -> Option<String> {
    serde_json::from_str::<Vec<Address>>(text)
        .ok()?
        .into_iter()
        .next()?
        .addr_info
        .into_iter()
        .find(|address| address.family == "inet" && address.scope == "global")
        .map(|address| format!("{}/{}", address.local, address.prefixlen))
}

fn rssi_to_percent(rssi: i32) -> u8 {
    (2 * (rssi + 100)).clamp(0, 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iwd_station_details() {
        let stations = "  Name                  State\n  wlan0                 connected\n";
        assert_eq!(parse_iwctl_stations(stations), vec!["wlan0"]);

        let station = "Connected network     Home\nFrequency             5640\nAverageRSSI           -62 dBm\nRxMode                802.11ax\n";
        let station = parse_wifi_station(station).unwrap();
        assert_eq!(station.network, "Home");
        assert_eq!(station.mode, "802.11ax");
        assert_eq!(station.frequency, 5640);
        assert_eq!(station.signal, 76);
    }

    #[test]
    fn parses_ip_json() {
        assert_eq!(
            parse_default_route(r#"[{"dev":"wlan0","prefsrc":"192.168.1.4"}]"#)
                .map(|route| (route.device, route.address)),
            Some(("wlan0".into(), Some("192.168.1.4".into())))
        );
        assert_eq!(
            parse_address(
                r#"[{"addr_info":[{"family":"inet","local":"192.168.1.4","prefixlen":24,"scope":"global"}]}]"#
            ),
            Some("192.168.1.4/24".into())
        );
    }
}
