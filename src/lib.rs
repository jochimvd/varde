//! Command-line parsing shared by the `varde` client and the `varde-shell`
//! process. Kept free of GTK so the client starts quickly.

pub mod command;

const APPLICATION_ID: &str = "org.varde.desktop";

pub fn application_id() -> String {
    std::env::var("VARDE_DEV_APP_ID").unwrap_or_else(|_| APPLICATION_ID.to_string())
}
