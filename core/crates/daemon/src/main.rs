//! `esp32benchd`: serves the core's JSON-RPC API on a Unix socket. See `docs/rpc.md`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::signal::unix::{SignalKind, signal};

fn usage() -> ! {
    eprintln!("usage: esp32benchd [--socket PATH]");
    std::process::exit(2)
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut socket: Option<PathBuf> = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--socket") => socket = Some(args.next().unwrap_or_else(|| usage()).into()),
            Some("--version") => {
                println!(
                    "{} {} (protocol {})",
                    esp32bench_daemon::NAME,
                    esp32bench_daemon::VERSION,
                    esp32bench_daemon::PROTOCOL
                );
                return Ok(());
            }
            _ => usage(),
        }
    }
    let socket = match socket {
        Some(s) => s,
        None => esp32bench_rpc::default_socket_path()
            .context("XDG_RUNTIME_DIR is not set; pass --socket")?,
    };

    let (listener, _lock) = esp32bench_daemon::bind(&socket)
        .with_context(|| format!("binding {}", socket.display()))?;
    eprintln!("esp32benchd: listening on {}", socket.display());

    let router = Arc::new(esp32bench_daemon::router());
    let mut term = signal(SignalKind::terminate())?;
    tokio::select! {
        r = esp32bench_rpc::serve(listener, router) => r.context("serving")?,
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    // Dropping the lock removes the socket.
    eprintln!("esp32benchd: shutting down");
    Ok(())
}
