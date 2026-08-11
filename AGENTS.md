# Project guidelines

This project is a custom desktop shell for Hyprland on Arch Linux, written in Rust.

- Keep the implementation small, direct, and easy to understand.
- Prefer simple code over abstractions, frameworks, and speculative flexibility.
- Minimize dependencies and features; add only what the current task needs.
- Target the current system only. Backwards compatibility is not a goal.
- When requirements change, rewrite toward the cleanest design instead of preserving old APIs.
- Keep modules focused and avoid premature generalization.
- Use minimal comments. Do not restate what the code makes clear; document why only when the reasoning is obscure enough to warrant it.
- Format with `cargo fmt`; keep `cargo clippy` clean; add focused tests where they provide real value.
- Automated agents must use a separate Git worktree and choose a unique
  development session ID, such as their agent or task name. Create worktrees as
  siblings of the main checkout, named `varde-wt-<session-id>`, on a branch
  named `<session-id>`. Choose a short, descriptive ID that says what the work
  does, and use that same ID with `--session ID` for
  `scripts/dev-session`, `scripts/dev-show`, `scripts/dev-inspect`, and
  `scripts/dev-screenshot`. Never use the commands' implicit `default` session;
  it is reserved for manual development.
- To control a running nested session, load its metadata and dispatch Lua to its
  compositor: `source "$XDG_RUNTIME_DIR/varde-dev/<session-id>/session"`, then
  `hyprctl -i "$VARDE_DEV_INSTANCE" eval 'hl.dispatch(hl.dsp.<action>(...))'`.
  Move/hover with `hl.dispatch(hl.dsp.cursor.move({ x = 640, y = 360 }))`.
  Left-click with `hl.dispatch(hl.dsp.send_key_state({ mods = "", key =
  "mouse:272", state = "down" }))`, then the same action with `state = "up"`.
  See the [Hyprland dispatchers](https://wiki.hypr.land/Configuring/Basics/Dispatchers/)
  for other actions.
