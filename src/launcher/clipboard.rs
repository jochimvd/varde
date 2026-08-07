use std::{
    io::{Cursor, Write},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use super::source::{
    Activation, Event, ImageKind, ImagePixels, Items, LoadedItem, LoadedVisual, Outcome, Source,
};
use async_channel::{Receiver, Sender};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct Clipboard {
    previews: Sender<PreviewRequest>,
    commands: Sender<CommandRequest>,
    generation: Arc<AtomicU64>,
}

enum PreviewRequest {
    Image {
        generation: u64,
        id: String,
        kind: ImageKind,
        width: i32,
        height: i32,
        result: Sender<Event>,
    },
    Text {
        generation: u64,
        id: String,
        result: Sender<Event>,
    },
}

enum CommandRequest {
    Items {
        generation: u64,
        result: Sender<Event>,
    },
    Activate {
        generation: u64,
        id: String,
        result: Sender<Event>,
    },
}

impl Clipboard {
    pub fn new() -> Self {
        let (previews, preview_receiver) = async_channel::unbounded();
        let (commands, command_receiver) = async_channel::unbounded();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = Arc::clone(&generation);
        crate::background::spawn("clipboard-previews", move || {
            preview_worker(preview_receiver, worker_generation)
        });
        let worker_generation = Arc::clone(&generation);
        crate::background::spawn("clipboard-commands", move || {
            command_worker(command_receiver, worker_generation)
        });
        Self {
            previews,
            commands,
            generation,
        }
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Release);
    }
}

impl Source for Clipboard {
    fn items(&self, generation: u64, result: Sender<Event>) -> Items {
        if self
            .commands
            .try_send(CommandRequest::Items { generation, result })
            .is_ok()
        {
            Items::Pending
        } else {
            Items::Ready(Err("Could not start clipboard history worker".into()))
        }
    }

    fn activate(&self, id: &str, generation: u64, result: Sender<Event>) -> Activation {
        if self
            .commands
            .try_send(CommandRequest::Activate {
                generation,
                id: id.to_string(),
                result,
            })
            .is_ok()
        {
            Activation::Pending
        } else {
            Activation::Ready(Err("Could not start clipboard restore worker".into()))
        }
    }

    fn set_generation(&self, generation: u64) {
        self.generation.store(generation, Ordering::Release);
    }

    fn request_image(
        &self,
        id: &str,
        kind: ImageKind,
        width: i32,
        height: i32,
        generation: u64,
        result: Sender<Event>,
    ) -> bool {
        self.previews
            .try_send(PreviewRequest::Image {
                generation,
                id: id.to_string(),
                kind,
                width,
                height,
                result,
            })
            .is_ok()
    }

    fn request_text(&self, id: &str, generation: u64, result: Sender<Event>) -> bool {
        self.previews
            .try_send(PreviewRequest::Text {
                generation,
                id: id.to_string(),
                result,
            })
            .is_ok()
    }
}

fn parse_list(output: &str) -> Result<Vec<LoadedItem>, String> {
    let items = output.lines().filter_map(parse_line).collect::<Vec<_>>();
    if !output.is_empty() && items.is_empty() {
        return Err("Could not parse clipboard history".into());
    }
    Ok(items)
}

fn parse_line(line: &str) -> Option<LoadedItem> {
    let (id, preview) = line.split_once('\t')?;
    id.parse::<u64>().ok()?;

    if let Some(dimensions) = image_dimensions(preview) {
        return Some(LoadedItem {
            id: id.to_string(),
            title: format!("Image · {}", dimensions.replace('x', "×")),
            visual: LoadedVisual::Image,
            search_terms: vec!["image".into(), preview.into()],
        });
    }

    Some(LoadedItem {
        id: id.to_string(),
        title: if preview.is_empty() {
            "Empty text".into()
        } else {
            preview.into()
        },
        visual: if preview.starts_with("[[ binary data ") {
            LoadedVisual::None
        } else {
            LoadedVisual::Text
        },
        search_terms: Vec::new(),
    })
}

fn image_dimensions(preview: &str) -> Option<&str> {
    let content = preview
        .strip_prefix("[[ binary data ")?
        .strip_suffix(" ]]")?;
    let mut previous: Option<&str> = None;
    for dimensions in content.split_whitespace() {
        if parse_dimensions(dimensions).is_some() {
            let format = previous?;
            return ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"]
                .iter()
                .any(|supported| format.eq_ignore_ascii_case(supported))
                .then_some(dimensions);
        }
        previous = Some(dimensions);
    }
    None
}

