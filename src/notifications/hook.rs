use std::{
    fs,
    io::{self, Write},
    os::fd::AsRawFd,
    path::Path,
    process::{Child, Stdio},
    sync::mpsc::{self, SyncSender},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::state::{Notification, Urgency};

const QUEUE_CAPACITY: usize = 64;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Config {
    hook: Option<Command>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    argv: Vec<String>,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}

fn default_timeout() -> u64 {
    3000
}

fn load(path: &Path) -> Result<Option<Command>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read configuration: {error}")),
    };
    parse(&text)
}

fn parse(text: &str) -> Result<Option<Command>, String> {
    // Parser diagnostics can echo configuration, including command arguments.
    let config: Config = toml::from_str(text).map_err(|_| "invalid notifications.toml")?;
    if let Some(command) = &config.hook {
        if command
            .argv
            .first()
            .is_none_or(|program| program.trim().is_empty())
            || command.argv.iter().any(|arg| arg.contains('\0'))
        {
            return Err("hook.argv needs a nonempty executable and no NUL bytes".into());
        }
        if !(1..=60_000).contains(&command.timeout_ms) {
            return Err("hook.timeout_ms must be between 1 and 60000".into());
        }
    }
    Ok(config.hook)
}

#[derive(Serialize)]
pub(super) struct Event {
    version: u8,
    event: &'static str,
    session: String,
    id: u32,
    revision: u64,
    replaced: bool,
    app_name: String,
    desktop_entry: String,
    summary: String,
    body: String,
    urgency: &'static str,
    dnd: bool,
}

#[derive(Clone)]
pub(super) struct Hook {
    sender: SyncSender<Event>,
    session: String,
}

impl Hook {
    pub fn start() -> Option<Self> {
        let path = gtk::glib::user_config_dir().join("varde/notifications.toml");
        let command = match load(&path) {
            Ok(command) => command?,
            Err(error) => {
                eprintln!("varde: notification hook disabled: {error}");
                return None;
            }
        };
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        crate::background::spawn("notification-hook", move || {
            for event in receiver {
                let mut input = serde_json::to_vec(&event).expect("notification event is JSON");
                input.push(b'\n');
                if let Err(error) = execute(&command, &input) {
                    eprintln!("varde: notification hook failed: {error}");
                }
            }
        })
        .then(|| Self {
            sender,
            session: gtk::glib::uuid_string_random().to_string(),
        })
    }

    pub fn received(&self, notification: &Notification, replaced: bool, dnd: bool) {
        let event = Event {
            version: 1,
            event: "received",
            session: self.session.clone(),
            id: notification.id,
            revision: notification.revision,
            replaced,
            app_name: notification.app_name.clone(),
            desktop_entry: notification.desktop_entry.clone(),
            summary: notification.summary.clone(),
            body: notification.body.clone(),
            urgency: match notification.urgency {
                Urgency::Low => "low",
                Urgency::Normal => "normal",
                Urgency::Critical => "critical",
            },
            dnd,
        };
        if let Err(error) = self.sender.try_send(event) {
            let reason = match error {
                mpsc::TrySendError::Full(_) => "queue full",
                mpsc::TrySendError::Disconnected(_) => "worker unavailable",
            };
            eprintln!("varde: notification hook dropped event: {reason}");
        }
    }

    #[cfg(test)]
    pub(super) fn capture() -> (Self, mpsc::Receiver<Event>) {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        (
            Self {
                sender,
                session: "test-session".into(),
            },
            receiver,
        )
    }
}

fn execute(command: &Command, input: &[u8]) -> Result<(), &'static str> {
    let deadline = Instant::now() + Duration::from_millis(command.timeout_ms);
    let args = command.argv[1..]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let mut child = crate::background::child_command(&command.argv[0], &args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|_| "could not start executable")?;
    let result = deliver(&mut child, input, deadline);
    // Also clean up descendants if the executable exits before they do.
    crate::background::kill(&mut child);
    result
}

