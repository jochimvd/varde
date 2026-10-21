# Varde

A small, personal desktop shell for Hyprland, written in Rust with GTK 4.

![Varde application launcher](docs/images/varde-launcher.png)

<details>
<summary>More screenshots</summary>

![Varde clipboard history with an image preview](docs/images/varde-clipboard.png)

![Varde calendar popover and notification popups](docs/images/varde-popups.png)

![Varde notification center with grouped, critical, progress, and media notifications](docs/images/varde-notifications.png)

</details>

Varde provides a status bar, application, action, and clipboard launchers,
notifications, workspace controls, system status, idle inhibition, privacy
indicators, and a StatusNotifier tray. The shell is configured directly in the source; launcher actions use TOML.

## Try it

Varde provides a bar and the `org.freedesktop.Notifications` service. Stop any
conflicting bar or notification daemon, then run it from a Hyprland session:

```sh
git clone https://github.com/jochimvd/varde.git
cd varde
cargo run --release -- start
```

## Requirements

Build dependencies: a current Rust toolchain, GTK 4.12 or newer, and
gtk4-layer-shell.

Runtime dependencies: Hyprland, PipeWire, WirePlumber, iwd, BlueZ, iproute2,
PulseAudio utilities, `uwsm-app`, and JetBrains Mono Nerd Font. Notification
sounds use `canberra-gtk-play` and `sound-theme-freedesktop`.

Clipboard history requires `cliphist`, `wl-copy`, and a running watcher:

```sh
wl-paste --watch cliphist store
```

Optional bar actions use `pavucontrol`, `impala`, `bluetui`, `btop`, and
`$TERMINAL`.

## Install

```sh
cargo install --path . --locked
```

This installs two programs: `varde-shell`, the GTK shell, and `varde`, a small
client that forwards commands to the running shell without loading GTK.
`varde start` runs `varde-shell`, as does any command when no shell is running.

<details>
<summary>Run Varde as a systemd user service</summary>

Create `~/.config/systemd/user/varde.service`:

```ini
[Unit]
Description=Varde desktop shell
PartOf=graphical-session.target
After=graphical-session.target
Requisite=graphical-session.target

[Service]
Type=exec
ExecStart=%h/.cargo/bin/varde start
Restart=on-failure
Slice=app-graphical.slice

[Install]
WantedBy=graphical-session.target
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now varde.service
```

</details>

## Use

```sh
varde launcher
varde actions
varde clipboard
varde notifications
varde notifications invoke
varde notifications actions
varde notifications dismiss
varde notifications clear
varde notifications dnd
```

`invoke`, `actions`, and `dismiss` act on the newest notification with a popup
on screen, else the newest one, so they suit keybindings.

Notifications and Do Not Disturb survive a restart: Varde saves them to
`~/.local/state/varde/notifications.json` when it stops normally and restores
them to the notification center on the next start. A crash loses them.

```sh
printf "Lock\nSuspend\nReboot\nShutdown" | varde dmenu -p "System..."
```

See `varde --help` for the complete CLI.

## Titlebar

Scripts can write to the bar's title with `varde title`. The most recently set
title is shown until it is cleared or expires; without one the bar shows the
active window title.

```sh
varde title set '{"version": 1, "id": "build", "icon": "󰔟", "text": "Building", "ttl_ms": 5000}'
varde title clear build
```

`set` reads the JSON from standard input when no argument is given. Optional
fields: `icon`, `ttl_ms` (0 keeps it until cleared), `items` (`key`, `label`,
`pinned`), and `items_delay_ms`, which shows the heading first and the items
after that delay. Items that do not fit collapse into `+N more`, except pinned
ones; hover for the full list. `--seq N` makes Varde ignore a set or clear that
arrives after a newer one for the same ID.

Callers can also activate the `title-set` or `title-clear` application action
over D-Bus directly; the parameter is `(seq, json-or-id)`, with seq 0 meaning
none:

```sh
busctl --user call org.varde.desktop /org/varde/desktop org.gtk.Actions Activate \
    'sava{sv}' title-set 1 '(ts)' 0 '{"version": 1, "id": "build", "text": "Building"}' 0
```

## Launcher actions

Type `>` in the application launcher or run `varde actions` to open the action
menu. Text after `>` searches action names and commands. Remove the prefix to
return to applications. Running `varde actions` again closes the action menu.

Define actions in `$XDG_CONFIG_HOME/varde/actions.toml`, defaulting to
`~/.config/varde/actions.toml`:

```toml
[[actions]]
name = "Restart audio"
command = 'systemctl --user restart pipewire'

[[actions]]
name = "Take screenshot"
command = '"$HOME/scripts/screenshot.sh"'

[[actions]]
name = "Save timestamp"
command = '''
mkdir -p "$HOME/notes"
date >> "$HOME/notes/timestamps.txt"
'''
```

The file is read whenever the action menu opens, including when switching into
it with `>`. A missing file gives an empty menu; invalid configuration shows an
error in the launcher.

Selecting an action starts `bash -c` with the configured command and closes the
menu. Commands inherit Varde's environment and working directory; use absolute
paths or `$HOME` for scripts. No interactive shell configuration is loaded and
no terminal is opened automatically. For an interactive command, launch your
terminal explicitly in `command`. Output and command failures go to Varde's
logs (the user journal when running as a service).

## Notification hook

To run a helper on incoming notifications, add
`~/.config/varde/notifications.toml` (or `$XDG_CONFIG_HOME/varde/notifications.toml`):

```toml
[hook]
argv = ["/path/to/notification-forwarder"]
timeout_ms = 3000
```

Arguments are literal, with no shell expansion. Each
invocation receives one JSON event on stdin, including app identity, title,
body, notification ID/revision, replacement flag, and DND state. See
[`Event`](src/notifications/hook.rs) for all fields.

## Customize

The bar layout is in `src/bar/mod.rs`, modules are under `src/bar/modules/`,
and appearance is defined in `src/style.css`.

```sh
cargo install --path . --locked
systemctl --user restart varde.service
```

## Development

See [docs/development.md](docs/development.md).
