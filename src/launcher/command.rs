use std::io::Read;

use gio::prelude::*;

pub(super) const USAGE: &str = "usage: varde [launcher [apps | clipboard | --dmenu [-p PROMPT]]]";

pub(super) enum Request {
    Activate,
    Launcher,
    Clipboard,
    Dmenu { prompt: String },
}

pub(super) fn parse(arguments: &[std::ffi::OsString]) -> Result<Request, String> {
    let arguments = arguments
        .iter()
        .skip(1)
        .map(|argument| {
            argument
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| "Arguments must be UTF-8".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    match arguments.as_slice() {
        [] => Ok(Request::Activate),
        [launcher] if launcher == "launcher" => Ok(Request::Launcher),
        [launcher, apps] if launcher == "launcher" && apps == "apps" => Ok(Request::Launcher),
        [launcher, clipboard] if launcher == "launcher" && clipboard == "clipboard" => {
            Ok(Request::Clipboard)
        }
        [launcher, dmenu, options @ ..] if launcher == "launcher" && dmenu == "--dmenu" => {
            let mut prompt = "Search".to_string();
            let mut index = 0;
            while index < options.len() {
                match options[index].as_str() {
                    "-p" | "--prompt" => {
                        prompt = options
                            .get(index + 1)
                            .ok_or_else(|| format!("{} needs a value", options[index]))?
                            .clone();
                        index += 2;
                    }
                    option => return Err(format!("Unknown option: {option}")),
                }
            }
            Ok(Request::Dmenu { prompt })
        }
        _ => Err("Unknown command".into()),
    }
}

pub(super) fn read_lines(
    command_line: &gio::ApplicationCommandLine,
) -> Result<Vec<String>, String> {
    let input = command_line
        .stdin()
        .ok_or_else(|| "Selector input is unavailable".to_string())?;
    let mut bytes = Vec::new();
    input
        .into_read()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read selector input: {error}"))?;
    parse_lines(bytes)
}

fn parse_lines(bytes: Vec<u8>) -> Result<Vec<String>, String> {
    let input = String::from_utf8(bytes).map_err(|_| "Selector input must be UTF-8".to_string())?;
    Ok(input.lines().map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<std::ffi::OsString> {
        std::iter::once("varde")
            .chain(values.iter().copied())
            .map(Into::into)
            .collect()
    }

    #[test]
    fn parses_launcher_commands() {
        assert!(matches!(parse(&args(&[])), Ok(Request::Activate)));
        assert!(matches!(parse(&args(&["launcher"])), Ok(Request::Launcher)));
        assert!(matches!(
            parse(&args(&["launcher", "apps"])),
            Ok(Request::Launcher)
        ));
        assert!(matches!(
            parse(&args(&["launcher", "clipboard"])),
            Ok(Request::Clipboard)
        ));
    }

    #[test]
    fn parses_dmenu_prompt() {
        let command =
            parse(&args(&["launcher", "--dmenu", "-p", "System..."])).expect("valid command");
        assert!(matches!(command, Request::Dmenu { prompt } if prompt == "System..."));
    }

    #[test]
    fn rejects_unknown_arguments() {
        assert!(parse(&args(&["launcher", "--width", "300"])).is_err());
        assert!(parse(&args(&["launcher", "--dmenu", "--bad"])).is_err());
    }

    #[test]
    fn preserves_duplicate_and_empty_selector_lines() {
        assert_eq!(
            parse_lines(b"One\n\nOne\n".to_vec()).unwrap(),
            ["One", "", "One"]
        );
        assert!(parse_lines(Vec::new()).unwrap().is_empty());
        assert!(parse_lines(vec![0xff]).is_err());
    }
}
