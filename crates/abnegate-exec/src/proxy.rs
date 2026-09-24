//! Optional proxy routing for HTTP clients launched by tools.

use std::env;
use std::ffi::OsString;
use std::fmt;

use tokio::process::Command;

/// Environment variable naming the proxy every spawned command is routed
/// through. Absent or empty leaves each command's own environment alone.
pub const PROXY_URL_VARIABLE: &str = "ABNEGATE_EXEC_PROXY_URL";

/// Environment variable listing, comma-separated, the hosts a routed command
/// reaches directly rather than through the proxy. Absent means
/// [`DEFAULT_BYPASS`]; set but empty means every host goes through the proxy.
pub const PROXY_BYPASS_VARIABLE: &str = "ABNEGATE_EXEC_PROXY_BYPASS";

/// The hosts a routed command reaches directly when no bypass list is given:
/// loopback, and nothing else.
pub const DEFAULT_BYPASS: &[&str] = &["localhost", "127.0.0.1", "::1"];

const PROXY_VARIABLES: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];
const BYPASS_VARIABLES: [&str; 2] = ["NO_PROXY", "no_proxy"];
const BYPASS_SEPARATOR: &str = ",";

/// Process-level routing policy applied after per-command environment overlays.
///
/// Clients must support standard proxy environment variables. This does not
/// constrain raw sockets or clients that explicitly disable proxy support.
/// `Debug` never prints the URL, which can carry the proxy's credentials.
#[derive(Clone, Default)]
pub struct Proxy {
    url: Option<OsString>,
    bypass: Vec<String>,
}

impl Proxy {
    /// Route through `url`, reaching only [`DEFAULT_BYPASS`] directly.
    pub fn new(url: impl Into<OsString>) -> Self {
        Self {
            url: Some(url.into()),
            bypass: DEFAULT_BYPASS.iter().map(|host| host.to_string()).collect(),
        }
    }

    /// Reach exactly `hosts` directly instead of [`DEFAULT_BYPASS`]. Each is
    /// a host name, an address, or a `.domain` suffix, as `NO_PROXY` takes.
    pub fn with_bypass<I, S>(mut self, hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.bypass = hosts.into_iter().map(Into::into).collect();
        self
    }

    /// Read [`PROXY_URL_VARIABLE`] and [`PROXY_BYPASS_VARIABLE`]. An absent or
    /// empty URL preserves the command's existing environment.
    pub fn from_environment() -> Self {
        Self::from_settings(
            env::var_os(PROXY_URL_VARIABLE),
            env::var(PROXY_BYPASS_VARIABLE).ok(),
        )
    }

    fn from_settings(url: Option<OsString>, bypass: Option<String>) -> Self {
        let Some(url) = url.filter(|url| !url.is_empty()) else {
            return Self::default();
        };
        let proxy = Self::new(url);
        match bypass {
            Some(hosts) => proxy.with_bypass(
                hosts
                    .split(BYPASS_SEPARATOR)
                    .map(str::trim)
                    .filter(|host| !host.is_empty()),
            ),
            None => proxy,
        }
    }

    /// Apply routing last so a tool's environment cannot accidentally bypass it.
    ///
    /// The command also receives [`PROXY_URL_VARIABLE`] and
    /// [`PROXY_BYPASS_VARIABLE`], so an executor it starts routes the same
    /// way.
    pub fn apply(&self, command: &mut Command) {
        let Some(url) = &self.url else {
            return;
        };
        for name in PROXY_VARIABLES.into_iter().chain([PROXY_URL_VARIABLE]) {
            command.env(name, url);
        }
        let bypass = self.bypass.join(BYPASS_SEPARATOR);
        for name in BYPASS_VARIABLES.into_iter().chain([PROXY_BYPASS_VARIABLE]) {
            command.env(name, &bypass);
        }
    }
}

