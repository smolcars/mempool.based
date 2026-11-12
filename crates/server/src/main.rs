use std::process::ExitCode;

use mempool_based::{App, Config};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !config.web_dir.join("index.html").is_file() {
        eprintln!(
            "warning: {} has no index.html; build the frontend or set WEB_DIR",
            config.web_dir.display()
        );
    }
    let listener = match TcpListener::bind(config.listen).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("error: cannot listen on {}: {e}", config.listen);
            return ExitCode::FAILURE;
        }
    };
    let names: Vec<&str> = config.networks.iter().map(|n| n.network.name()).collect();
    if names.is_empty() {
        eprintln!("warning: no network is configured; only the frontend and /healthz are served");
    }
    let bound = listener.local_addr().unwrap_or(config.listen);
    eprintln!("listening on http://{bound} for {}", names.join(", "));
    App::new(&config).serve(listener, shutdown_signal()).await;
    ExitCode::SUCCESS
}

/// Resolves on Ctrl-C or SIGTERM; in-flight requests then finish before exit.
async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    eprintln!("shutting down");
}
