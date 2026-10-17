use gio::prelude::*;

pub const HELP: &str = "\
Varde desktop shell

Usage: varde <COMMAND>

Commands:
  start          Start the desktop shell
  launcher       Toggle the application launcher
  actions        Toggle the action menu
  clipboard      Toggle the clipboard launcher
  notifications  Manage notifications
  title          Write to the bar's title
  dmenu          Select a line read from standard input
  help           Print help for a command

Options:
  -h, --help     Print help
  -V, --version  Print version";

const START_HELP: &str = "Start the Varde desktop shell

Usage: varde start";
const LAUNCHER_HELP: &str = "Toggle the application launcher

Usage: varde launcher";
const ACTIONS_HELP: &str = "Toggle the action menu

Usage: varde actions";
const CLIPBOARD_HELP: &str = "Toggle the clipboard launcher

Usage: varde clipboard";
const NOTIFICATIONS_HELP: &str = "Manage notifications

Usage: varde notifications [clear]";
const TITLE_HELP: &str = "Write to the bar's title

Usage: varde title set [--seq <N>] [JSON]
       varde title clear [--seq <N>] <ID>

Without JSON, set reads it from standard input. The most recently set title is
shown until it is cleared or its ttl_ms expires. A --seq lower than or equal to
one already applied to the same ID is ignored.

JSON: {\"version\": 1, \"id\": \"...\", \"text\": \"...\", \"icon\": \"...\",
       \"ttl_ms\": 0, \"items_delay_ms\": 0,
       \"items\": [{\"key\": \"...\", \"label\": \"...\", \"pinned\": false}]}";
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
    Actions,
    Notifications,
    ClearNotifications,
    Dmenu {
        prompt: String,
    },
    TitleSet {
        seq: Option<u64>,
        json: Option<String>,
    },
    TitleClear {
        seq: Option<u64>,
        id: String,
    },
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
        [command] if command == "actions" => Ok(Request::Actions),
        [command] if command == "clipboard" => Ok(Request::Clipboard),
        [command] if command == "notifications" => Ok(Request::Notifications),
        [command, action] if command == "notifications" && action == "clear" => {
            Ok(Request::ClearNotifications)
        }
        [help_command, command] if help_command == "help" => help(command),
        [command, option] if option == "-h" || option == "--help" => help(command),
        [command, options @ ..] if command == "dmenu" => parse_dmenu(options),
        [command, options @ ..] if command == "title" => parse_title(options),
        [command, argument, ..]
            if matches!(
                command.as_str(),
                "start" | "launcher" | "actions" | "clipboard" | "notifications"
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
        "actions" => Ok(Request::Help(ACTIONS_HELP)),
        "clipboard" => Ok(Request::Help(CLIPBOARD_HELP)),
        "notifications" => Ok(Request::Help(NOTIFICATIONS_HELP)),
        "dmenu" => Ok(Request::Help(DMENU_HELP)),
        "title" => Ok(Request::Help(TITLE_HELP)),
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

fn parse_title(options: &[String]) -> Result<Request, String> {
    let Some((action, mut options)) = options.split_first() else {
        return Ok(Request::Help(TITLE_HELP));
    };
    if action == "-h" || action == "--help" {
        return Ok(Request::Help(TITLE_HELP));
    }
    let mut seq = None;
    if let [flag, value, rest @ ..] = options
        && flag == "--seq"
    {
        seq = Some(
            value
                .parse()
                .map_err(|_| format!("invalid sequence number '{value}'"))?,
        );
        options = rest;
    }
    match (action.as_str(), options) {
        ("set", []) => Ok(Request::TitleSet { seq, json: None }),
        ("set", [json]) => Ok(Request::TitleSet {
            seq,
            json: Some(json.clone()),
        }),
        ("clear", [id]) => Ok(Request::TitleClear {
            seq,
            id: id.clone(),
        }),
        ("set" | "clear", [.., argument]) => Err(format!("unexpected argument '{argument}'")),
        ("clear", []) => Err("clear requires an ID".into()),
        (action, _) => Err(format!("unknown title action '{action}'")),
    }
}

pub async fn read_all(input: gio::InputStream) -> Result<String, String> {
    let bytes = read_bytes(input).await?;
    String::from_utf8(bytes).map_err(|_| "input must be UTF-8".to_string())
}

pub async fn read_lines(input: gio::InputStream) -> Result<Vec<String>, String> {
    parse_lines(read_bytes(input).await?)
}

async fn read_bytes(input: gio::InputStream) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    loop {
        let chunk = input
            .read_bytes_future(64 * 1024, gtk::glib::Priority::DEFAULT)
            .await
            .map_err(|error| format!("could not read input: {error}"))?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
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
        assert!(matches!(parse(&args(&["actions"])), Ok(Request::Actions)));
        assert!(matches!(
            parse(&args(&["actions", "--help"])),
            Ok(Request::Help(ACTIONS_HELP))
        ));
        assert!(parse(&args(&["actions", "extra"])).is_err());
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
    fn parses_title_actions() {
        assert!(matches!(
            parse(&args(&["title", "set"])),
            Ok(Request::TitleSet {
                seq: None,
                json: None
            })
        ));
        assert!(matches!(
            parse(&args(&["title", "set", "--seq", "7", "{}"])),
            Ok(Request::TitleSet { seq: Some(7), json: Some(json) }) if json == "{}"
        ));
        assert!(matches!(
            parse(&args(&["title", "clear", "submap"])),
            Ok(Request::TitleClear { seq: None, id }) if id == "submap"
        ));
        assert!(matches!(
            parse(&args(&["title", "clear", "--seq", "3", "submap"])),
            Ok(Request::TitleClear { seq: Some(3), id }) if id == "submap"
        ));
        assert!(matches!(
            parse(&args(&["title", "--help"])),
            Ok(Request::Help(TITLE_HELP))
        ));
        for invalid in [
            &["title", "clear"][..],
            &["title", "clear", "a", "b"],
            &["title", "set", "--seq", "x", "{}"],
            &["title", "show"],
        ] {
            assert!(parse(&args(invalid)).is_err(), "accepted {invalid:?}");
        }
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
