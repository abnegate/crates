use crate::error::Error;
use crate::error::Result;
use crate::metadata::MetadataResolver;
use crate::public::client::PublicClient;
use crate::public::resolver::PublicResolver;
use crate::public::target_policy::TargetPolicy;
use reqwest::Client;
use reqwest::ClientBuilder;
use reqwest::redirect::Policy;
use std::time::Duration;

/// Redirect hops a caller-supplied fetch may follow. Each one is validated, so
/// this bounds the chain rather than the trust.
const MAXIMUM_REDIRECTS: usize = 3;

/// Configures a [`PublicClient`].
///
/// The guard is applied when the client is built, after any configuration,
/// so [`PublicClientBuilder::configure`] cannot replace the resolver or the
/// redirect policy. A proxy configured explicitly is honoured; it resolves the
/// target itself, so only the textual checks apply to the target then.
/// Pinning a name to an address with [`ClientBuilder::resolve`] bypasses the
/// resolver, so do not.
///
/// The [`TargetPolicy`] decides which targets the guard refuses:
/// [`TargetPolicy::Public`] unless
/// [`PublicClientBuilder::with_target_policy`] names another.
#[derive(Debug)]
pub struct PublicClientBuilder {
    builder: ClientBuilder,
    policy: TargetPolicy,
}

impl PublicClientBuilder {
    pub(crate) fn new(timeout: Duration) -> Self {
        Self {
            builder: Client::builder().timeout(timeout).no_proxy(),
            policy: TargetPolicy::default(),
        }
    }

    /// Refuse the targets `policy` names, by URL, by resolved address and on
    /// every redirect hop, in place of [`TargetPolicy::Public`].
    pub fn with_target_policy(self, policy: TargetPolicy) -> Self {
        Self { policy, ..self }
    }

    /// Adjust the underlying reqwest builder: a user agent, default headers,
    /// TLS settings or an explicit proxy.
    pub fn configure(self, configure: impl FnOnce(ClientBuilder) -> ClientBuilder) -> Self {
        Self {
            builder: configure(self.builder),
            ..self
        }
    }

    /// Build the client, with the resolver and redirect guards applied last.
    pub fn build(self) -> Result<PublicClient> {
        let policy = self.policy;
        let builder = match policy {
            TargetPolicy::Public => self.builder.dns_resolver(PublicResolver),
            TargetPolicy::ExceptMetadata => self.builder.dns_resolver(MetadataResolver::new()),
        };
        let client = builder
            .redirect(Policy::custom(move |attempt| {
                if attempt.previous().len() > MAXIMUM_REDIRECTS {
                    return attempt.error(Error::TooManyRedirects);
                }
                match policy.validate(attempt.url().as_str()) {
                    Ok(_) => attempt.follow(),
                    Err(error) => attempt.error(error),
                }
            }))
            .build()?;
        Ok(PublicClient::new(client, policy))
    }
}

#[cfg(test)]
mod tests {
    use crate::error::Error;
    use crate::public::PublicClient;
    use crate::public::public_client;
    use crate::public::public_client_builder;
    use crate::public::target_policy::TargetPolicy;
    use crate::test_support::LOOPBACK_SPELLINGS;
    use crate::test_support::assert_untouched;
    use crate::test_support::loopback_listener;
    use crate::test_support::serve_once;
    use reqwest::Method;
    use reqwest::Proxy;
    use reqwest::Request;
    use reqwest::Url;
    use reqwest::redirect::Policy;
    use std::error::Error as _;
    use std::time::Duration;
    use tokio::process::Command;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;

    const TIMEOUT: Duration = Duration::from_secs(2);
    const CHILD: &str = "ABNEGATE_HTTP_TEST_CHILD";
    const PROXY_VARIABLES: [&str; 6] = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    const PROXY_EXCLUSIONS: [&str; 3] = ["NO_PROXY", "no_proxy", "REQUEST_METHOD"];
    const METADATA_URLS: [&str; 9] = [
        "http://169.254.169.254/latest",
        "http://169.254.170.2/v1",
        "http://[fe80::1]:8080/v1",
        "http://[fd00:ec2::254]/v1",
        "http://[::ffff:169.254.169.254]/v1",
        "http://[fd20:ce::254]/computeMetadata/v1",
        "http://168.63.129.16/machine",
        "http://metadata.google.internal/computeMetadata/v1",
        "http://METADATA.GOOGLE.INTERNAL./v1",
    ];

