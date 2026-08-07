use std::{
    io::{BufRead, BufReader, Read},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use async_channel::Receiver;

/// How long anything in the shell waits before retrying a failed connection,
/// command, or subscription.
pub const RETRY_DELAY: Duration = Duration::from_secs(1);

const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub fn listen<T: 'static>(receiver: Receiver<T>, mut handle: impl FnMut(T) + 'static) {
    gtk::glib::spawn_future_local(async move {
        while let Ok(value) = receiver.recv().await {
            handle(value);
        }
    });
}

pub fn spawn(name: &str, task: impl FnOnce() + Send + 'static) -> bool {
    match thread::Builder::new().name(name.into()).spawn(task) {
        Ok(_) => true,
        Err(error) => {
            eprintln!("varde: failed to spawn {name}: {error}");
            false
        }
    }
}

/// Runs a long-lived command, calling `handle` for every line it prints and
/// restarting it whenever it exits or fails to start.
pub fn watch_lines(
    name: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    mut handle: impl FnMut(&str) + Send + 'static,
) {
    spawn(name, move || {
        loop {
            if let Some(mut child) = spawn_child(program, args) {
                if let Some(output) = child.stdout.take() {
                    for line in BufReader::new(output).lines().map_while(Result::ok) {
                        handle(&line);
                    }
                }
                kill(&mut child);
            }
            thread::sleep(RETRY_DELAY);
        }
    });
}

/// Runs a command to completion, giving up on it after `timeout`.
pub fn command_output(program: &str, args: &[&str], timeout: Duration) -> Option<Vec<u8>> {
    let mut child = spawn_child(program, args)?;
    let mut stdout = child.stdout.take()?;
    let deadline = Instant::now() + timeout;
    thread::scope(|scope| {
        let (output_sender, output_receiver) = mpsc::sync_channel(1);
        scope.spawn(move || {
            let mut bytes = Vec::new();
            let output = stdout.read_to_end(&mut bytes).ok().map(|_| bytes);
            let _ = output_sender.send(output);
        });
        let Some(status) = wait_until(&mut child, deadline) else {
            kill(&mut child);
            return None;
        };
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            kill(&mut child);
            return None;
        };
        let Ok(Some(bytes)) = output_receiver.recv_timeout(remaining) else {
            // The leader may have left a descendant with the output pipe open.
            kill(&mut child);
            return None;
        };
        status.success().then_some(bytes)
    })
}

fn wait_until(child: &mut Child, deadline: Instant) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(EXIT_POLL_INTERVAL),
            _ => return None,
        }
    }
}

pub(crate) fn spawn_child(program: &str, args: &[&str]) -> Option<Child> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            // The child must not outlive the shell when the compositor stops it,
            // and its own group lets `kill` reach whatever it started in turn.
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 || libc::setpgid(0, 0) == -1
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn().ok()
}

/// Kills the child and anything it started, so nothing is left holding its
/// output pipe open.
pub(crate) fn kill(child: &mut Child) {
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_successful_command_output() {
        assert_eq!(
            command_output("sh", &["-c", "printf ready"], Duration::from_secs(1)),
            Some(b"ready".to_vec())
        );
    }

    #[test]
    fn stops_commands_at_the_timeout() {
        assert_eq!(
            command_output("sh", &["-c", "sleep 5"], Duration::from_millis(10)),
            None
        );
    }

    #[test]
    fn stops_commands_that_left_the_output_pipe_open() {
        assert_eq!(
            command_output("sh", &["-c", "sleep 5 | cat"], Duration::from_millis(10)),
            None
        );
    }

    #[test]
    fn successful_leader_cannot_leave_the_output_pipe_open() {
        let started = Instant::now();
        assert_eq!(
            command_output("sh", &["-c", "sleep 2 &"], Duration::from_millis(20)),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
