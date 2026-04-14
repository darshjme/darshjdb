// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// Integration test: start the RESP3 server on an ephemeral port and
// verify a raw TCP PING/PONG round-trip plus SET/GET round-trip.

use std::sync::Arc;
use std::time::Duration;

use ddb_cache::DdbCache;
use ddb_cache_server::{ServerConfig, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

async fn spawn_server() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let config = ServerConfig { addr, password: None };
    let cache = Arc::new(DdbCache::new());
    tokio::spawn(async move {
        let _ = serve(config, cache).await;
    });
    tokio::time::sleep(Duration::from_millis(80)).await;
    addr
}

async fn read_reply(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = [0u8; 1024];
    let n = timeout(Duration::from_secs(2), stream.read(&mut buf))
        .await
        .expect("server response timed out")
        .expect("read error");
    buf[..n].to_vec()
}

#[tokio::test]
async fn resp3_ping_returns_pong() {
    let addr = spawn_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();

    stream.write_all(b"PING\r\n").await.unwrap();
    stream.flush().await.unwrap();

    let reply = read_reply(&mut stream).await;
    assert_eq!(
        reply,
        b"+PONG\r\n",
        "expected +PONG, got {:?}",
        String::from_utf8_lossy(&reply)
    );
}

#[tokio::test]
async fn resp3_set_get_roundtrip() {
    let addr = spawn_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();

    let set_cmd = b"*3\r\n$3\r\nSET\r\n$3\r\nfoo\r\n$3\r\nbar\r\n";
    stream.write_all(set_cmd).await.unwrap();
    stream.flush().await.unwrap();
    let reply = read_reply(&mut stream).await;
    assert_eq!(reply, b"+OK\r\n");

    let get_cmd = b"*2\r\n$3\r\nGET\r\n$3\r\nfoo\r\n";
    stream.write_all(get_cmd).await.unwrap();
    stream.flush().await.unwrap();
    let reply = read_reply(&mut stream).await;
    assert_eq!(reply, b"$3\r\nbar\r\n");
}

#[tokio::test]
async fn resp3_hello_upgrade_returns_map() {
    let addr = spawn_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();

    let hello = b"*2\r\n$5\r\nHELLO\r\n$1\r\n3\r\n";
    stream.write_all(hello).await.unwrap();
    stream.flush().await.unwrap();

    let reply = read_reply(&mut stream).await;
    assert!(
        reply.starts_with(b"%"),
        "expected map reply, got {:?}",
        String::from_utf8_lossy(&reply)
    );
}
