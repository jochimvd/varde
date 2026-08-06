# Project guidelines

This project is a custom desktop shell for Hyprland on Arch Linux, written in Rust. It starts as a status bar and may later grow an app launcher, settings panel, and other shell components.

- Keep the implementation small, direct, and easy to understand.
- Prefer simple code over abstractions, frameworks, and speculative flexibility.
- Minimize dependencies and features; add only what the current task needs.
- Target the current system only. Backwards compatibility is not a goal.
- When requirements change, rewrite toward the cleanest design instead of preserving old APIs.
- Keep modules focused and avoid premature generalization.
- Use minimal comments. Do not restate what the code makes clear; document why only when the reasoning is obscure enough to warrant it.
- Format with `cargo fmt`; keep `cargo clippy` clean; add focused tests where they provide real value.
- Automated agents must use a separate Git worktree and choose a unique
  development session ID, such as their agent or task name. Always pass that ID
  with `--session ID` to `scripts/dev-session`, `scripts/dev-show`,
  `scripts/dev-inspect`, and `scripts/dev-screenshot`. Never use the commands'
  implicit `default` session; it is reserved for manual development.
