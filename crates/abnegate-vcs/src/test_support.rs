use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// A stand-in server on a loopback port that answers each request with what
/// `answer` gives for its request line, and then hangs up; an empty answer
/// hangs up without one. Its base URL is returned, `http://` and the address
/// it listens on.
pub(crate) async fn hanging_up(answer: fn(&str) -> &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port to listen on");
    let address = listener
        .local_addr()
        .expect("the address the listener is bound to");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).await.unwrap_or_default();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let head = String::from_utf8_lossy(&request);
            let line = head.lines().next().unwrap_or_default().to_string();
            let _ = stream.write_all(answer(&line).as_bytes()).await;
        }
    });
    format!("http://{address}")
}
