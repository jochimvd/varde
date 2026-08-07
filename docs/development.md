# Development

The shell is tested in a nested Hyprland compositor, not in the live desktop
session.

## Start a session

Run this from a terminal in the live Wayland session:

```sh
scripts/dev-session
```

The script builds `target/debug/varde`, launches a nested Hyprland window with
`dev/hyprland.lua`, and starts the built binary inside it. Close the window or
press Ctrl+Alt+Escape inside it to stop the session.

Use `scripts/dev-session --release` to build and run the optimized binary for
resource measurements. Add `--cairo` to run GTK without its Vulkan renderer.

Inside the nested session, Ctrl+Space toggles the app launcher, Ctrl+V toggles
clipboard history, Ctrl+Q closes the active window, and Ctrl+1 through Ctrl+9
switch workspaces.

The nested compositor uses its own config, instance signature, Wayland socket,
and output. `HYPRLAND_NO_SD_VARS=1` prevents it from exporting its environment
to the live user session. The live compositor hosts a temporary 1280x720
window on a dedicated `varde-dev` workspace. It is created silently without
focus or animation, so starting it does not switch workspaces, steal focus, or
retile the current workspace. Show it only when wanted:

```sh
scripts/dev-show --focus
```

The test shell and inspection commands target the nested socket.
Session metadata is kept in
`$XDG_RUNTIME_DIR/varde-dev/default/session` and removed on exit.

## Concurrent agent sessions

Use a separate Git worktree and a unique session ID for each agent. Pass that
ID to every development command:

```sh
scripts/dev-session --session agent-1
scripts/dev-inspect --session agent-1
scripts/dev-screenshot --session agent-1
scripts/dev-show --session agent-1 --focus
```

Session IDs start with a lowercase letter, contain only lowercase letters,
digits, and hyphens, and are at most 32 characters long. Omitting `--session`
uses `default`, which is reserved for manual development. Automated agents must
always pass their unique session ID explicitly to every development command.

Each session has its own runtime metadata and lock, Wayland socket,
Hyprland instance, `varde-dev-<session>` host workspace, GTK application ID,
working directory at `$XDG_RUNTIME_DIR/varde-dev/<session>/work`, and default
screenshot at `target/dev/<session>/screenshot.png`. The compositor, shell, and
applications launched from it inherit that working directory, preventing
relative application paths from writing into the Git worktree. Two processes
cannot use the same session ID concurrently.

The nested sessions share the live user D-Bus and host services so the tray,
idle inhibition, audio, network, and Bluetooth remain available. Notifications
use a private D-Bus session owned by the development session. Interacting with
the shared modules can therefore affect the live desktop.

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

The default session image is `target/dev/default/screenshot.png`. `grim` is
explicitly granted the nested session's `screencopy` permission.

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
