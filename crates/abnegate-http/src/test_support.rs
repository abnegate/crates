use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

/// Answer one request on a loopback port with `response`, verbatim, and close.
pub(crate) async fn serve_once(response: impl Into<Vec<u8>>) -> u16 {
    let response = response.into();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let port = listener.local_addr().expect("a bound address").port();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("a connection");
        let mut request = [0_u8; 4_096];
        let _ = stream.read(&mut request).await;
        let _ = stream.write_all(&response).await;
        let _ = stream.shutdown().await;
    });

    port
}
