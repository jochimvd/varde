pub(super) const ICON_SIZE: i32 = 14;
const ITEM_PATH: &str = "/StatusNotifierItem";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct ItemId {
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
pub(super) struct Item {
    pub id: ItemId,
    pub status: String,
    pub tooltip: Option<String>,
    pub icon_name: String,
    pub pixmap: Option<Pixmap>,
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

pub(super) enum Event {
    Upsert(Item),
    Remove(ItemId),
}

#[derive(Clone)]
pub(super) struct Pixmap {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

pub(super) fn tooltip(title: &str, text: &str) -> Option<String> {
    match (title.is_empty(), text.is_empty()) {
        (true, true) => None,
        (false, true) => Some(title.into()),
        (true, false) => Some(text.into()),
        (false, false) => Some(format!("{title}\n{text}")),
    }
}

pub(super) fn select_pixmap(pixmaps: Vec<(i32, i32, Vec<u8>)>) -> Option<Pixmap> {
    let (width, height, argb) = pixmaps
        .into_iter()
        .filter(|(width, height, pixels)| {
            *width > 0 && *height > 0 && pixels.len() == *width as usize * *height as usize * 4
        })
        .min_by_key(|(width, height, _)| {
            let edge = (*width).max(*height);
            ((edge - ICON_SIZE).unsigned_abs(), std::cmp::Reverse(edge))
        })?;
    let rgba = argb
        .chunks_exact(4)
        .flat_map(|pixel| [pixel[1], pixel[2], pixel[3], pixel[0]])
        .collect();
    Some(Pixmap {
        width,
        height,
        rgba,
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

    #[test]
    fn selects_the_nearest_valid_pixmap_and_converts_argb() {
        let pixmap = select_pixmap(vec![
            (8, 8, vec![0; 8 * 8 * 4]),
            (16, 16, [1, 2, 3, 4].repeat(16 * 16)),
            (32, 32, vec![0; 32 * 32 * 4]),
        ])
        .expect("a valid pixmap");
        assert_eq!((pixmap.width, pixmap.height), (16, 16));
        assert_eq!(&pixmap.rgba[..4], &[2, 3, 4, 1]);
    }
}