impl fmt::Debug for Proxy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Proxy")
            .field("url", &self.url.as_ref().map(|_| "<redacted>"))
            .field("bypass", &self.bypass)
            .finish()
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
            let proxy = Proxy::new(format!("http://{}", listener.local_addr().unwrap()));
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
            let proxy = Proxy::new(format!("http://{}", unavailable.local_addr().unwrap()));
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

    async fn reaches(policy: &Proxy, hostname: &str) -> bool {
        let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = destination.local_addr().unwrap().port();
        let server = tokio::spawn(async move { response(destination, "200 OK").await });
        let output = curl(policy, &format!("http://{hostname}:{port}/check"))
            .args(["--resolve", &format!("{hostname}:{port}:127.0.0.1")])
            .output()
            .await
            .unwrap();
        let reached = output.status.success();
        if reached {
            assert_eq!(server.await.unwrap(), "GET /check HTTP/1.1\r\n");
        } else {
            server.abort();
        }
        reached
    }

    async fn unused_proxy() -> (TcpListener, Proxy) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = Proxy::new(format!("http://{}", listener.local_addr().unwrap()));
        (listener, proxy)
    }

    #[tokio::test]
    async fn loopback_bypasses_proxy() {
        for hostname in ["127.0.0.1", "localhost"] {
            let (listener, proxy) = unused_proxy().await;

            assert!(reaches(&proxy, hostname).await, "{hostname}");
            assert!(
                timeout(Duration::from_millis(50), listener.accept())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn only_loopback_bypasses_by_default() {
        let (listener, proxy) = unused_proxy().await;
        let proxied = tokio::spawn(async move { response(listener, "200 OK").await });

        let output = curl(&proxy, "http://ollama:11434/check")
            .output()
            .await
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            proxied.await.unwrap(),
            "GET http://ollama:11434/check HTTP/1.1\r\n",
            "a host outside the default bypass must go through the proxy"
        );
    }

    #[tokio::test]
    async fn a_configured_bypass_replaces_the_default() {
        let (listener, proxy) = unused_proxy().await;
        let proxy = proxy.with_bypass(["ollama"]);

        assert!(reaches(&proxy, "ollama").await);
        assert!(
            timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    }

    #[test]
    fn the_bypass_setting_is_a_trimmed_comma_separated_list() {
        let proxy = Proxy::from_settings(
            Some("http://proxy:3128".into()),
            Some(" ollama, .svc ,,127.0.0.1 ".to_string()),
        );

        assert_eq!(proxy.bypass, ["ollama", ".svc", "127.0.0.1"]);
    }

    #[test]
    fn an_absent_bypass_setting_means_loopback_only() {
        let proxy = Proxy::from_settings(Some("http://proxy:3128".into()), None);

        assert_eq!(proxy.bypass, DEFAULT_BYPASS);
    }

    #[test]
    fn an_empty_bypass_setting_sends_everything_through_the_proxy() {
        let proxy = Proxy::from_settings(Some("http://proxy:3128".into()), Some(String::new()));

        assert!(proxy.bypass.is_empty());
    }

    #[test]
    fn debug_never_prints_the_url() {
        let debug = format!("{:?}", Proxy::new("http://user:hunter2@proxy:3128"));

        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
    }

    #[test]
    fn an_absent_or_empty_url_routes_nothing() {
        for url in [None, Some(OsString::new())] {
            let proxy = Proxy::from_settings(url, Some("ollama".to_string()));

            assert!(proxy.url.is_none());
        }
    }

    #[tokio::test]
    async fn a_routed_command_learns_the_same_routing() {
        let mut command = Command::new("env");
        command.env_clear();
        Proxy::new("http://proxy:3128")
            .with_bypass(["ollama", ".svc"])
            .apply(&mut command);

        let output = String::from_utf8(command.output().await.unwrap().stdout).unwrap();

        for line in [
            "ABNEGATE_EXEC_PROXY_URL=http://proxy:3128",
            "ABNEGATE_EXEC_PROXY_BYPASS=ollama,.svc",
            "NO_PROXY=ollama,.svc",
            "no_proxy=ollama,.svc",
        ] {
            assert!(
                output.lines().any(|candidate| candidate == line),
                "{output}"
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
