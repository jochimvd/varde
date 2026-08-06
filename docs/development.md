# Development

The shell is tested in a nested Hyprland compositor, not in the live desktop
session.

## Start a session

Run this from a terminal in the live Wayland session:

```sh
scripts/dev-session
```

The script builds `target/debug/shell`, launches a nested Hyprland window with
`dev/hyprland.lua`, and starts the built binary inside it. Close the window or
press Ctrl+Alt+Escape inside it to stop the session.

Use `scripts/dev-session --release` to build and run the optimized binary for
resource measurements. Add `--cairo` to run GTK without its Vulkan renderer.

Inside the nested session, Ctrl+Q closes the active window and Ctrl+1 through
Ctrl+9 switch workspaces.

The nested compositor uses its own config, instance signature, Wayland socket,
and output. `HYPRLAND_NO_SD_VARS=1` prevents it from exporting its environment
to the live user session. The live compositor hosts a temporary 1280x720
window on a dedicated `shell-dev` workspace. It is created silently without
focus or animation, so starting it does not switch workspaces, steal focus, or
retile the current workspace. Show it only when wanted:

```sh
scripts/dev-show --focus
```

The test shell and inspection commands target the nested socket.
Session metadata is kept in
`$XDG_RUNTIME_DIR/shell-dev/session` and removed on exit.

## Inspect and capture

With the nested compositor running, inspect its monitor, layer-surface, and
window state:

```sh
scripts/dev-inspect
```

Capture only its virtual output:

```sh
scripts/dev-screenshot
scripts/dev-screenshot target/dev/another-name.png
```

The default image is `target/dev/screenshot.png`. `grim` is explicitly granted
the nested session's `screencopy` permission.

## Checks

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
Hyprland --verify-config --config dev/hyprland.lua
```

Hyprland's documentation covers the mechanisms used here:

- [nested compositor development](https://wiki.hypr.land/Contributing-and-Debugging/)
- [selecting a config with `--config`](https://wiki.hypr.land/Configuring/Start/)
- [Lua autostart](https://wiki.hypr.land/Configuring/Basics/Autostart/)
- [`hyprctl` instances and JSON inspection](https://wiki.hypr.land/Configuring/Advanced-and-Cool/Using-hyprctl/)
- [screenshots and recording](https://wiki.hypr.land/Useful-Utilities/Screenshots-and-Recording/)
- [screencopy permissions](https://wiki.hypr.land/Configuring/Advanced-and-Cool/Permissions/)
