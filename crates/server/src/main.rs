use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use mempool_based::{App, Config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return if healthcheck(config.listen).await {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
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

/// `mempool-based healthcheck`: whether the server at `LISTEN_ADDR` answers `/healthz` with 200.
/// The container image has no shell or curl, so its health check runs the binary itself.
async fn healthcheck(listen: SocketAddr) -> bool {
    let ip = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    let check = async {
        let mut stream = TcpStream::connect((ip, listen.port())).await?;
        stream
            .write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        // The status line starts `HTTP/1.x 200`.
        let mut head = [0; 12];
        stream.read_exact(&mut head).await?;
        Ok::<_, std::io::Error>(head.ends_with(b" 200"))
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(5), check).await,
        Ok(Ok(true))
    )
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use mempool_based::Limits;

    #[tokio::test]
    async fn healthcheck_needs_a_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = Config {
            listen: addr,
            web_dir: "missing".into(),
            networks: Vec::new(),
            limits: Limits::default(),
        };
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(App::new(&config).serve(listener, async {
            stopped.await.ok();
        }));
        // An unspecified address is checked on loopback, as LISTEN_ADDR=0.0.0.0:… in the image.
        let any = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), addr.port());
        assert!(healthcheck(any).await);
        stop.send(()).unwrap();
        server.await.unwrap();
        assert!(!healthcheck(addr).await);
    }
}
