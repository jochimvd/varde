# Varde

A small, personal desktop shell for Hyprland, written in Rust with GTK 4.

![Varde application launcher](docs/images/varde-launcher.png)

![Varde notification center](docs/images/varde-notifications.png)

Varde currently provides a status bar, application and clipboard launchers,
notifications, workspace controls, system status, idle inhibition, privacy
indicators, and a StatusNotifier tray. Its layout and behavior are configured
directly in the source.

## Usage

```sh
cargo run
```

The launcher and notification center are exposed as application actions:

```sh
gapplication action org.varde.desktop launcher
gapplication action org.varde.desktop notifications
```

The launcher fuzzy-searches installed applications. It can also search
clipboard history or act as a dmenu-style selector:

```sh
gapplication action org.varde.desktop clipboard
varde launcher clipboard
printf "Lock\nSuspend\nReboot\nShutdown" | varde launcher --dmenu -p "System..."
```

Notifications include floating popups and a persistent notification center.
Non-critical popups hide after five seconds; critical popups have no automatic
timeout. Opening the notification center consumes visible popups. Notifications
remain live in the center until they are actioned, dismissed, or recalled by
their sender. The application-provided expiration timeout is ignored.

## Requirements

Varde targets its author's current Arch Linux system. The setup uses GTK 4,
gtk4-layer-shell, Hyprland, PipeWire, WirePlumber, iwd, BlueZ, iproute2,
PulseAudio utilities, libcanberra, `sound-theme-freedesktop`, coreutils,
`uwsm-app`, `jq`, `grim`, and JetBrains Mono Nerd Font. Clipboard history
additionally needs `cliphist`, `wl-copy`, and a running
`wl-paste --watch cliphist store` process.

Bar actions call `hyprctl`, `wpctl`, `pactl`, `ip`, `iwctl`, `bluetoothctl`,
`pw-dump`, `dot-menu-audio-switcher`, `pavucontrol`, `impala`, `bluetui`,
`btop`, and the terminal configured by `$TERMINAL`.

For the isolated nested-Hyprland development workflow and project checks, see
[docs/development.md](docs/development.md).
