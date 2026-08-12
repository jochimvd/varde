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
scripts/dev-restart --session agent-1
scripts/dev-notify --session agent-1 'Summary' 'Body'
scripts/dev-pointer --session agent-1 click 640 360
scripts/dev-screenshot --session agent-1
scripts/dev-measure --session agent-1 color 640 360
scripts/dev-inspect --session agent-1
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
scripts/dev-screenshot --no-pointer
scripts/dev-screenshot --crop '1180,106 76x38' --zoom 6
```

The default session image is `target/dev/default/screenshot.png`. `grim` is
explicitly granted the nested session's `screencopy` permission. Captures
include the pointer, which is how a cursor shape can be checked; `--no-pointer`
leaves it out. `--crop` takes a `X,Y WxH` region of the output and `--zoom`
scales the result without smoothing, which keeps single pixels legible.

Measure what was drawn instead of eyeballing it. `color` and `bbox` capture the
session unless `--file` names an image; `bbox` reports where a colour appears,
which is how a panel or a widget is located, and `--region` keeps the search
away from anything else painted in the same colour:

```sh
scripts/dev-measure color 1228 123
scripts/dev-measure bbox '#3b3c3e' --region '600,40 680x680'
scripts/dev-measure bbox '#9ece6a' --tolerance 8
scripts/dev-measure diff before.png after.png
```

`color` prints `#RRGGBB`, `bbox` and `diff` print `WxH+X+Y`, and `diff` prints
`identical` when two captures match, which answers whether a click did
anything.

A widget that paints no background of its own still measures if it is given a
temporary colour in `src/style.css` and reverted afterwards: `bbox` then
reports its box, and the insets around whatever it contains settle questions
about alignment and padding that are otherwise guessed at.

## Drive a session

Code and the stylesheet are both compiled into the binary, so an edit reaches a
session only through a rebuild and a restart of the shell. `dev-restart` does
both in well under a second and leaves the compositor, its workspace and its
window alone, which restarting the session does not; state held in the shell,
such as stored notifications, is lost either way:

```sh
scripts/dev-restart
scripts/dev-restart --no-build
```

Send, close and clear notifications on the session's private bus:

```sh
scripts/dev-notify 'Summary' 'Body'
scripts/dev-notify --app Chat --actions "['default','Open','reply','Reply']" 'Summary' 'Body'
scripts/dev-notify --hints "{'urgency': <byte 2>}" 'Critical'
scripts/dev-notify --close 3
scripts/dev-notify --clear
```

`scripts/dev-notification-tests` remains the interactive walkthrough of the
notification protocol. It waits for a keypress before each step, so reach for
`dev-notify` when a session just needs filling.

Move the pointer and click:

```sh
scripts/dev-pointer move 640 360
scripts/dev-pointer click 1228 123
scripts/dev-pointer click 900 200 --button right
```

Warping the cursor does not deliver anything to the client, which learns the
pointer position only from a button event and acts on that event before taking
the new position in. `move` is therefore a warp and nothing else: the shell
does not notice it, so hovering does not light up until something is clicked.
`click` sends two presses for the same reason, the first being spent on the
stale position, and lands exactly one click on the target. A click is still
swallowed now and then, so capture the output to see what happened rather than
assuming, and click again if nothing did. None of this happens with real input,
so a click that misbehaves here is the harness rather than the shell.

Anything the scripts do not cover can be dispatched to the nested compositor
directly. Load the session's metadata, then evaluate Lua against its instance:

```sh
source "$XDG_RUNTIME_DIR/varde-dev/<session-id>/session"
hyprctl -i "$VARDE_DEV_INSTANCE" eval 'hl.dispatch(hl.dsp.<action>(...))'
```

The [Hyprland dispatchers](https://wiki.hypr.land/Configuring/Basics/Dispatchers/)
list the available actions.

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
