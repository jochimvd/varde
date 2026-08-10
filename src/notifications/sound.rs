use std::{
    path::PathBuf,
    sync::mpsc::{self, SyncSender},
    time::Duration,
};

const PLAY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Sound {
    File(PathBuf),
    Name(String),
}

#[derive(Clone)]
pub(super) struct Player {
    sender: SyncSender<Sound>,
}

impl Player {
    pub fn start() -> Option<Self> {
        let program = gtk::glib::find_program_in_path("canberra-gtk-play")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        crate::background::spawn("notification-sounds", move || {
            while let Ok(sound) = receiver.recv() {
                play(&program, &sound);
            }
        })
        .then_some(Self { sender })
    }

    pub fn play(&self, sound: Sound) {
        let _ = self.sender.try_send(sound);
    }
}

fn play(program: &std::path::Path, sound: &Sound) {
    let Some(program) = program.to_str() else {
        return;
    };
    let (option, value) = match sound {
        Sound::File(path) => ("--file", path.to_string_lossy()),
        Sound::Name(name) => ("--id", name.into()),
    };
    if crate::background::command_output(program, &[option, &value], PLAY_TIMEOUT).is_none() {
        eprintln!("varde: failed to play notification sound");
    }
}
