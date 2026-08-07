use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};

use async_channel::Sender;
use gtk::{gdk, prelude::*};

use super::source::{Event, ImageKind, Source};

pub(super) struct Preview {
    root: gtk::Box,
    space: gtk::Box,
    stack: gtk::Stack,
    picture: gtk::Picture,
    text: gtk::Label,
    id: RefCell<Option<String>>,
    images: RefCell<HashMap<String, Option<gdk::MemoryTexture>>>,
    images_pending: RefCell<HashSet<String>>,
    texts: RefCell<HashMap<String, Option<String>>>,
    texts_pending: RefCell<HashSet<String>>,
    width: i32,
    height: i32,
}

impl Preview {
    pub(super) fn new(width: i32, height: i32) -> Self {
        let picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Contain)
            .hexpand(true)
            .vexpand(true)
            .build();
        let text = gtk::Label::builder()
            .hexpand(true)
            .vexpand(true)
            .xalign(0.0)
            .yalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build();
        let text_scroll = gtk::ScrolledWindow::builder()
            .child(&text)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .build();
        let message = gtk::Label::builder()
            .label("Could not load image preview")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        message.add_css_class("launcher-preview-message");

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(&picture, Some("image"));
        stack.add_named(&text_scroll, Some("text"));
        stack.add_named(&message, Some("message"));

        let panel = gtk::Box::builder()
            .vexpand(true)
            .valign(gtk::Align::Fill)
            .build();
        panel.add_css_class("launcher-preview");
        panel.append(&stack);

        let offset = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        offset.set_can_target(false);
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_homogeneous(true);
        root.set_can_target(false);
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.append(&offset);
        root.append(&panel);
        root.set_visible(false);

        let space = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        space.set_visible(false);

        Self {
            root,
            space,
            stack,
            picture,
            text,
            id: RefCell::new(None),
            images: RefCell::new(HashMap::new()),
            images_pending: RefCell::new(HashSet::new()),
            texts: RefCell::new(HashMap::new()),
            texts_pending: RefCell::new(HashSet::new()),
            width,
            height,
        }
    }

    pub(super) fn root(&self) -> &gtk::Box {
        &self.root
    }

    pub(super) fn space(&self) -> &gtk::Box {
        &self.space
    }

    pub(super) fn is_visible(&self) -> bool {
        self.root.is_visible()
    }

    pub(super) fn reset(&self) {
        self.images.borrow_mut().clear();
        self.images_pending.borrow_mut().clear();
        self.texts.borrow_mut().clear();
        self.texts_pending.borrow_mut().clear();
        self.hide();
    }

    pub(super) fn show_image(
        &self,
        id: &str,
        source: &dyn Source,
        generation: u64,
        events: Sender<Event>,
    ) {
        self.show(id, "image");
        match self.images.borrow().get(id).cloned() {
            Some(Some(texture)) => {
                self.picture.set_paintable(Some(&texture));
                return;
            }
            Some(None) => {
                self.show(id, "message");
                return;
            }
            None => {}
        }
        self.picture.set_paintable(gdk::Paintable::NONE);
        if !self.images_pending.borrow_mut().insert(id.to_string()) {
            return;
        }
        if !source.request_image(
            id,
            ImageKind::Preview,
            self.width,
            self.height,
            generation,
            events,
        ) {
            self.images_pending.borrow_mut().remove(id);
            self.images.borrow_mut().insert(id.to_string(), None);
            self.show(id, "message");
        }
    }

    pub(super) fn show_text(
        &self,
        id: &str,
        source: &dyn Source,
        generation: u64,
        events: Sender<Event>,
    ) {
        if let Some(text) = self.texts.borrow().get(id) {
            self.show_cached_text(id, text.as_deref());
            return;
        }
        self.hide();
        self.id.replace(Some(id.to_string()));
        if self.texts_pending.borrow_mut().insert(id.to_string())
            && !source.request_text(id, generation, events)
        {
            self.texts_pending.borrow_mut().remove(id);
            self.texts.borrow_mut().insert(id.to_string(), None);
            self.hide();
        }
    }

    pub(super) fn image_loaded(&self, id: String, texture: Option<gdk::MemoryTexture>) {
        self.images_pending.borrow_mut().remove(&id);
        self.images.borrow_mut().insert(id.clone(), texture.clone());
        if self.id.borrow().as_deref() != Some(id.as_str()) {
            return;
        }
        if let Some(texture) = texture {
            self.picture.set_paintable(Some(&texture));
        } else {
            self.show(&id, "message");
        }
    }

    pub(super) fn text_loaded(&self, id: String, text: Option<String>) {
        self.texts_pending.borrow_mut().remove(&id);
        self.texts.borrow_mut().insert(id.clone(), text);
        if self.id.borrow().as_deref() == Some(id.as_str()) {
            let texts = self.texts.borrow();
            self.show_cached_text(&id, texts.get(&id).and_then(Option::as_deref));
        }
    }

    pub(super) fn hide(&self) {
        self.id.take();
        self.root.set_visible(false);
        self.space.set_visible(false);
    }

    fn show_cached_text(&self, id: &str, text: Option<&str>) {
        if let Some(text) = text.filter(|text| is_multiline(text)) {
            self.text.set_text(text);
            self.show(id, "text");
        } else {
            self.hide();
        }
    }

    fn show(&self, id: &str, child: &str) {
        self.id.replace(Some(id.to_string()));
        self.stack.set_visible_child_name(child);
        self.space.set_visible(true);
        self.root.set_visible(true);
    }
}

fn is_multiline(text: &str) -> bool {
    text.contains('\n') || text.contains('\r')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_only_multiline_text() {
        assert!(!is_multiline("one line"));
        assert!(is_multiline("first\nsecond"));
        assert!(is_multiline("first\r\nsecond"));
    }
}
