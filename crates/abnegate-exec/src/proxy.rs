//! Optional proxy routing for HTTP clients launched by tools.

use std::ffi::OsString;

use tokio::process::Command;

/// Environment variable naming the proxy every spawned command is routed
/// through. Absent or empty leaves each command's own environment alone.
pub const PROXY_URL_ENV: &str = "ABNEGATE_EXEC_PROXY_URL";

/// Loopback and co-located service names stay directly reachable rather than
/// being sent to the proxy.
const BYPASS: &str = "localhost,127.0.0.1,::1,host.docker.internal,gateway.docker.internal,gluetun,searxng,manager,console,litellm,ollama,comfyui,postgres,valkey,traefik,prometheus,grafana,.svc,.svc.cluster.local";

/// Process-level routing policy applied after per-command environment overlays.
///
/// Clients must support standard proxy environment variables. This does not
/// constrain raw sockets or clients that explicitly disable proxy support.
#[derive(Clone, Default)]
pub struct Proxy {
    url: Option<OsString>,
}

impl Proxy {
    /// An absent or empty setting preserves the command's existing environment.
    pub fn from_env() -> Self {
        Self {
            url: std::env::var_os(PROXY_URL_ENV).filter(|url| !url.is_empty()),
        }
    }

    /// Apply routing last so a tool's environment cannot accidentally bypass it.
    pub fn apply(&self, command: &mut Command) {
        let Some(url) = &self.url else {
            return;
        };
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            PROXY_URL_ENV,
        ] {
            command.env(name, url);
        }
        command.env("NO_PROXY", BYPASS).env("no_proxy", BYPASS);
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;
    use tokio::net::TcpListener;
    use tokio::time::Duration;
    use tokio::time::timeout;

    use super::*;

    fn curl(proxy: &Proxy, url: &str) -> Command {
        let mut command = Command::new("curl");
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .args(["--silent", "--show-error", "--fail", "--max-time", "3", url]);
        proxy.apply(&mut command);
        command
    }

    async fn response(listener: TcpListener, status: &str) -> String {
        let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut stream = BufReader::new(stream);
        let mut request = String::new();
        stream.read_line(&mut request).await.unwrap();
        loop {
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        stream
            .write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Length: 6\r\nConnection: close\r\n\r\nrouted"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        request
    }

    #[tokio::test]
    async fn curl_uses_proxy_for_http_and_https() {
        for (url, status, request, success) in [
            (
                "http://routing.invalid/check",
                "200 OK",
                "GET http://routing.invalid/check HTTP/1.1\r\n",
                true,
            ),
            (
                "https://routing.invalid/check",
                "403 Forbidden",
                "CONNECT routing.invalid:443 HTTP/1.1\r\n",
                false,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy = Proxy {
                url: Some(format!("http://{}", listener.local_addr().unwrap()).into()),
            };
            let server = tokio::spawn(async move { response(listener, status).await });
            let output = curl(&proxy, url).output().await.unwrap();
            assert_eq!(
                output.status.success(),
                success,
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(server.await.unwrap(), request);
            if success {
                assert_eq!(output.stdout, b"routed");
            }
        }
    }

    #[tokio::test]
    async fn unavailable_proxy_does_not_retry_destination_directly() {
        for scheme in ["http", "https"] {
            let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let unavailable = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy = Proxy {
                url: Some(format!("http://{}", unavailable.local_addr().unwrap()).into()),
            };
            drop(unavailable);
            let port = destination.local_addr().unwrap().port();
            let output = curl(&proxy, &format!("{scheme}://routing.invalid:{port}/check"))
                .args(["--resolve", &format!("routing.invalid:{port}:127.0.0.1")])
                .output()
                .await
                .unwrap();
            assert!(!output.status.success());
            assert_eq!(
                output.status.code(),
                Some(7),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                timeout(Duration::from_millis(100), destination.accept())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn loopback_and_stack_services_bypass_proxy() {
        for hostname in [
            "127.0.0.1",
            "localhost",
            "host.docker.internal",
            "gateway.docker.internal",
            "gluetun",
            "ollama",
            "litellm",
            "manager",
        ] {
            let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = destination.local_addr().unwrap().port();
            let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let policy = Proxy {
                url: Some(format!("http://{}", proxy.local_addr().unwrap()).into()),
            };
            let server = tokio::spawn(async move { response(destination, "200 OK").await });
            let output = curl(&policy, &format!("http://{hostname}:{port}/check"))
                .args(["--resolve", &format!("{hostname}:{port}:127.0.0.1")])
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(server.await.unwrap(), "GET /check HTTP/1.1\r\n");
            assert!(
                timeout(Duration::from_millis(50), proxy.accept())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn unconfigured_proxy_preserves_direct_requests_and_environment() {
        let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = destination.local_addr().unwrap().port();
        let server = tokio::spawn(async move { response(destination, "200 OK").await });
        let output = curl(
            &Proxy::default(),
            &format!("http://routing.invalid:{port}/check"),
        )
        .args(["--resolve", &format!("routing.invalid:{port}:127.0.0.1")])
        .output()
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(server.await.unwrap(), "GET /check HTTP/1.1\r\n");

        let mut command = Command::new("env");
        command.env_clear().env("https_proxy", "http://custom:8888");
        Proxy::default().apply(&mut command);
        let output = command.output().await.unwrap();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "https_proxy=http://custom:8888\n"
        );
    }
}
