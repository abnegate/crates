use std::io::ErrorKind;
use std::net::TcpListener;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

/// Loopback written as a dotted quad, as an IPv4-mapped IPv6 literal, and as
/// the single decimal number a URL parser normalises back to `127.0.0.1`.
pub(crate) const LOOPBACK_SPELLINGS: [&str; 3] = ["127.0.0.1", "[::ffff:127.0.0.1]", "2130706433"];

/// A live loopback listener that is never accepted from, so any connection a
/// test makes to it waits in the backlog where [`assert_untouched`] finds it.
pub(crate) fn loopback_listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    listener
        .set_nonblocking(true)
        .expect("a non-blocking listener");
    let port = listener.local_addr().expect("a bound address").port();
    (listener, port)
}

pub(crate) fn assert_untouched(listener: &TcpListener) {
    match listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
        Err(error) => panic!("the listener failed: {error}"),
        Ok((_, peer)) => {
            panic!("{peer} connected to a listener the guard should have kept it from")
        }
    }
}

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
