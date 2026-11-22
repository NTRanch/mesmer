//! mesmer — MCP stdio server (thin shell over the `mesmer` lib).
//!
//! Startup: validate the embedded canon (a bad canon is a typed
//! refusal and a non-zero exit, never a panic), open the store at
//! `$MESMER_DATA_DIR`, then serve. Modern protocol era only
//! (2026-07-28): `initialize` is refused at the stdio front door
//! by the era guard with the typed `-32022` error.
//!
//! Maintenance verb: `mesmer tools-list` prints the tools/list
//! payload as JSON (for surface measurement) and exits.

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::sync::Arc;

use mesmer::canon::Canon;
use mesmer::guard;
use mesmer::server::{MesmerServer, tools_list_json};
use mesmer::store::{DATA_DIR_ENV, Store};
use mesmer::tools::Mesmer;
use rmcp::ServiceExt;
use rmcp::transport::async_rw::AsyncRwTransport;
use tokio::sync::Mutex;

/// Duplex buffer between the era guard and the SDK service.
const DUPLEX_BUFFER_BYTES: usize = 1024 * 1024;
/// Bound on the pump drain at shutdown.
const PUMP_DRAIN_SECS: u64 = 2;

fn load_state() -> Result<Mesmer, String> {
    let canon = Canon::embedded().map_err(|e| format!("canon refused: {e}"))?;
    let dir = std::env::var_os(DATA_DIR_ENV)
        .ok_or_else(|| format!("{DATA_DIR_ENV} is not set; refusing to guess a data directory"))?;
    let store = Store::open(dir).map_err(|e| format!("store refused: {e}"))?;
    Ok(Mesmer { canon, store })
}

async fn run(state: Mesmer) -> std::io::Result<()> {
    let (client_side, server_side) = tokio::io::duplex(DUPLEX_BUFFER_BYTES);
    let (server_read, server_write) = tokio::io::split(server_side);
    let (client_read, client_write) = tokio::io::split(client_side);
    let stdout: guard::SharedStdout = Arc::new(Mutex::new(tokio::io::stdout()));
    let ordering = guard::Ordering::new();
    let handler = MesmerServer::new(state);
    let serve = tokio::spawn(async move {
        let transport = AsyncRwTransport::new_server(server_read, server_write);
        match handler.serve(transport).await {
            Ok(running) => running
                .waiting()
                .await
                .map(|_| ())
                .map_err(|e| std::io::Error::other(e.to_string())),
            // Bare EOF before any lifecycle frame is a clean end.
            Err(rmcp::service::ServerInitializeError::ConnectionClosed(_)) => Ok(()),
            Err(e) => Err(std::io::Error::other(e.to_string())),
        }
    });
    let mut pump = tokio::spawn(guard::pump(
        client_read,
        Arc::clone(&stdout),
        Arc::clone(&ordering),
    ));
    let guard_result = guard::run(tokio::io::stdin(), client_write, stdout, ordering).await;
    let serve_result = match serve.await {
        Ok(result) => result,
        Err(e) => Err(std::io::Error::other(e.to_string())),
    };
    let drain = std::time::Duration::from_secs(PUMP_DRAIN_SECS);
    if tokio::time::timeout(drain, &mut pump).await.is_err() {
        pump.abort();
    }
    guard_result.and(serve_result)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("tools-list") {
        println!("{}", tools_list_json());
        return ExitCode::SUCCESS;
    }
    let state = match load_state() {
        Ok(state) => state,
        Err(e) => {
            eprintln!("mesmer: {e}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("mesmer: cannot start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(state)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mesmer: serve loop ended: {e}");
            ExitCode::FAILURE
        }
    }
}
