//! A scriptable plugin for the `plugin_proc` tests (built by `cargo test`; also a minimal example
//! of the protocol in docs/plugins.md). The first argument picks the behavior:
//!
//! - `echo`      answers `init` with `ready`, `query` with one result, `action` with `done`; exits on `shutdown`
//! - `hang`      like `echo` until `ready`, then never answers anything
//! - `deaf`      says `ready`, then never reads stdin again (ignores `shutdown` and EOF)
//! - `silent`    never reads stdin and never writes
//! - `flood`     says `ready`, then writes log lines as fast as it can
//! - `badjson`   says `ready`, then writes 30 lines that are not JSON
//! - `oversized` like `echo`, but each query is first answered with a 100 KB line
//! - `crash`     exits with code 3 at once
//! - `tree FILE` like `echo`, after starting a `silent` child of its own and writing its pid to FILE
//! - `stubborn`  like `echo`, but ignores `shutdown` and the end of stdin (only the job object stops it)
//! - `slow FILE` like `echo`, but each query takes 30 ms to answer; writes its own pid to FILE
//!
//! Every mode logs the `settings` messages it receives, so a test can see that they arrived.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{Value, json};

fn send(value: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn sleep_forever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "echo".to_owned());
    match mode.as_str() {
        "silent" => sleep_forever(),
        "crash" => std::process::exit(3),
        "tree" => {
            let child = std::process::Command::new(std::env::current_exe().unwrap()).arg("silent").spawn().unwrap();
            if let Some(file) = args.next() {
                std::fs::write(file, child.id().to_string()).unwrap();
            }
            std::mem::forget(child);
        }
        "slow" => {
            if let Some(file) = args.next() {
                std::fs::write(file, std::process::id().to_string()).unwrap();
            }
        }
        _ => {}
    }

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        match message["type"].as_str() {
            Some("init") => {
                eprintln!("hello from stderr");
                send(json!({"type": "log", "level": "info", "message": "fake plugin up"}));
                send(json!({"type": "ready", "protocol": 1}));
                match mode.as_str() {
                    "deaf" => sleep_forever(),
                    "flood" => loop {
                        send(json!({"type": "log", "level": "debug", "message": "flood"}));
                    },
                    "badjson" => {
                        for _ in 0..30 {
                            println!("this is not json");
                        }
                        let _ = std::io::stdout().flush();
                    }
                    _ => {}
                }
            }
            Some("settings") => send(json!({"type": "log", "level": "info", "message": format!("settings: {}", message["settings"])})),
            Some("query") if mode != "hang" => {
                if mode == "oversized" {
                    println!("{}", "x".repeat(100 * 1024));
                }
                if mode == "slow" {
                    std::thread::sleep(Duration::from_millis(30));
                }
                let text = message["text"].as_str().unwrap_or_default();
                send(json!({"type": "results", "id": message["id"], "items": [{"key": "e", "title": format!("echo: {text}")}]}));
            }
            Some("action") if mode != "hang" => send(json!({"type": "done", "req": message["req"]})),
            Some("shutdown") if mode != "stubborn" => std::process::exit(0),
            _ => {}
        }
    }
    if mode == "stubborn" {
        sleep_forever();
    }
}
