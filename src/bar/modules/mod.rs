pub mod connectivity;
pub mod hyprland;
pub mod services;
pub mod system;
pub mod tray;

use crate::background;

/// Opens `command` in a floating window; Hyprland's exec expands variables
/// such as `$TERMINAL`.
fn open_floating(command: &str) {
    if let Err(error) = background::run(
        &["hyprctl", "dispatch", &floating_dispatch(command)],
        |_| {},
    ) {
        eprintln!("varde: could not open {command:?}: {error}");
    }
}

fn floating_dispatch(command: &str) -> String {
    format!(
        r#"hl.dsp.exec_cmd({}, {{ tag = "+floating-window" }})"#,
        lua_string(command)
    )
}

fn lua_string(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_floating_commands_to_hyprland() {
        assert_eq!(
            floating_dispatch("$TERMINAL -e btop"),
            r#"hl.dsp.exec_cmd('$TERMINAL -e btop', { tag = "+floating-window" })"#
        );
        assert_eq!(
            floating_dispatch("sh -c 'echo \"a\\b\"'\nx"),
            r#"hl.dsp.exec_cmd('sh -c \'echo "a\\b"\'\nx', { tag = "+floating-window" })"#
        );
    }
}
