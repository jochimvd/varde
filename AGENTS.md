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
  does, and use that same ID with `--session ID` for every `scripts/dev-*`
  command. Never use the commands' implicit `default` session; it is reserved
  for manual development.
- `docs/development.md` covers the nested development session: starting one,
  restarting the shell after an edit, driving it with notifications and the
  pointer, and measuring what it drew. Read it before driving a session by
  hand.
