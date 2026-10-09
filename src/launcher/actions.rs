use std::{fs, path::Path, rc::Rc};

use async_channel::Sender;
use gtk::glib;
use serde::Deserialize;

use super::source::{Activation, Event, Item, Items, Outcome, Source, Visual};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    actions: Vec<Action>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    name: String,
    command: String,
}

struct Actions {
    config: Result<Config, String>,
}

pub(super) fn source() -> Rc<dyn Source> {
    let path = glib::user_config_dir().join("varde/actions.toml");
    Rc::new(Actions {
        config: load(&path),
    })
}

fn load(path: &Path) -> Result<Config, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Config {
                actions: Vec::new(),
            });
        }
        Err(error) => return Err(format!("Could not read {}: {error}", path.display())),
    };
    parse(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn parse(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|error| error.to_string())?;
    for (index, action) in config.actions.iter().enumerate() {
        if action.name.trim().is_empty() || action.command.trim().is_empty() {
            return Err(format!(
                "Action {} needs a nonempty name and command",
                index + 1
            ));
        }
        if action.command.contains('\0') {
            return Err(format!("Action {} command contains a NUL byte", index + 1));
        }
    }
    Ok(config)
}

impl Source for Actions {
    fn items(&self, _generation: u64, _sender: Sender<Event>) -> Items {
        Items::Ready(self.config.as_ref().map_err(Clone::clone).map(|config| {
            config
                .actions
                .iter()
                .enumerate()
                .map(|(index, action)| Item {
                    id: index.to_string(),
                    title: action.name.clone(),
                    visual: Visual::None,
                    search_terms: vec![action.command.clone()],
                })
                .collect()
        }))
    }

    fn activate(&self, id: &str, _generation: u64, _sender: Sender<Event>) -> Activation {
        let action = self.config.as_ref().ok().and_then(|config| {
            id.parse::<usize>()
                .ok()
                .and_then(|index| config.actions.get(index))
        });
        Activation::Ready(match action {
            Some(action) => launch(action),
            None => Err("Action is no longer available".into()),
        })
    }
}

fn launch(action: &Action) -> Result<Outcome, String> {
    let name = action.name.clone();
    crate::background::run(&["bash", "-c", &action.command], move |result| {
        if let Err(error) = result {
            eprintln!("varde: action {name:?} failed: {error}");
        }
    })
    .map_err(|error| format!("Could not launch {}: {error}", action.name))?;
    Ok(Outcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands_and_multiline_scripts_without_changing_shell_syntax() {
        let config = parse(
            r#"
[[actions]]
name = "Restart audio"
command = 'systemctl --user restart pipewire'

[[actions]]
name = "Script"
command = '''
printf '%s\n' "$HOME"
"$HOME/scripts/example.sh" | cat
'''
"#,
        )
        .unwrap();
        assert_eq!(config.actions.len(), 2);
        assert_eq!(config.actions[0].name, "Restart audio");
        assert_eq!(
            config.actions[1].command,
            "printf '%s\\n' \"$HOME\"\n\"$HOME/scripts/example.sh\" | cat\n"
        );
        assert!(parse("").unwrap().actions.is_empty());
    }

    #[test]
    fn rejects_malformed_or_incomplete_actions() {
        for text in [
            "[[actions]",
            "[[actions]]\nname = 'Missing command'",
            "[[actions]]\nname = ' '\ncommand = 'true'",
            "[[actions]]\nname = 'Empty'\ncommand = ' '",
            "[[actions]]\nname = 'Typo'\ncommand = 'true'\ncomand = 'false'",
            "[[action]]\nname = 'Typo'\ncommand = 'true'",
            "[[actions]]\nname = 'NUL'\ncommand = \"\\u0000\"",
        ] {
            assert!(parse(text).is_err(), "accepted {text}");
        }
    }
}
