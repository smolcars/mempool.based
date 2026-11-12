use std::process::ExitCode;

use mempool_based::{Config, router};
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
    eprintln!("listening on http://{}", config.listen);
    let app = router(&config.web_dir);
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
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
