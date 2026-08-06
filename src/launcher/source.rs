use std::rc::Rc;

use gio::prelude::*;
use gtk::{gdk, gdk::prelude::*};

#[derive(Clone)]
pub(super) struct Item {
    pub id: String,
    pub title: String,
    pub icon: Option<gio::Icon>,
    pub search_terms: Vec<String>,
}

pub(super) enum Outcome {
    Done,
    Return(String),
}

pub(super) trait Source {
    fn items(&self) -> Result<Vec<Item>, String>;
    fn activate(&self, id: &str) -> Result<Outcome, String>;
}

pub(super) fn apps() -> Rc<dyn Source> {
    Rc::new(Apps)
}

pub(super) fn dmenu(lines: Vec<String>) -> Rc<dyn Source> {
    Rc::new(Dmenu { lines })
}

struct Apps;

impl Source for Apps {
    fn items(&self) -> Result<Vec<Item>, String> {
        Ok(gio::AppInfo::all()
            .into_iter()
            .filter(|app| app.should_show())
            .filter_map(|app| {
                let id = app.id()?.to_string();
                let title = app.display_name().to_string();
                let mut search_terms = vec![
                    app.name().to_string(),
                    app.executable().to_string_lossy().into_owned(),
                    id.clone(),
                ];
                if let Ok(desktop) = app.clone().downcast::<gio::DesktopAppInfo>() {
                    if desktop.is_hidden() || desktop.is_nodisplay() {
                        return None;
                    }
                    if let Some(name) = desktop.generic_name() {
                        search_terms.push(name.to_string());
                    }
                    search_terms
                        .extend(desktop.keywords().into_iter().map(|word| word.to_string()));
                }
                Some(Item {
                    id,
                    title,
                    icon: app.icon(),
                    search_terms,
                })
            })
            .collect())
    }

    fn activate(&self, id: &str) -> Result<Outcome, String> {
        let app = gio::DesktopAppInfo::new(id)
            .ok_or_else(|| format!("Application is no longer available: {id}"))?;
        let context = gdk::Display::default().map(|display| display.app_launch_context());
        app.launch(&[] as &[gio::File], context.as_ref())
            .map_err(|error| error.to_string())?;
        Ok(Outcome::Done)
    }
}

struct Dmenu {
    lines: Vec<String>,
}

impl Source for Dmenu {
    fn items(&self) -> Result<Vec<Item>, String> {
        Ok(self
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| Item {
                id: index.to_string(),
                title: line.clone(),
                icon: None,
                search_terms: Vec::new(),
            })
            .collect())
    }

    fn activate(&self, id: &str) -> Result<Outcome, String> {
        let index = id
            .parse::<usize>()
            .map_err(|_| "Invalid selector item".to_string())?;
        self.lines
            .get(index)
            .cloned()
            .map(Outcome::Return)
            .ok_or_else(|| "Selector item is no longer available".to_string())
    }
}
