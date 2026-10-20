use std::sync::Arc;

pub(super) const ICON_SIZE: i32 = 14;
const ITEM_PATH: &str = "/StatusNotifierItem";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ItemId {
    pub service: String,
    pub path: String,
}

impl ItemId {
    pub fn parse(request: &str, sender: &str) -> Option<Self> {
        if request.starts_with('/') {
            valid_path(request).then(|| Self {
                service: sender.into(),
                path: request.into(),
            })
        } else if !request.is_empty() {
            Some(Self {
                service: request.into(),
                path: ITEM_PATH.into(),
            })
        } else {
            None
        }
    }

    pub fn registration(&self) -> String {
        format!("{}{}", self.service, self.path)
    }

    pub fn from_registration(registration: &str) -> Option<Self> {
        let Some((service, path)) = registration.split_once('/') else {
            // A watcher may list an item by bus name alone, as `parse` also accepts.
            return (!registration.is_empty()).then(|| Self {
                service: registration.into(),
                path: ITEM_PATH.into(),
            });
        };
        let path = format!("/{path}");
        (!service.is_empty() && valid_path(&path)).then(|| Self {
            service: service.into(),
            path,
        })
    }
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.ends_with('/')
        && path.split('/').skip(1).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

#[derive(Clone)]
pub struct Item {
    pub id: ItemId,
    pub status: String,
    pub tooltip: Option<String>,
    pub icon_name: String,
    pub pixmaps: Arc<[Pixmap]>,
    pub item_is_menu: bool,
    pub menu_path: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct MenuItem {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub separator: bool,
    pub icon_name: Option<String>,
    pub toggle: Option<Toggle>,
    pub children: Vec<MenuItem>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Toggle {
    pub kind: ToggleKind,
    pub active: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ToggleKind {
    Checkmark,
    Radio,
}

pub enum Event {
    Upsert(Item),
    Remove(ItemId),
}

impl Event {
    pub fn id(&self) -> &ItemId {
        match self {
            Event::Upsert(item) => &item.id,
            Event::Remove(id) => id,
        }
    }
}

#[derive(Clone)]
pub struct Pixmap {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

/// Records `event` in the items the tray shows; passive items are hidden.
pub fn apply(items: &mut Vec<Item>, event: &Event) {
    match event {
        Event::Upsert(item) if item.status != "Passive" => {
            match items.iter_mut().find(|current| current.id == item.id) {
                Some(current) => *current = item.clone(),
                None => items.push(item.clone()),
            }
        }
        Event::Upsert(Item { id, .. }) | Event::Remove(id) => {
            items.retain(|current| current.id != *id);
        }
    }
}

pub(super) fn tooltip(title: &str, text: &str) -> Option<String> {
    match (title.is_empty(), text.is_empty()) {
        (true, true) => None,
        (false, true) => Some(title.into()),
        (true, false) => Some(text.into()),
        (false, false) => Some(format!("{title}\n{text}")),
    }
}

/// Keeps the valid pixmaps, converted from ARGB to RGBA.
pub(super) fn valid_pixmaps(pixmaps: Vec<(i32, i32, Vec<u8>)>) -> Arc<[Pixmap]> {
    pixmaps
        .into_iter()
        .filter(|(width, height, pixels)| {
            *width > 0 && *height > 0 && pixels.len() == *width as usize * *height as usize * 4
        })
        .map(|(width, height, argb)| Pixmap {
            width,
            height,
            rgba: argb
                .as_chunks()
                .0
                .iter()
                .flat_map(|&[a, r, g, b]| [r, g, b, a])
                .collect(),
        })
        .collect()
}

pub(super) fn select_pixmap(pixmaps: &[Pixmap], size: i32) -> Option<&Pixmap> {
    pixmaps.iter().min_by_key(|pixmap| {
        let edge = pixmap.width.max(pixmap.height);
        ((edge - size).unsigned_abs(), std::cmp::Reverse(edge))
    })
}

pub(super) fn scale_pixmap(pixmap: &Pixmap, size: i32) -> Pixmap {
    let edge = pixmap.width.max(pixmap.height);
    if edge == size {
        return pixmap.clone();
    }

    let width = (pixmap.width * size / edge).max(1);
    let height = (pixmap.height * size / edge).max(1);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let source_x = x * pixmap.width / width;
            let source_y = y * pixmap.height / height;
            let index = (source_y * pixmap.width + source_x) as usize * 4;
            rgba.extend_from_slice(&pixmap.rgba[index..index + 4]);
        }
    }
    Pixmap {
        width,
        height,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_service_and_object_path_registration() {
        assert_eq!(
            ItemId::parse("org.kde.StatusNotifierItem-42-1", ":1.42"),
            Some(ItemId {
                service: "org.kde.StatusNotifierItem-42-1".into(),
                path: ITEM_PATH.into(),
            })
        );
        assert_eq!(
            ItemId::parse("/StatusNotifierItem", ":1.42"),
            Some(ItemId {
                service: ":1.42".into(),
                path: "/StatusNotifierItem".into(),
            })
        );
        assert_eq!(ItemId::parse("/not-valid/", ":1.42"), None);
    }

    #[test]
    fn parses_registrations_with_and_without_an_object_path() {
        assert_eq!(
            ItemId::from_registration(":1.42/StatusNotifierItem"),
            Some(ItemId {
                service: ":1.42".into(),
                path: "/StatusNotifierItem".into(),
            })
        );
        assert_eq!(
            ItemId::from_registration("org.kde.StatusNotifierItem-42-1"),
            Some(ItemId {
                service: "org.kde.StatusNotifierItem-42-1".into(),
                path: ITEM_PATH.into(),
            })
        );
        assert_eq!(ItemId::from_registration(""), None);
    }

    fn item(service: &str, status: &str) -> Item {
        Item {
            id: ItemId {
                service: service.into(),
                path: ITEM_PATH.into(),
            },
            status: status.into(),
            tooltip: None,
            icon_name: String::new(),
            pixmaps: Arc::from([]),
            item_is_menu: false,
            menu_path: None,
        }
    }

    #[test]
    fn keeps_shown_items_in_order_and_hides_passive_ones() {
        let services = |items: &[Item]| {
            items
                .iter()
                .map(|item| (item.id.service.clone(), item.status.clone()))
                .collect::<Vec<_>>()
        };
        let mut items = Vec::new();
        apply(&mut items, &Event::Upsert(item("a", "Active")));
        apply(&mut items, &Event::Upsert(item("b", "Active")));
        apply(&mut items, &Event::Upsert(item("a", "NeedsAttention")));
        assert_eq!(
            services(&items),
            [
                ("a".into(), "NeedsAttention".into()),
                ("b".into(), "Active".into())
            ]
        );

        apply(&mut items, &Event::Upsert(item("a", "Passive")));
        apply(&mut items, &Event::Upsert(item("c", "Passive")));
        assert_eq!(services(&items), [("b".into(), "Active".into())]);

        apply(&mut items, &Event::Remove(item("b", "Active").id));
        assert!(items.is_empty());
    }

    #[test]
    fn selects_the_nearest_valid_pixmap_and_converts_argb() {
        let pixmaps = valid_pixmaps(vec![
            (8, 8, vec![0; 8 * 8 * 4]),
            (16, 16, [1, 2, 3, 4].repeat(16 * 16)),
            (24, 24, vec![0; 3]),
            (32, 32, vec![0; 32 * 32 * 4]),
        ]);
        assert_eq!(pixmaps.len(), 3);

        let pixmap = select_pixmap(&pixmaps, ICON_SIZE).expect("a valid pixmap");
        assert_eq!((pixmap.width, pixmap.height), (16, 16));
        assert_eq!(&pixmap.rgba[..4], &[2, 3, 4, 1]);
        let pixmap = select_pixmap(&pixmaps, ICON_SIZE * 2).expect("a valid pixmap");
        assert_eq!((pixmap.width, pixmap.height), (32, 32));
    }
}
