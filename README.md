# Shell

A custom desktop shell for Hyprland, written in Rust.

The status bar currently provides Hyprland workspaces and window titles,
date and time, Bluetooth, network and audio state, CPU and memory usage,
notifications, idle inhibition, privacy indicators, and a StatusNotifier tray.
Its layout and behavior are defined directly in the Rust source.

Mako remains responsible for notification popups. The bell opens a compact
notification center backed by Mako's active list and history. It can also be
opened as an application action:

```sh
gapplication action be.jochim.shell notifications
```

The application launcher is exposed as a GTK application action, so the shell
continues to run as one process:

```sh
gapplication action be.jochim.shell launcher
```

It lists visible desktop applications and supports fuzzy searching. The same
window can also select newline-delimited input for shell scripts:

```sh
printf "Lock\nSuspend\nReboot\nShutdown\nLog Out" |
  shell launcher --dmenu -p "System..."
```

Clipboard history is available through a second action or the launcher CLI:

```sh
gapplication action be.jochim.shell clipboard
shell launcher clipboard
```

It reads the existing cliphist history, fuzzy-searches text and image metadata,
loads visible image thumbnails in the background, and restores the selected
entry to the Wayland clipboard.

The current system requires GTK 4, gtk4-layer-shell, Hyprland, Mako, PipeWire,
WirePlumber, iwd, BlueZ, iproute2, PulseAudio utilities, coreutils, `jq`, `grim`,
and JetBrains Mono Nerd Font. Clipboard history requires `cliphist`, `wl-copy`,
and an existing `wl-paste --watch cliphist store` process. The bar's actions
use `hyprctl`, `wpctl`, `pactl`, `ip`, `iwctl`, `bluetoothctl`, `pw-dump`,
`makoctl`, `dot-menu-audio-switcher`, `pavucontrol`, `impala`, `bluetui`,
`btop`, and the terminal named by `$TERMINAL`.

## Development workflow

Development happens in a nested Hyprland compositor rather than the live
desktop session.

Start the development session from a terminal in Hyprland:

```sh
scripts/dev-session
```

This builds `target/debug/shell`, starts a nested Hyprland instance with
`dev/hyprland.lua`, and launches the binary inside it. The 1280x720 host window
is created silently on a dedicated `shell-dev` workspace, so starting the
session does not switch workspaces, steal focus, or retile the current
workspace.

The launcher stays running for the lifetime of the session. Press Ctrl+C in
that terminal to stop it. After showing the nested session, it can also be
stopped with Ctrl+Alt+Escape inside its window.

Show the nested compositor when wanted:

```sh
scripts/dev-show --focus
```

Inspect its monitor, layer-surface, and window state without showing it:

```sh
scripts/dev-inspect
```

Capture only the nested output:

```sh
scripts/dev-screenshot
scripts/dev-screenshot target/dev/another-name.png
```

The default session screenshot path is `target/dev/default/screenshot.png`.

Multiple sessions can run at once. Give each agent a separate Git worktree and
use the same unique session ID with every development command:

```sh
scripts/dev-session --session agent-1
scripts/dev-inspect --session agent-1
scripts/dev-screenshot --session agent-1
scripts/dev-show --session agent-1 --focus
```

Named sessions keep their metadata, Wayland socket, host workspace, GTK
application ID, working directory, and default screenshot separate. Their
working directory is `$XDG_RUNTIME_DIR/shell-dev/<session>/work`, so
applications with relative paths cannot write into the Git worktree. Their
screenshots default to `target/dev/<session>/screenshot.png`. Omitting
`--session` selects the single-instance `default` session, which is reserved
for manual development. Automated agents must always pass their unique session
ID explicitly.

The nested compositor has its own config, Hyprland instance signature,
Wayland socket, and output. Inspection and screenshots target those nested
identifiers. `HYPRLAND_NO_SD_VARS=1` prevents the test compositor from
exporting its environment into the live user session. It also sets
`SHELL_DEVELOPMENT=1` to show the module alignment guides; normal launches do
not show them. The sessions intentionally retain access to the live user D-Bus
and system services, so notification, tray, idle, audio, network, and Bluetooth
interactions can affect the live desktop.

Run the project checks with:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
Hyprland --verify-config --config dev/hyprland.lua
```

More implementation details and relevant Hyprland documentation are collected
in [docs/development.md](docs/development.md).
