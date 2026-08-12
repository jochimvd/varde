use std::io::Read;

use gio::prelude::*;

pub const HELP: &str = "\
Varde desktop shell

Usage: varde <COMMAND>

Commands:
  start          Start the desktop shell
  launcher       Toggle the application launcher
  clipboard      Toggle the clipboard launcher
  notifications  Manage notifications
  dmenu          Select a line read from standard input
  help           Print help for a command

Options:
  -h, --help     Print help
  -V, --version  Print version";

const START_HELP: &str = "Start the Varde desktop shell

Usage: varde start";
const LAUNCHER_HELP: &str = "Toggle the application launcher

Usage: varde launcher";
const CLIPBOARD_HELP: &str = "Toggle the clipboard launcher

Usage: varde clipboard";
const NOTIFICATIONS_HELP: &str = "Manage notifications

Usage: varde notifications [clear]";
const DMENU_HELP: &str = "Select a line read from standard input

Usage: varde dmenu [OPTIONS]

Options:
  -p, --prompt <PROMPT>  Set the prompt [default: Search]
  -h, --help             Print help";

pub enum Request {
    Help(&'static str),
    Version,
    Start,
    Launcher,
    Clipboard,
    Notifications,
    ClearNotifications,
    Dmenu { prompt: String },
}

pub fn parse(arguments: &[std::ffi::OsString]) -> Result<Request, String> {
    let arguments = arguments
        .iter()
        .skip(1)
        .map(|argument| {
            argument
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| "arguments must be UTF-8".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    match arguments.as_slice() {
        [] => Ok(Request::Help(HELP)),
        [option] if option == "help" || option == "-h" || option == "--help" => {
            Ok(Request::Help(HELP))
        }
        [option] if option == "-V" || option == "--version" => Ok(Request::Version),
        [command] if command == "start" => Ok(Request::Start),
        [command] if command == "launcher" => Ok(Request::Launcher),
        [command] if command == "clipboard" => Ok(Request::Clipboard),
        [command] if command == "notifications" => Ok(Request::Notifications),
        [command, action] if command == "notifications" && action == "clear" => {
            Ok(Request::ClearNotifications)
        }
        [help_command, command] if help_command == "help" => help(command),
        [command, option] if option == "-h" || option == "--help" => help(command),
        [command, options @ ..] if command == "dmenu" => parse_dmenu(options),
        [command, argument, ..]
            if matches!(
                command.as_str(),
                "start" | "launcher" | "clipboard" | "notifications"
            ) =>
        {
            Err(format!("unexpected argument '{argument}'"))
        }
        [option, ..] if option.starts_with('-') => Err(format!("unknown option '{option}'")),
        [command, ..] => Err(format!("unknown command '{command}'")),
    }
}

fn help(command: &str) -> Result<Request, String> {
    match command {
        "start" => Ok(Request::Help(START_HELP)),
        "launcher" => Ok(Request::Help(LAUNCHER_HELP)),
        "clipboard" => Ok(Request::Help(CLIPBOARD_HELP)),
        "notifications" => Ok(Request::Help(NOTIFICATIONS_HELP)),
        "dmenu" => Ok(Request::Help(DMENU_HELP)),
        _ => Err(format!("unknown command '{command}'")),
    }
}

fn parse_dmenu(options: &[String]) -> Result<Request, String> {
    let mut prompt = "Search".to_string();
    let mut index = 0;
    while index < options.len() {
        match options[index].as_str() {
            "-h" | "--help" => return Ok(Request::Help(DMENU_HELP)),
            "-p" | "--prompt" => {
                prompt = options
                    .get(index + 1)
                    .ok_or_else(|| format!("{} requires a value", options[index]))?
                    .clone();
                index += 2;
            }
            option => return Err(format!("unknown option '{option}'")),
        }
    }
    Ok(Request::Dmenu { prompt })
}

pub fn read_lines(command_line: &gio::ApplicationCommandLine) -> Result<Vec<String>, String> {
    let input = command_line
        .stdin()
        .ok_or_else(|| "selector input is unavailable".to_string())?;
    let mut bytes = Vec::new();
    input
        .into_read()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read selector input: {error}"))?;
    parse_lines(bytes)
}

fn parse_lines(bytes: Vec<u8>) -> Result<Vec<String>, String> {
    let input = String::from_utf8(bytes).map_err(|_| "selector input must be UTF-8".to_string())?;
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
    fn parses_commands() {
        assert!(matches!(parse(&args(&[])), Ok(Request::Help(HELP))));
        assert!(matches!(parse(&args(&["start"])), Ok(Request::Start)));
        assert!(matches!(parse(&args(&["launcher"])), Ok(Request::Launcher)));
        assert!(matches!(
            parse(&args(&["clipboard"])),
            Ok(Request::Clipboard)
        ));
        assert!(matches!(
            parse(&args(&["notifications"])),
            Ok(Request::Notifications)
        ));
        assert!(matches!(
            parse(&args(&["notifications", "clear"])),
            Ok(Request::ClearNotifications)
        ));
    }

    #[test]
    fn parses_help_and_version() {
        assert!(matches!(parse(&args(&["--help"])), Ok(Request::Help(HELP))));
        assert!(matches!(
            parse(&args(&["help", "dmenu"])),
            Ok(Request::Help(DMENU_HELP))
        ));
        assert!(matches!(
            parse(&args(&["notifications", "--help"])),
            Ok(Request::Help(NOTIFICATIONS_HELP))
        ));
        assert!(matches!(parse(&args(&["--version"])), Ok(Request::Version)));
    }

    #[test]
    fn parses_dmenu_prompt() {
        let command = parse(&args(&["dmenu", "-p", "System..."])).expect("valid command");
        assert!(matches!(command, Request::Dmenu { prompt } if prompt == "System..."));
    }

    #[test]
    fn rejects_unknown_arguments() {
        assert!(matches!(
            parse(&args(&["unknown"])),
            Err(error) if error == "unknown command 'unknown'"
        ));
        assert!(matches!(
            parse(&args(&["launcher", "extra"])),
            Err(error) if error == "unexpected argument 'extra'"
        ));
        assert!(matches!(
            parse(&args(&["dmenu", "--bad"])),
            Err(error) if error == "unknown option '--bad'"
        ));
        assert!(parse(&args(&["help", "unknown"])).is_err());
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