    fn except_metadata() -> PublicClient {
        public_client_builder(TIMEOUT)
            .with_target_policy(TargetPolicy::ExceptMetadata)
            .build()
            .expect("a guarded client")
    }

    async fn answering(response: ResponseTemplate, expected: u64) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(response)
            .expect(expected)
            .mount(&server)
            .await;
        server
    }

    /// With a proxy in the environment, a proxied request only ever resolves
    /// the proxy's own name, so the guard never sees the target's address.
    /// The child process carries the proxy variables so this test does not
    /// have to mutate its own environment.
    #[tokio::test]
    async fn an_environment_proxy_is_never_consulted() {
        environment_proxy_is_never_consulted(
            "public::builder::tests::an_environment_proxy_is_never_consulted",
            TargetPolicy::Public,
        )
        .await;
    }

    #[tokio::test]
    async fn an_environment_proxy_is_never_consulted_except_metadata() {
        environment_proxy_is_never_consulted(
            "public::builder::tests::an_environment_proxy_is_never_consulted_except_metadata",
            TargetPolicy::ExceptMetadata,
        )
        .await;
    }

    async fn environment_proxy_is_never_consulted(name: &str, policy: TargetPolicy) {
        if std::env::var(CHILD).as_deref() == Ok(name) {
            let client = public_client_builder(TIMEOUT)
                .with_target_policy(policy)
                .build()
                .expect("a guarded client");
            let request = client
                .get("http://proxy-probe.invalid/")
                .expect("a public name passes the URL check");
            let _ = request.send().await;
            return;
        }

        let (listener, port) = loopback_listener();
        let proxy = format!("http://127.0.0.1:{port}");
        let mut command = Command::new(std::env::current_exe().expect("the test binary"));
        command
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, name);
        for variable in PROXY_VARIABLES {
            command.env(variable, &proxy);
        }
        for variable in PROXY_EXCLUSIONS {
            command.env_remove(variable);
        }

        let output = command.output().await.expect("the child runs");
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_untouched(&listener);
    }

    #[tokio::test]
    async fn an_explicit_proxy_still_has_its_target_checked() {
        let (proxy_listener, proxy_port) = loopback_listener();
        let proxy = Proxy::all(format!("http://127.0.0.1:{proxy_port}")).expect("a proxy URL");
        let client = public_client_builder(TIMEOUT)
            .configure(|builder| builder.proxy(proxy))
            .build()
            .expect("a guarded client");

        for spelling in LOOPBACK_SPELLINGS {
            let error = client
                .get(&format!("http://{spelling}:9/"))
                .expect_err("loopback must not be fetched through a proxy either");
            assert!(matches!(error, Error::PrivateAddress), "{error}");
        }
        assert_untouched(&proxy_listener);
    }

    /// The proxy stands in for a public origin, which a test cannot otherwise
    /// reach: it answers the first request with a redirect into loopback,
    /// which it does not proxy, and the redirect policy the caller configured
    /// must not be the one applied.
    #[tokio::test]
    async fn a_redirect_into_the_deployment_is_refused_whatever_policy_was_configured() {
        const ORIGIN: &str = "public.example";
        let (target, target_port) = loopback_listener();
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:{target_port}/secret\r\ncontent-length: 0\r\n\r\n"
        );
        let proxy = format!("http://127.0.0.1:{}", serve_once(redirect).await);
        let proxy =
            Proxy::custom(move |url| (url.host_str() == Some(ORIGIN)).then(|| proxy.clone()));
        let client = public_client_builder(TIMEOUT)
            .configure(|builder| builder.proxy(proxy).redirect(Policy::limited(10)))
            .build()
            .expect("a guarded client");

        let error = client
            .get(&format!("http://{ORIGIN}/start?key=hunter2"))
            .expect("a public name passes the URL check")
            .send()
            .await
            .expect_err("the redirect leads into loopback");

        assert_untouched(&target);
        let Error::Request(ref inner) = error else {
            panic!("expected a transport error, got {error}");
        };
        assert!(inner.is_redirect(), "{error}");
        assert!(
            matches!(
                inner
                    .source()
                    .and_then(|source| source.downcast_ref::<Error>()),
                Some(Error::PrivateAddress)
            ),
            "{error:?}"
        );
        assert!(!format!("{error:?}").contains("hunter2"), "{error:?}");
    }

    #[tokio::test]
    async fn a_metadata_target_is_refused_before_a_connection_is_made() {
        let client = except_metadata();

        for url in METADATA_URLS {
            let refused = client.post(url).map(|_| ());
            assert!(
                matches!(refused, Err(Error::MetadataTarget)),
                "{url}: {refused:?}"
            );

            let request = Request::new(Method::POST, Url::parse(url).expect("a URL"));
            let refused = client.execute(request).await.map(|_| ());
            assert!(
                matches!(refused, Err(Error::MetadataTarget)),
                "{url}: {refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_explicit_proxy_still_has_a_metadata_target_checked() {
        let (proxy_listener, proxy_port) = loopback_listener();
        let proxy = Proxy::all(format!("http://127.0.0.1:{proxy_port}")).expect("a proxy URL");
        let client = public_client_builder(TIMEOUT)
            .with_target_policy(TargetPolicy::ExceptMetadata)
            .configure(|builder| builder.proxy(proxy))
            .build()
            .expect("a guarded client");

        for url in METADATA_URLS {
            let refused = client.get(url).map(|_| ());
            assert!(
                matches!(refused, Err(Error::MetadataTarget)),
                "{url}: {refused:?}"
            );
        }
        assert_untouched(&proxy_listener);
    }

    #[tokio::test]
    async fn a_redirect_to_a_metadata_address_is_refused() {
        let server = answering(
            ResponseTemplate::new(307)
                .insert_header("location", "http://169.254.169.254/latest/meta-data/"),
            1,
        )
        .await;

        let error = except_metadata()
            .post(&server.uri())
            .expect("loopback passes the URL check")
            .send()
            .await
            .expect_err("the redirect leads to a metadata address");

        let Error::Request(ref inner) = error else {
            panic!("expected a transport error, got {error}");
        };
        assert!(inner.is_redirect(), "{error}");
        assert!(
            matches!(
                inner
                    .source()
                    .and_then(|source| source.downcast_ref::<Error>()),
                Some(Error::MetadataTarget)
            ),
            "{error:?}"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn loopback_stays_reachable_except_metadata() {
        let server = answering(ResponseTemplate::new(200), 2).await;
        let localhost = server.uri().replace("127.0.0.1", "localhost");

        for url in [server.uri(), localhost] {
            assert!(
                matches!(
                    public_client(TIMEOUT).expect("a guarded client").get(&url),
                    Err(Error::PrivateAddress | Error::InternalHost)
                ),
                "the public policy reached {url}"
            );

            let answered = except_metadata()
                .get(&url)
                .expect("loopback passes the URL check")
                .send()
                .await;

            assert!(
                answered.is_ok_and(|response| response.status().is_success()),
                "{url}"
            );
        }
        server.verify().await;
    }

    #[tokio::test]
    async fn a_redirect_inside_the_deployment_is_followed_except_metadata() {
        let elsewhere = answering(ResponseTemplate::new(200), 1).await;
        let server = answering(
            ResponseTemplate::new(307).insert_header("location", format!("{}/v1", elsewhere.uri())),
            1,
        )
        .await;

        let answered = except_metadata()
            .get(&server.uri())
            .expect("loopback passes the URL check")
            .send()
            .await;

        assert!(
            answered.is_ok_and(|response| response.status().is_success()),
            "the redirect was not followed"
        );
        server.verify().await;
        elsewhere.verify().await;
    }

    #[test]
    fn a_configuration_reqwest_refuses_is_an_error_rather_than_a_panic() {
        let error = public_client_builder(TIMEOUT)
            .configure(|builder| builder.user_agent("line\nbreak"))
            .build()
            .expect_err("a header value cannot hold a line break");

        assert!(matches!(error, Error::Request(_)), "{error}");
    }
}