fn parse_dimensions(value: &str) -> Option<(i32, i32)> {
    let (width, height) = value.split_once('x')?;
    let width = width.parse().ok()?;
    let height = height.parse().ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

fn preview_worker(receiver: Receiver<PreviewRequest>, generation: Arc<AtomicU64>) {
    while let Ok(request) = receiver.recv_blocking() {
        let request_generation = match &request {
            PreviewRequest::Image { generation, .. } | PreviewRequest::Text { generation, .. } => {
                *generation
            }
        };
        if generation.load(Ordering::Acquire) != request_generation {
            continue;
        }
        match request {
            PreviewRequest::Image {
                generation: request_generation,
                id,
                kind,
                width,
                height,
                result,
            } => {
                let pixels = decode_image(&id, kind, width, height);
                if generation.load(Ordering::Acquire) == request_generation {
                    let _ = result.send_blocking(Event::Image {
                        generation: request_generation,
                        id,
                        kind,
                        pixels,
                    });
                }
            }
            PreviewRequest::Text {
                generation: request_generation,
                id,
                result,
            } => {
                let text = decode_text(&id);
                if generation.load(Ordering::Acquire) == request_generation {
                    let _ = result.send_blocking(Event::Text {
                        generation: request_generation,
                        id,
                        text,
                    });
                }
            }
        }
    }
}

fn command_worker(receiver: Receiver<CommandRequest>, generation: Arc<AtomicU64>) {
    while let Ok(request) = receiver.recv_blocking() {
        let request_generation = match &request {
            CommandRequest::Items { generation, .. }
            | CommandRequest::Activate { generation, .. } => *generation,
        };
        if generation.load(Ordering::Acquire) != request_generation {
            continue;
        }
        match request {
            CommandRequest::Items {
                generation: request_generation,
                result,
            } => {
                let items = load_items();
                if generation.load(Ordering::Acquire) == request_generation {
                    let _ = result.send_blocking(Event::Items {
                        generation: request_generation,
                        items,
                    });
                }
            }
            CommandRequest::Activate {
                generation: request_generation,
                id,
                result,
            } => {
                let outcome = copy_entry(&id).map(|()| Outcome::Done);
                if generation.load(Ordering::Acquire) == request_generation {
                    let _ = result.send_blocking(Event::Activation {
                        generation: request_generation,
                        outcome,
                    });
                }
            }
        }
    }
}

fn load_items() -> Result<Vec<LoadedItem>, String> {
    let output = crate::background::command_output("cliphist", &["list"], COMMAND_TIMEOUT)
        .ok_or_else(|| "Could not read clipboard history".to_string())?;
    let output = String::from_utf8(output)
        .map_err(|_| "Clipboard history contains invalid text".to_string())?;
    parse_list(&output)
}

fn decode_text(id: &str) -> Option<String> {
    let bytes = crate::background::command_output("cliphist", &["decode", id], COMMAND_TIMEOUT)?;
    String::from_utf8(bytes).ok()
}

fn decode_image(id: &str, kind: ImageKind, width: i32, height: i32) -> Option<ImagePixels> {
    let bytes = crate::background::command_output("cliphist", &["decode", id], COMMAND_TIMEOUT)?;
    let pixbuf = gdk_pixbuf::Pixbuf::from_read(Cursor::new(bytes)).ok()?;
    let image = match kind {
        ImageKind::Thumbnail => crop_thumbnail(&pixbuf, width, height)?,
        ImageKind::Preview => fit_preview(&pixbuf, width, height)?,
    };
    let rgba = if image.has_alpha() {
        image
    } else {
        image.add_alpha(false, 0, 0, 0).ok()?
    };
    Some(ImagePixels {
        width: rgba.width(),
        height: rgba.height(),
        stride: rgba.rowstride() as usize,
        rgba: rgba.read_pixel_bytes().as_ref().to_vec(),
    })
}

fn crop_thumbnail(
    pixbuf: &gdk_pixbuf::Pixbuf,
    target_width: i32,
    target_height: i32,
) -> Option<gdk_pixbuf::Pixbuf> {
    let scale = (f64::from(target_width) / f64::from(pixbuf.width()))
        .max(f64::from(target_height) / f64::from(pixbuf.height()));
    let width = (f64::from(pixbuf.width()) * scale).ceil() as i32;
    let height = (f64::from(pixbuf.height()) * scale).ceil() as i32;
    let scaled = pixbuf.scale_simple(width, height, gdk_pixbuf::InterpType::Bilinear)?;
    Some(scaled.new_subpixbuf(
        (width - target_width) / 2,
        (height - target_height) / 2,
        target_width,
        target_height,
    ))
}

fn fit_preview(
    pixbuf: &gdk_pixbuf::Pixbuf,
    target_width: i32,
    target_height: i32,
) -> Option<gdk_pixbuf::Pixbuf> {
    let scale = (f64::from(target_width) / f64::from(pixbuf.width()))
        .min(f64::from(target_height) / f64::from(pixbuf.height()))
        .min(1.0);
    pixbuf.scale_simple(
        (f64::from(pixbuf.width()) * scale).round().max(1.0) as i32,
        (f64::from(pixbuf.height()) * scale).round().max(1.0) as i32,
        gdk_pixbuf::InterpType::Bilinear,
    )
}

fn copy_entry(id: &str) -> Result<(), String> {
    id.parse::<u64>()
        .map_err(|_| "Invalid clipboard entry".to_string())?;
    let decoded = crate::background::command_output("cliphist", &["decode", id], COMMAND_TIMEOUT)
        .ok_or_else(|| "Could not decode clipboard entry".to_string())?;

    let timeout = format!("{}s", COMMAND_TIMEOUT.as_secs());
    let mut copy = Command::new("timeout")
        .args(["--signal=KILL", timeout.as_str(), "wl-copy"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start wl-copy: {error}"))?;
    copy.stdin
        .take()
        .ok_or_else(|| "Could not open wl-copy input".to_string())?
        .write_all(&decoded)
        .map_err(|error| format!("Could not write clipboard entry: {error}"))?;
    if !copy
        .wait()
        .map_err(|error| format!("Could not wait for wl-copy: {error}"))?
        .success()
    {
        return Err("Could not restore clipboard entry".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_and_images_in_history_order() {
        let items = parse_list(
            "42\tmost recent text\n41\t[[ binary data 12 KiB png 474x598 ]]\n40\tolder\n",
        )
        .unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["42", "41", "40"]
        );
        assert_eq!(items[1].title, "Image · 474×598");
        assert!(matches!(items[1].visual, LoadedVisual::Image));
    }

    #[test]
    fn ignores_malformed_lines_but_rejects_unparseable_output() {
        assert_eq!(parse_list("bad\n1\tvalid\n").unwrap().len(), 1);
        assert!(parse_list("bad\n").is_err());
        assert!(parse_list("").unwrap().is_empty());
    }

    #[test]
    fn recognizes_supported_image_metadata() {
        let dimensions = image_dimensions("[[ binary data 2 MiB jpeg 1920x1080 ]]").unwrap();
        assert_eq!(parse_dimensions(dimensions), Some((1920, 1080)));
        assert!(image_dimensions("[[ binary data 2 MiB JPEG 1920x1080 ]]").is_some());
        assert!(image_dimensions("[[ binary data 2 KiB pdf ]]").is_none());
    }

    #[test]
    fn crops_thumbnails_and_contains_previews() {
        let wide =
            gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 1_000, 200).unwrap();
        let thumbnail = crop_thumbnail(&wide, 56, 36).unwrap();
        let preview = fit_preview(&wide, 280, 280).unwrap();
        assert_eq!((thumbnail.width(), thumbnail.height()), (56, 36));
        assert_eq!((preview.width(), preview.height()), (280, 56));
    }

    #[test]
    fn dropping_clipboard_advances_generation() {
        let (previews, _) = async_channel::unbounded();
        let (commands, _) = async_channel::unbounded();
        let generation = Arc::new(AtomicU64::new(7));
        let clipboard = Clipboard {
            previews,
            commands,
            generation: Arc::clone(&generation),
        };

        drop(clipboard);

        assert_eq!(generation.load(Ordering::Acquire), 8);
    }

    #[test]
    fn reports_unavailable_preview_worker() {
        let (previews, preview_receiver) = async_channel::unbounded();
        let (commands, _command_receiver) = async_channel::unbounded();
        let generation = Arc::new(AtomicU64::new(7));
        let clipboard = Clipboard {
            previews,
            commands,
            generation,
        };
        drop(preview_receiver);

        let (image_result, _) = async_channel::unbounded();
        assert!(!clipboard.request_image("42", ImageKind::Thumbnail, 56, 36, 7, image_result,));
        let (text_result, _) = async_channel::unbounded();
        assert!(!clipboard.request_text("42", 7, text_result));
    }
}
