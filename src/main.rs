#![windows_subsystem = "windows"]

mod actions;
mod apps;
mod autostart;
mod commands;
mod config;
mod everything;
mod fontmenu;
mod fonts;
mod hook;
mod hotkey;
mod install;
mod lang;
mod lineedit;
mod menu;
mod plugin_check;
mod plugin_host;
mod plugin_icon;
mod plugin_proc;
mod plugin_proto;
mod plugins;
mod power;
mod power_overlay;
mod render;
mod settings;
mod style;
mod search;
mod themes;
mod togglemenu;
mod usage;
mod window;
mod winsearch;

use std::path::PathBuf;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Toggle,
    Show,
    Hide,
    Quit,
}

impl Command {
    fn parse(arg: &str) -> Option<Self> {
        match arg {
            "toggle" => Some(Self::Toggle),
            "show" => Some(Self::Show),
            "hide" => Some(Self::Hide),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }

    pub fn from_wparam(value: usize) -> Option<Self> {
        [Self::Toggle, Self::Show, Self::Hide, Self::Quit]
            .into_iter()
            .find(|c| *c as usize == value)
    }
}

fn main() -> windows::core::Result<()> {
    // Internal: the app indexer child process (see apps.rs); not a menu command.
    if std::env::args().nth(1).as_deref() == Some("index") {
        return apps::run_indexer();
    }
    // Internal: opens one Control Panel task (see apps::launch_setting).
    if std::env::args().nth(1).as_deref() == Some("setting") {
        return std::env::args().nth(2).map_or(Ok(()), |id| apps::run_setting(&id));
    }

    // `cpmenu.exe plugin check <folder>`: a developer tool, runs without the menu or its mutex.
    if std::env::args().nth(1).as_deref() == Some("plugin") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        std::process::exit(plugin_check::cli(&args));
    }

    // `cpmenu.exe install [exe]`: installs that exe (default: this one) as the autostart copy.
    // cpmenu-setup.exe calls it with the exe it carries, so the steps live here once.
    if std::env::args().nth(1).as_deref() == Some("install") {
        return run_install();
    }

    let command = match std::env::args().nth(1) {
        Some(arg) => match Command::parse(&arg) {
            Some(c) => Some(c),
            None => return Ok(()),
        },
        None => None,
    };

    // The mutex handle is intentionally kept alive for the whole process lifetime.
    let _mutex = unsafe { CreateMutexW(None, false, w!("Local\\cpmenu"))? };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        window::send(command.unwrap_or(Command::Toggle));
        return Ok(());
    }

    match command {
        Some(Command::Quit) | Some(Command::Hide) => Ok(()),
        // Started without a command (autostart) the menu stays hidden until asked.
        initial => window::run(initial),
    }
}

/// The `install [exe]` command: loud on stderr, since the setup exe shows it in a message box.
fn run_install() -> windows::core::Result<()> {
    let source = match std::env::args().nth(2) {
        Some(path) => Ok(PathBuf::from(path)),
        None => std::env::current_exe(),
    };
    match source.and_then(|source| install::run(&source)) {
        Ok(_) => Ok(()),
        Err(err) => {
            eprintln!("cpmenu install failed: {err}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Command;

    #[test]
    fn parses_known_commands() {
        assert_eq!(Command::parse("toggle"), Some(Command::Toggle));
        assert_eq!(Command::parse("quit"), Some(Command::Quit));
        assert_eq!(Command::parse("TOGGLE"), None);
    }

    #[test]
    fn wparam_roundtrip() {
        for c in [Command::Toggle, Command::Show, Command::Hide, Command::Quit] {
            assert_eq!(Command::from_wparam(c as usize), Some(c));
        }
        assert_eq!(Command::from_wparam(99), None);
    }
}
