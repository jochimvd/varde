use std::rc::Rc;

use async_channel::Sender;
use gio::prelude::*;
use gtk::{gdk, gdk::prelude::*};

use super::clipboard;

#[derive(Clone)]
pub(super) struct Item {
    pub id: String,
    pub title: String,
    pub visual: Visual,
    pub search_terms: Vec<String>,
}

#[derive(Clone)]
pub(super) enum Visual {
    None,
    Icon(gio::Icon),
    Image,
    Text,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ImageKind {
    Thumbnail,
    Preview,
}

pub(super) struct ImagePixels {
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub rgba: Vec<u8>,
}

pub(super) struct LoadedItem {
    pub id: String,
    pub title: String,
    pub visual: LoadedVisual,
    pub search_terms: Vec<String>,
}

pub(super) enum LoadedVisual {
    None,
    Image,
    Text,
}

impl From<LoadedItem> for Item {
    fn from(item: LoadedItem) -> Self {
        Self {
            id: item.id,
            title: item.title,
            visual: match item.visual {
                LoadedVisual::None => Visual::None,
                LoadedVisual::Image => Visual::Image,
                LoadedVisual::Text => Visual::Text,
            },
            search_terms: item.search_terms,
        }
    }
}

pub(super) enum Outcome {
    Done,
    Return(String),
}

pub(super) enum Event {
    Items {
        generation: u64,
        items: Result<Vec<LoadedItem>, String>,
    },
    Activation {
        generation: u64,
        outcome: Result<Outcome, String>,
    },
    Image {
        generation: u64,
        id: String,
        kind: ImageKind,
        pixels: Option<ImagePixels>,
    },
    Text {
        generation: u64,
        id: String,
        text: Option<String>,
    },
}

pub(super) enum Items {
    Ready(Result<Vec<Item>, String>),
    Pending,
}

pub(super) enum Activation {
    Ready(Result<Outcome, String>),
    Pending,
}

pub(super) trait Source {
    fn items(&self, _generation: u64, _sender: Sender<Event>) -> Items;
    fn activate(&self, id: &str, _generation: u64, _sender: Sender<Event>) -> Activation;

    fn set_generation(&self, _generation: u64) {}

    fn request_image(
        &self,
        _id: &str,
        _kind: ImageKind,
        _width: i32,
        _height: i32,
        _generation: u64,
        _sender: Sender<Event>,
    ) -> bool {
        false
    }

    fn request_text(&self, _id: &str, _generation: u64, _sender: Sender<Event>) -> bool {
        false
    }
}

pub(super) fn apps() -> Rc<dyn Source> {
    Rc::new(Apps)
}

pub(super) fn dmenu(lines: Vec<String>) -> Rc<dyn Source> {
    Rc::new(Dmenu { lines })
}

pub(super) fn clipboard() -> Rc<dyn Source> {
    Rc::new(clipboard::Clipboard::new())
}

struct Apps;

impl Source for Apps {
    fn items(&self, _generation: u64, _sender: Sender<Event>) -> Items {
        Items::Ready(Ok(gio::AppInfo::all()
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
                    visual: app.icon().map_or(Visual::None, Visual::Icon),
                    search_terms,
                })
            })
            .collect()))
    }

    fn activate(&self, id: &str, _generation: u64, _sender: Sender<Event>) -> Activation {
        Activation::Ready(launch_app(id))
    }
}

fn launch_app(id: &str) -> Result<Outcome, String> {
    let app = gio::DesktopAppInfo::new(id)
        .ok_or_else(|| format!("Application is no longer available: {id}"))?;
    let context = gdk::Display::default().map(|display| display.app_launch_context());
    app.launch(&[] as &[gio::File], context.as_ref())
        .map_err(|error| error.to_string())?;
    Ok(Outcome::Done)
}

struct Dmenu {
    lines: Vec<String>,
}

impl Source for Dmenu {
    fn items(&self, _generation: u64, _sender: Sender<Event>) -> Items {
        Items::Ready(Ok(self
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| Item {
                id: index.to_string(),
                title: line.clone(),
                visual: Visual::None,
                search_terms: Vec::new(),
            })
            .collect()))
    }

    fn activate(&self, id: &str, _generation: u64, _sender: Sender<Event>) -> Activation {
        let result = id
            .parse::<usize>()
            .map_err(|_| "Invalid selector item".to_string())
            .and_then(|index| {
                self.lines
                    .get(index)
                    .cloned()
                    .map(Outcome::Return)
                    .ok_or_else(|| "Selector item is no longer available".to_string())
            });
        Activation::Ready(result)
    }
}