fn deliver(child: &mut Child, mut input: &[u8], deadline: Instant) -> Result<(), &'static str> {
    let stdin = child.stdin.take().ok_or("stdin unavailable")?;
    let fd = stdin.as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags == -1 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == -1 {
            return Err("could not configure stdin");
        }
    }
    let mut stdin = Some(stdin);
    loop {
        if Instant::now() >= deadline {
            return Err("timed out");
        }
        if let Some(pipe) = &mut stdin {
            match pipe.write(input) {
                Ok(0) if !input.is_empty() => return Err("stdin closed early"),
                Ok(written) => input = &input[written..],
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err("could not write stdin"),
            }
            if input.is_empty() {
                stdin.take();
            }
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() && input.is_empty() => return Ok(()),
            Ok(Some(_)) => return Err("executable failed or exited before input was delivered"),
            Ok(None) => {}
            Err(_) => return Err("could not wait for executable"),
        }
        thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(script: &str, timeout_ms: u64) -> Command {
        Command {
            argv: vec!["sh".into(), "-c".into(), script.into()],
            timeout_ms,
        }
    }

    #[test]
    fn optional_configuration_and_literal_arguments() {
        assert!(parse("").unwrap().is_none());
        let command = parse("[hook]\nargv = ['/path with spaces/hook', '$HOME', '']")
            .unwrap()
            .unwrap();
        assert_eq!(command.argv, ["/path with spaces/hook", "$HOME", ""]);
        assert_eq!(command.timeout_ms, 3000);
        for invalid in [
            "[hooks]",
            "[hook]\nargv = []",
            "[hook]\nargv = [' ']",
            "[hook]\nargv = ['hook']\ntimeout_ms = 0",
            "[hook]\nargv = ['hook']\ntimeout_ms = 60001",
            "[hook]\nargv = ['hook']\ntimeout = 3",
            "[hook]\nargv = [\"bad\\u0000path\"]",
        ] {
            assert!(parse(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn preserves_json_and_arguments_without_shell_interpolation() {
        let literal = "quotes: \" newline:\n$(false); `false` <b>hello</b> 🦀";
        let input = serde_json::to_vec(&serde_json::json!({"body": literal})).unwrap();
        let mut command = command(
            "test \"$(cat)\" = \"$1\" && test \"$2\" = '$HOME; $(false)'",
            3000,
        );
        command.argv.extend([
            "hook-test".into(),
            String::from_utf8(input.clone()).unwrap(),
            "$HOME; $(false)".into(),
        ]);
        assert_eq!(execute(&command, &input), Ok(()));
    }

    #[test]
    fn bounds_nonreading_and_hung_hooks_and_reports_failures() {
        let input = vec![b'x'; 1024 * 1024];
        let start = Instant::now();
        assert_eq!(execute(&command("sleep 30", 100), &input), Err("timed out"));
        assert_eq!(
            execute(&command("cat >/dev/null; sleep 30", 100), b"event"),
            Err("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(execute(&command("cat >/dev/null; exit 7", 3000), b"event").is_err());
        assert!(
            execute(
                &Command {
                    argv: vec!["/does-not-exist/varde-hook".into()],
                    timeout_ms: 100
                },
                b"event"
            )
            .is_err()
        );
        assert_eq!(
            execute(&command("cat >/dev/null", 3000), b"next event"),
            Ok(())
        );
    }

    #[test]
    fn cleans_up_descendants_after_timeout_and_normal_exit() {
        for script in [
            "cat >/dev/null; (sleep 0.5; touch \"$1\") & wait",
            "cat >/dev/null; (sleep 0.5; touch \"$1\") & exit 0",
        ] {
            let marker = std::env::temp_dir().join(format!(
                "varde-hook-descendant-{}",
                gtk::glib::uuid_string_random()
            ));
            let mut command = command(script, 200);
            command
                .argv
                .extend(["hook-test".into(), marker.to_str().unwrap().into()]);
            let result = execute(&command, b"event");
            if script.ends_with("wait") {
                assert_eq!(result, Err("timed out"));
            } else {
                assert_eq!(result, Ok(()));
            }
            thread::sleep(Duration::from_millis(600));
            let leaked = marker.exists();
            if leaked {
                fs::remove_file(&marker).unwrap();
            }
            assert!(!leaked, "hook left a running descendant");
        }
    }

    #[test]
    fn queue_is_bounded_and_drops_new_events_without_waiting() {
        let (hook, receiver) = Hook::capture();
        let start = Instant::now();
        for id in 0..QUEUE_CAPACITY + 1 {
            hook.received(
                &Notification {
                    id: id as u32,
                    ..Notification::default()
                },
                false,
                false,
            );
        }
        assert!(start.elapsed() < Duration::from_secs(1));
        let events = receiver.try_iter().collect::<Vec<_>>();
        assert_eq!(events.len(), QUEUE_CAPACITY);
        assert_eq!(events.first().unwrap().id, 0);
        assert_eq!(events.last().unwrap().id, QUEUE_CAPACITY as u32 - 1);
    }
}
