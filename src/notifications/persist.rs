use std::{
    fs, io,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use super::state::{Saved, Store};

fn path() -> PathBuf {
    gtk::glib::user_state_dir().join("varde/notifications.json")
}

pub(super) fn load() -> Store {
    match load_from(&path()) {
        Ok(saved) => Store::restore(saved),
        Err(error) => {
            eprintln!("varde: could not restore notifications: {error}");
            Store::default()
        }
    }
}

pub(super) fn save(saved: &Saved) {
    if let Err(error) = save_to(&path(), saved) {
        eprintln!("varde: could not save notifications: {error}");
    }
}

fn load_from(path: &Path) -> io::Result<Saved> {
    let text = match fs::read(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Saved::default()),
        Err(error) => return Err(error),
    };
    Ok(serde_json::from_slice(&text)?)
}

fn save_to(path: &Path, saved: &Saved) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    // Notification text is private, so the file is readable only by its owner.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(saved)?)?;
    file.sync_all()?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::fs::PermissionsExt,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };

    use super::*;
    use crate::notifications::{
        image::Thumbnail,
        state::{Incoming, Picture},
    };

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let number = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "varde-notification-persist-{}-{number}",
                std::process::id()
            )))
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn round_trip(store: &Store) -> Store {
        let dir = TestDir::new();
        let path = dir.0.join("state/notifications.json");
        save_to(&path, &store.saved()).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        Store::restore(load_from(&path).unwrap())
    }

    #[test]
    fn restores_notifications_into_the_center_without_popups() {
        let mut store = Store::default();
        let picture = Picture::Pixels(Thumbnail {
            width: 1,
            height: 1,
            rowstride: 4,
            bytes: Arc::from([1, 2, 3, 4]),
        });
        let first = store.notify(Incoming {
            app_name: "volume".into(),
            tag: "volume".into(),
            summary: "50%".into(),
            ..Incoming::default()
        });
        let second = store.notify(Incoming {
            summary: "with picture".into(),
            picture: Some(picture.clone()),
            ..Incoming::default()
        });
        store.notify(Incoming {
            transient: true,
            ..Incoming::default()
        });
        store.set_dnd(true);

        let mut restored = round_trip(&store);
        let notifications = restored
            .notifications()
            .map(|(notification, show_popup)| (notification.clone(), show_popup))
            .collect::<Vec<_>>();
        assert_eq!(notifications.len(), 2);
        assert_eq!(notifications[0].0.id, second);
        assert_eq!(notifications[0].0.picture, Some(picture));
        assert_eq!(notifications[1].0.id, first);
        assert!(notifications.iter().all(|(_, show_popup)| !show_popup));
        assert!(restored.dnd());

        let replaced = restored.notify_with_eviction(Incoming {
            app_name: "volume".into(),
            tag: "volume".into(),
            summary: "55%".into(),
            ..Incoming::default()
        });
        assert_eq!(replaced.id, first);
        let next = restored.notify(Incoming::default());
        assert!(next != first && next != second);
    }

    #[test]
    fn missing_files_restore_an_empty_store() {
        let dir = TestDir::new();
        let saved = load_from(&dir.0.join("notifications.json")).unwrap();
        assert_eq!(Store::restore(saved).notifications().count(), 0);
    }

    #[test]
    fn rejects_corrupt_files_and_invalid_pictures() {
        let dir = TestDir::new();
        let path = dir.0.join("notifications.json");
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(&path, b"{").unwrap();
        assert!(load_from(&path).is_err());

        let mut store = Store::default();
        store.notify(Incoming {
            picture: Some(Picture::Pixels(Thumbnail {
                width: 1,
                height: 1,
                rowstride: 4,
                bytes: Arc::from([1, 2, 3, 4]),
            })),
            ..Incoming::default()
        });
        let text = serde_json::to_string(&store.saved())
            .unwrap()
            .replace("\"width\":1", "\"width\":2");
        fs::write(&path, text).unwrap();
        assert!(load_from(&path).is_err());
    }
}
