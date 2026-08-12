# Development

Varde is tested in a nested Hyprland compositor so shell changes do not replace
the live desktop.

Every public `scripts/dev-*` command supports `-h` and `--help` for its options,
accepted formats, and defaults.

## Start a session

From a terminal in the live Hyprland session, run:

```sh
scripts/dev-session
```

This builds Varde, opens a 1280x720 nested compositor on a dedicated host
workspace, and runs the shell inside it. Close the window or press
Ctrl+Alt+Escape inside it to stop the session.

Use `--release` for an optimized build or `--cairo` to run GTK without its
Vulkan renderer:

```sh
scripts/dev-session --release
scripts/dev-session --cairo
```

The session starts without changing workspace or taking focus. Show it when
needed:

```sh
scripts/dev-show --focus
```

Inside the nested compositor, Ctrl+Space toggles the launcher, Ctrl+V toggles
clipboard history, Ctrl+Q closes the active window, and Ctrl+1 through Ctrl+9
switch workspaces.

### Named sessions

The default session is reserved for manual development. Automated agents use a
separate worktree and pass the same unique session ID to every dev command:

```sh
scripts/dev-session --session agent-1
scripts/dev-restart --session agent-1
scripts/dev-notify --session agent-1 'Summary' 'Body'
scripts/dev-pointer --session agent-1 click 640 360
scripts/dev-inspect --session agent-1
scripts/dev-screenshot --session agent-1
scripts/dev-measure --session agent-1 color 640 360
scripts/dev-show --session agent-1 --focus
```

Session IDs start with a lowercase letter, contain lowercase letters, digits,
or hyphens, and are at most 32 characters long. Each session has its own lock,
compositor, Wayland socket, notification bus, GTK application ID, host
workspace, working directory, and screenshot path. Runtime metadata lives at
`$XDG_RUNTIME_DIR/varde-dev/<session>/session` and is removed on exit.

The nested session shares live tray, audio, network, Bluetooth, and idle
services. Interacting with those modules can therefore affect the live desktop.

## Update the shell

Code and CSS are compiled into the binary. Rebuild and replace only Varde while
leaving the nested compositor and its windows running:

```sh
scripts/dev-restart
scripts/dev-restart --no-build
```

Restarting Varde clears shell-owned state, including stored notifications.

## Drive the session

### Notifications

Send notifications to the session's private bus:

```sh
scripts/dev-notify 'Summary' 'Body'
scripts/dev-notify --app Chat --actions "['default','Open','reply','Reply']" 'Summary' 'Body'
scripts/dev-notify --hints "{'urgency': <byte 2>}" 'Critical'
scripts/dev-notify --timeout 2500 'Timed popup'
scripts/dev-notify --close 3
scripts/dev-notify --clear
```

`scripts/dev-notification-tests --session ID` runs the interactive protocol
walkthrough one step at a time. Use `dev-notify` for quick setup.

### Pointer input

Move and click using absolute nested-output coordinates:

```sh
scripts/dev-pointer move 640 360
scripts/dev-pointer click 1228 123
scripts/dev-pointer click 900 200 --button right
```

`dev-pointer` uses `wlrctl` to produce ordinary framed pointer input. A click at
the current position sends no preceding motion, which reproduces interactions
with content that moved under a stationary pointer. Hyprland's cursor and key
dispatchers do not provide equivalent pointer event framing.

For compositor operations not covered by a helper, load the session metadata
and dispatch Lua directly:

```sh
source "$XDG_RUNTIME_DIR/varde-dev/<session>/session"
hyprctl -i "$VARDE_DEV_INSTANCE" eval 'hl.dispatch(hl.dsp.<action>(...))'
```

## Inspect the result

Inspect monitors, layer surfaces, and windows as JSON:

```sh
scripts/dev-inspect
```

Capture the nested output, optionally including the pointer, cropping, or
zooming without smoothing:

```sh
scripts/dev-screenshot
scripts/dev-screenshot target/dev/another-name.png
scripts/dev-screenshot --no-pointer
scripts/dev-screenshot --crop '1180,106 76x38' --zoom 6
```

The default image is `target/dev/<session>/screenshot.png`. The nested config
explicitly grants `grim` screencopy permission.

Measure a pixel, locate a color, or compare captures:

```sh
scripts/dev-measure color 1228 123
scripts/dev-measure bbox '#3b3c3e' --region '600,40 680x680'
scripts/dev-measure bbox '#9ece6a' --tolerance 8
scripts/dev-measure --session agent-1 diff before.png after.png
```

`color` prints `#RRGGBB`; `bbox` and `diff` print `WxH+X+Y`. An identical diff
prints `identical`.

## Requirements and checks

Beyond the project dependencies in [README.md](../README.md), pointer input
needs `wlrctl`, measurements need Pillow and optionally `grim`, and screenshot
zooming needs ImageMagick.

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
Hyprland --verify-config --config dev/hyprland.lua
```

Useful Hyprland references:

- [nested compositor development](https://wiki.hypr.land/Contributing-and-Debugging/)
- [Lua dispatchers](https://wiki.hypr.land/Configuring/Basics/Dispatchers/)
- [instance inspection](https://wiki.hypr.land/Configuring/Advanced-and-Cool/Using-hyprctl/)
- [screencopy permissions](https://wiki.hypr.land/Configuring/Advanced-and-Cool/Permissions/)
