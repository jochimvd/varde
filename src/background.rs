use std::{thread, time::Duration};

use async_channel::Receiver;

pub fn listen<T: 'static>(receiver: Receiver<T>, mut handle: impl FnMut(T) + 'static) {
    gtk::glib::spawn_future_local(async move {
        while let Ok(value) = receiver.recv().await {
            handle(value);
        }
    });
}

pub fn spawn(name: &str, task: impl FnOnce() + Send + 'static) {
    if let Err(error) = thread::Builder::new().name(name.into()).spawn(task) {
        eprintln!("shell: failed to spawn {name}: {error}");
    }
}

pub fn repeat<T: Send + 'static>(
    name: &str,
    interval: Duration,
    fetch: impl Fn() -> T + Send + 'static,
    handle: impl FnMut(T) + 'static,
) {
    let (sender, receiver) = async_channel::unbounded();
    listen(receiver, handle);
    spawn(name, move || {
        loop {
            if sender.send_blocking(fetch()).is_err() {
                break;
            }
            thread::sleep(interval);
        }
    });
}
