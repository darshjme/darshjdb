// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache-server :: server — TCP accept loop + per-connection
// Framed<TcpStream, RESP3Codec> pipeline. Factored out of `main.rs` so
// integration tests and embedded mode reuse the same code path.

use std::net::SocketAddr;
use std::sync::Arc;

use ddb_cache::DdbCache;
use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

use crate::codec::{RESP3Codec, RespFrame};
use crate::dispatch::{Dispatcher, Session};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub addr: SocketAddr,
    pub password: Option<String>,
}

impl ServerConfig {
    pub fn from_env() -> Self {
        let port: u16 = std::env::var("DARSH_CACHE_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(7701);
        let password = std::env::var("DARSH_CACHE_PASSWORD").ok();
        Self {
            addr: SocketAddr::from(([0u8, 0, 0, 0], port)),
            password,
        }
    }
}

pub async fn serve(config: ServerConfig, cache: Arc<DdbCache>) -> std::io::Result<()> {
    let listener = TcpListener::bind(config.addr).await?;
    tracing::info!(addr = %config.addr, "ddb-cache-server listening (RESP3)");
    let dispatcher = Arc::new(Dispatcher::new(cache, config.password.clone()));

    loop {
        let (socket, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                continue;
            }
        };
        let dispatcher = dispatcher.clone();
        let auth_required = config.password.is_some();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(socket, peer, dispatcher, auth_required).await {
                tracing::debug!(error = %e, "connection closed");
            }
        });
    }
}

async fn handle_connection(
    socket: TcpStream,
    peer: SocketAddr,
    dispatcher: Arc<Dispatcher>,
    auth_required: bool,
) -> std::io::Result<()> {
    tracing::debug!(%peer, "new RESP3 connection");
    let mut framed = Framed::new(socket, RESP3Codec);
    let mut session = Session::new(auth_required);

    while let Some(frame_res) = framed.next().await {
        let frame = match frame_res {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!(%peer, error = %e, "decode error");
                let _ = framed
                    .send(RespFrame::err(format!("ERR protocol error: {e}")))
                    .await;
                break;
            }
        };
        let response = dispatcher.handle(&mut session, frame).await;
        if let Err(e) = framed.send(response).await {
            tracing::debug!(%peer, error = %e, "write error");
            break;
        }
    }

    Ok(())
}
