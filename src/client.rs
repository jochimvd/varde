//! The `varde` command. It forwards commands to the running shell and links
//! only GIO, so it starts without loading GTK and its dependencies.

use std::{
    env,
    ffi::OsString,
    os::unix::process::CommandExt,
    process::{Command, ExitCode},
};

use gio::{glib, prelude::*};
use varde_cli::command::{self, Request};

const SHELL: &str = "varde-shell";

fn main() -> ExitCode {
    let arguments = env::args_os().collect::<Vec<_>>();
    match command::parse(&arguments) {
        Ok(Request::Help(help)) => {
            println!("{help}");
            return ExitCode::SUCCESS;
        }
        Ok(Request::Version) => {
            println!("varde {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("varde: {error}\n\nFor more information, try '--help'.");
            return ExitCode::from(2);
        }
        Ok(Request::Start) => return exec_shell(&arguments),
        Ok(_) => {}
    }

    let application_id = varde_cli::application_id();
    if !is_running(&application_id) {
        // Without a running shell, the command starts one, as GApplication would.
        return exec_shell(&arguments);
    }
    let app = gio::Application::new(
        Some(&application_id),
        gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::IS_LAUNCHER,
    );
    ExitCode::from(u8::from(app.run()))
}

fn is_running(application_id: &str) -> bool {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return false;
    };
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&(application_id,).to_variant()),
        Some(glib::VariantTy::new("(b)").expect("valid variant type")),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
    )
    .ok()
    .and_then(|reply| reply.get::<(bool,)>())
    .is_some_and(|(running,)| running)
}

fn exec_shell(arguments: &[OsString]) -> ExitCode {
    let shell = env::current_exe()
        .map(|path| path.with_file_name(SHELL))
        .unwrap_or_else(|_| SHELL.into());
    let error = Command::new(&shell).args(&arguments[1..]).exec();
    eprintln!("varde: could not start {}: {error}", shell.display());
    ExitCode::from(2)
}
