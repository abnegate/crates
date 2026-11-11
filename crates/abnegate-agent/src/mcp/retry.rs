use std::cell::Cell;
use std::error::Error;

use h2::Reason;
use reqwest::StatusCode;
use reqwest::retry::Builder;

tokio::task_local! {
    /// The status the request being [`answered`] was last answered with,
    /// if its last attempt was answered at all.
    static ANSWER: Cell<Option<StatusCode>>;
}

/// The host of a retry scope that every request's host matches, so a retry
/// is held to no host, as the HTTP client's own default holds it to none.
struct Anywhere;

impl PartialEq<&str> for Anywhere {
    fn eq(&self, _: &&str) -> bool {
        true
    }
}

/// The retry policy of a remote server's client: the retries the HTTP
/// client makes when it is given no policy, and no others, with each
/// attempt's status noted for [`answered`].
///
/// A client given any policy loses its default one, which cannot be asked
/// for by name, so this repeats it: at most two retries of a request, held
/// to no budget and no host, each only after an attempt the server
/// [`refused`] before acting on it.
pub(super) fn retries() -> Builder {
    reqwest::retry::for_host(Anywhere)
        .no_budget()
        .classify_fn(|attempt| {
            let _ = ANSWER.try_with(|answer| answer.set(attempt.status()));
            if refused(attempt.error()) {
                attempt.retryable()
            } else {
                attempt.success()
            }
        })
}

/// What `work` returns, with the status its request was last answered with
/// when a client retrying as [`retries`] says sent it from within `work`.
///
/// Every attempt is noted, a retry's and a followed redirect's included,
/// and one that got no answer forgets the answer before it, so the status
/// is that of the answer the request ended with. An attempt past the last
/// retry is not noted, and neither is a request sent from anywhere else.
pub(super) async fn answered<T>(work: impl Future<Output = T>) -> (T, Option<StatusCode>) {
    ANSWER
        .scope(Cell::new(None), async {
            let output = work.await;
            (output, ANSWER.with(Cell::get))
        })
        .await
}

/// Whether `error` is an HTTP/2 server refusing a request before acting on
/// it, which makes the request safe to send again: a graceful `GOAWAY`, or
/// a stream reset with `REFUSED_STREAM`.
///
/// This repeats the HTTP/2 test `reqwest` 0.13.5 applies by default, which
/// it keeps private as `retry::is_retryable_error`: the HTTP/2 error is
/// looked for exactly three sources below the request's own error, under
/// the connection pool's error and the protocol's, and nowhere else. It
/// matches only while this crate's `h2` is the copy `reqwest` links. The
/// tests that run each case under both `reqwest`'s default policy and
/// [`retries`] guard it, so re-run them on every `reqwest`, `hyper` or `h2`
/// bump.
fn refused(error: Option<&(dyn Error + 'static)>) -> bool {
    error
        .and_then(|request| request.source())
        .and_then(|pool| pool.source())
        .and_then(|protocol| protocol.source())
        .and_then(|cause| cause.downcast_ref::<h2::Error>())
        .is_some_and(|cause| {
            let leaving = cause.is_go_away() && cause.reason() == Some(Reason::NO_ERROR);
            let refusing = cause.is_reset() && cause.reason() == Some(Reason::REFUSED_STREAM);
            cause.is_remote() && (leaving || refusing)
        })
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::io;
    use std::net::Ipv4Addr;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use reqwest::header::HeaderValue;
    use reqwest::header::LOCATION;
    use serde_json::json;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use tokio::net::TcpStream;
    use tokio::task::JoinSet;

    use super::*;
    use crate::mcp::fixture::MCP_PATH;

    /// The policies that must retry alike: the HTTP client's own, and the
    /// one that stands in for it.
    const ALIKE: [Policy; 2] = [Policy::Default, Policy::Replica];

    /// More requests refused every time than a retry budget lets one client
    /// retry twice: the budget a `reqwest` policy has unless it is told
    /// `no_budget` takes five from a reserve of 500 for each retry, and a
    /// refused attempt puts nothing back, so it allows 100 of these 120.
    const REFUSALS: usize = 60;

    const PREFACE: usize = 24;

    /// The length of a frame's head: its payload's length in three bytes,
    /// its type, its flags, and its stream in four.
    const HEAD: usize = 9;

    /// Where a frame's head holds the last byte of its payload's length.
    const LENGTH: usize = 2;

    /// Where a frame's head holds its type.
    const KIND: usize = 3;

    /// An empty `SETTINGS` frame, which a server opens a connection with.
    const SETTINGS: [u8; HEAD] = [0, 0, 0, 4, 0, 0, 0, 0, 0];

    const HEADERS: u8 = 1;

    const GOAWAY: u8 = 7;

    /// The length of a `GOAWAY` frame's payload: the last stream the
    /// server acted on, and why it is leaving, in four bytes each.
    const FAREWELL: u8 = 8;

    /// Where a `GOAWAY` frame holds why the server is leaving, past the
    /// last stream it acted on.
    const REASON: usize = HEAD + 4;

    /// A `GOAWAY` frame for `reason` that names no stream, so the server
    /// acted on none.
    fn departure(reason: Reason) -> [u8; HEAD + FAREWELL as usize] {
        let mut frame = [0; HEAD + FAREWELL as usize];
        frame[LENGTH] = FAREWELL;
        frame[KIND] = GOAWAY;
        frame[REASON..].copy_from_slice(&u32::from(reason).to_be_bytes());
        frame
    }

    #[derive(Debug, Clone, Copy)]
    enum Policy {
        Default,
        Replica,
        /// Shows a retry is the policy's doing.
        Never,
    }

    /// What the server does with a stream once it has read its request.
    #[derive(Clone, Copy)]
    enum Reply {
        Reset(Reason),
        /// An answer of this status, which points back at [`MCP_PATH`]
        /// when it is a redirect.
        Answer(StatusCode),
    }

    /// Chooses the reply to each stream by how many came before it, over
    /// every connection.
    type Script = fn(usize) -> Reply;

    /// A local server speaking HTTP/2 in cleartext, and what it has seen.
    struct Served {
        url: String,
        connections: Arc<AtomicUsize>,
        streams: Arc<AtomicUsize>,
    }

    impl Served {
        fn connections(&self) -> usize {
            self.connections.load(Ordering::SeqCst)
        }

        fn streams(&self) -> usize {
            self.streams.load(Ordering::SeqCst)
        }
    }

    /// A server that leaves its first connection with a `GOAWAY` for
    /// `departing`, when there is one, and replies to every stream of any
    /// other connection as `script` chooses.
    async fn serving(departing: Option<Reason>, script: Script) -> Served {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("a listener");
        let address = listener.local_addr().expect("an address");
        let connections = Arc::new(AtomicUsize::new(0));
        let streams = Arc::new(AtomicUsize::new(0));
        let (accepted, opened) = (connections.clone(), streams.clone());
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let first = accepted.fetch_add(1, Ordering::SeqCst) == 0;
                match departing.filter(|_| first) {
                    Some(reason) => {
                        tokio::spawn(depart(socket, reason));
                    }
                    None => {
                        tokio::spawn(reply(socket, script, opened.clone()));
                    }
                }
            }
        });
        Served {
            url: format!("http://{address}{MCP_PATH}"),
            connections,
            streams,
        }
    }

    /// `send` given a default response that `edit` has changed. The
    /// response's type is one this crate cannot name, so it is taken from
    /// what `send` accepts.
    fn shaped<R: Default, T>(send: impl FnOnce(R) -> T, edit: impl FnOnce(&mut R)) -> T {
        let mut response = R::default();
        edit(&mut response);
        send(response)
    }

    /// Reply to every stream of `socket` as `script` chooses, once the
    /// stream's request has been read to its end.
    async fn reply(socket: TcpStream, script: Script, streams: Arc<AtomicUsize>) {
        let Ok(mut connection) = h2::server::handshake(socket).await else {
            return;
        };
        while let Some(Ok((mut request, mut respond))) = connection.accept().await {
            let reply = script(streams.fetch_add(1, Ordering::SeqCst));
            tokio::spawn(async move {
                while request.body_mut().data().await.is_some() {}
                let status = match reply {
                    Reply::Reset(reason) => return respond.send_reset(reason),
                    Reply::Answer(status) => status,
                };
                shaped(
                    |response| respond.send_response(response, true),
                    |response| {
                        *response.status_mut() = status;
                        if status.is_redirection() {
                            response
                                .headers_mut()
                                .insert(LOCATION, HeaderValue::from_static(MCP_PATH));
                        }
                    },
                )
                .expect("an answer is sent");
            });
        }
    }

    /// Leave `socket` with a `GOAWAY` for `reason` once its client has
    /// opened a stream, so that stream is one the server says it never
    /// acted on, then read until the client hangs up.
    async fn depart(mut socket: TcpStream, reason: Reason) -> io::Result<()> {
        socket.write_all(&SETTINGS).await?;
        socket.read_exact(&mut [0; PREFACE]).await?;
        loop {
            let mut head = [0; HEAD];
            socket.read_exact(&mut head).await?;
            let [high, middle, low, kind, ..] = head;
            let length = u32::from_be_bytes([0, high, middle, low]);
            tokio::io::copy(
                &mut (&mut socket).take(u64::from(length)),
                &mut tokio::io::sink(),
            )
            .await?;
            if kind == HEADERS {
                break;
            }
        }
        socket.write_all(&departure(reason)).await?;
        tokio::io::copy(&mut socket, &mut tokio::io::sink()).await?;
        Ok(())
    }

    fn client(policy: Policy) -> reqwest::Client {
        let builder = reqwest::Client::builder()
            .no_proxy()
            .http2_prior_knowledge();
        let builder = match policy {
            Policy::Default => builder,
            Policy::Replica => builder.retry(retries()),
            Policy::Never => builder.retry(reqwest::retry::never()),
        };
        builder.build().expect("a client")
    }

    /// The status `server` answers a JSON request posted under `policy`.
    async fn post(policy: Policy, server: &Served) -> reqwest::Result<StatusCode> {
        client(policy)
            .post(&server.url)
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .send()
            .await
            .map(|response| response.status())
    }

    fn refused_once(stream: usize) -> Reply {
        match stream {
            0 => Reply::Reset(Reason::REFUSED_STREAM),
            _ => Reply::Answer(StatusCode::OK),
        }
    }

    fn accepting(_: usize) -> Reply {
        Reply::Answer(StatusCode::OK)
    }

    fn unavailable(_: usize) -> Reply {
        Reply::Answer(StatusCode::SERVICE_UNAVAILABLE)
    }

    /// An error with `source` beneath it.
    #[derive(Debug)]
    struct Wrapped(Box<dyn Error + 'static>);

    impl fmt::Display for Wrapped {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("wrapped")
        }
    }

    impl Error for Wrapped {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(self.0.as_ref())
        }
    }

    #[test]
    fn every_host_is_within_the_scope() {
        for host in ["", "localhost", "mcp.example.com"] {
            assert!(Anywhere == host, "{host}");
        }
    }

    #[tokio::test]
    async fn a_refused_stream_is_sent_again_as_the_default_sends_it() {
        for policy in ALIKE {
            let server = serving(None, refused_once).await;

            let status = post(policy, &server).await;

            assert_eq!(
                status.expect("the second stream is answered"),
                StatusCode::OK,
                "{policy:?}"
            );
            assert_eq!(server.streams(), 2, "{policy:?}");
        }
    }

    #[tokio::test]
    async fn a_refused_stream_fails_a_client_that_never_retries() {
        let server = serving(None, refused_once).await;

        let status = post(Policy::Never, &server).await;

        assert!(status.is_err(), "{status:?}");
        assert_eq!(server.streams(), 1);
    }

    #[tokio::test]
    async fn a_request_a_departing_server_never_acted_on_is_sent_again_as_the_default_sends_it() {
        for policy in ALIKE {
            let server = serving(Some(Reason::NO_ERROR), accepting).await;

            let status = post(policy, &server).await;

            assert_eq!(
                status.expect("another connection answers"),
                StatusCode::OK,
                "{policy:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_departing_server_fails_a_client_that_never_retries() {
        let server = serving(Some(Reason::NO_ERROR), accepting).await;

        let status = post(Policy::Never, &server).await;

        assert!(status.is_err(), "{status:?}");
        assert_eq!(server.streams(), 0);
    }

    #[tokio::test]
    async fn a_server_departing_over_an_error_is_not_sent_the_request_again() {
        for policy in ALIKE {
            let server = serving(Some(Reason::INTERNAL_ERROR), accepting).await;

            let status = post(policy, &server).await;

            assert!(status.is_err(), "{policy:?}: {status:?}");
            assert_eq!(server.connections(), 1, "{policy:?}");
            assert_eq!(server.streams(), 0, "{policy:?}");
        }
    }

    #[tokio::test]
    async fn a_stream_reset_over_an_error_is_not_sent_again() {
        for policy in ALIKE {
            let server = serving(None, |_| Reply::Reset(Reason::INTERNAL_ERROR)).await;

            let status = post(policy, &server).await;

            assert!(status.is_err(), "{policy:?}: {status:?}");
            assert_eq!(server.streams(), 1, "{policy:?}");
        }
    }

    #[tokio::test]
    async fn a_failing_status_is_an_answer_and_is_not_sent_again() {
        for policy in ALIKE {
            let server = serving(None, unavailable).await;

            let status = post(policy, &server).await;

            assert_eq!(
                status.expect("an answer"),
                StatusCode::SERVICE_UNAVAILABLE,
                "{policy:?}"
            );
            assert_eq!(server.streams(), 1, "{policy:?}");
        }
    }

    #[tokio::test]
    async fn a_stream_refused_every_time_is_sent_twice_more_and_no_further() {
        for policy in ALIKE {
            let server = serving(None, |_| Reply::Reset(Reason::REFUSED_STREAM)).await;

            let status = post(policy, &server).await;

            assert!(status.is_err(), "{policy:?}: {status:?}");
            assert_eq!(server.streams(), 3, "{policy:?}");
        }
    }

    #[tokio::test]
    async fn a_client_retries_every_refused_request_whatever_it_retried_before() {
        for policy in ALIKE {
            let server = serving(None, |_| Reply::Reset(Reason::REFUSED_STREAM)).await;
            let client = client(policy);
            let mut requests = JoinSet::new();
            for _ in 0..REFUSALS {
                let (client, url) = (client.clone(), server.url.clone());
                requests.spawn(async move { client.post(url).send().await });
            }

            while let Some(sent) = requests.join_next().await {
                let status = sent.expect("the request ran");
                assert!(status.is_err(), "{policy:?}: {status:?}");
            }
            assert_eq!(server.streams(), 3 * REFUSALS, "{policy:?}");
        }
    }

    #[tokio::test]
    async fn the_status_is_that_of_the_answer_a_retry_ended_with() {
        let server = serving(None, refused_once).await;

        let (status, observed) = answered(post(Policy::Replica, &server)).await;

        assert_eq!(status.expect("an answer"), StatusCode::OK);
        assert_eq!(observed, Some(StatusCode::OK));
        assert_eq!(server.streams(), 2);
    }

    #[tokio::test]
    async fn a_failing_status_is_observed() {
        let server = serving(None, unavailable).await;

        let (status, observed) = answered(post(Policy::Replica, &server)).await;

        assert_eq!(status.expect("an answer"), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(observed, Some(StatusCode::SERVICE_UNAVAILABLE));
    }

    #[tokio::test]
    async fn a_request_that_got_no_answer_has_no_status() {
        let scripts: [Script; 2] = [
            |_| Reply::Reset(Reason::INTERNAL_ERROR),
            |_| Reply::Reset(Reason::REFUSED_STREAM),
        ];

        for script in scripts {
            let server = serving(None, script).await;

            let (status, observed) = answered(post(Policy::Replica, &server)).await;

            assert!(status.is_err(), "{status:?}");
            assert_eq!(observed, None);
        }
    }

    /// The client does not say what became of an attempt it could not
    /// follow with another, so an answer to the last one allowed is not
    /// known, and what is left is that the attempt before it got none.
    #[tokio::test]
    async fn an_answer_to_the_attempt_after_the_last_retry_is_not_observed() {
        let server = serving(None, |stream| match stream {
            0 | 1 => Reply::Reset(Reason::REFUSED_STREAM),
            _ => Reply::Answer(StatusCode::SERVICE_UNAVAILABLE),
        })
        .await;

        let (status, observed) = answered(post(Policy::Replica, &server)).await;

        assert_eq!(status.expect("an answer"), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(observed, None);
        assert_eq!(server.streams(), 3);
    }

    /// Each hop of a followed redirect is an attempt of its own, so the
    /// status left is the last hop's, and a hop that got no answer leaves
    /// none, the redirect that led to it included.
    #[tokio::test]
    async fn a_followed_redirect_leaves_only_what_its_last_hop_was_answered() {
        let ended: [(Script, Option<StatusCode>); 2] = [
            (
                |stream| match stream {
                    0 => Reply::Answer(StatusCode::TEMPORARY_REDIRECT),
                    _ => Reply::Answer(StatusCode::SERVICE_UNAVAILABLE),
                },
                Some(StatusCode::SERVICE_UNAVAILABLE),
            ),
            (
                |stream| match stream {
                    0 => Reply::Answer(StatusCode::TEMPORARY_REDIRECT),
                    _ => Reply::Reset(Reason::INTERNAL_ERROR),
                },
                None,
            ),
        ];

        for (script, status) in ended {
            let server = serving(None, script).await;

            let (_, observed) = answered(post(Policy::Replica, &server)).await;

            assert_eq!(observed, status);
            assert_eq!(server.streams(), 2);
        }
    }

    #[tokio::test]
    async fn a_request_sent_outside_the_scope_is_answered_and_not_observed() {
        let server = Arc::new(serving(None, unavailable).await);

        let unscoped = post(Policy::Replica, &server).await;
        let elsewhere = server.clone();
        let (spawned, observed) = answered(tokio::spawn(async move {
            post(Policy::Replica, &elsewhere).await
        }))
        .await;

        for status in [unscoped, spawned.expect("the task ran")] {
            assert_eq!(status.expect("an answer"), StatusCode::SERVICE_UNAVAILABLE);
        }
        assert_eq!(observed, None);
        assert_eq!(server.streams(), 2);
    }

    /// `cause` three sources below the request's own error, where an
    /// HTTP/2 refusal is looked for.
    fn beneath(cause: impl Error + 'static) -> Wrapped {
        Wrapped(Box::new(Wrapped(Box::new(Wrapped(Box::new(cause))))))
    }

    #[test]
    fn no_error_or_one_that_is_not_http2_is_a_refusal() {
        let reset = io::Error::from(io::ErrorKind::ConnectionReset);

        assert!(!refused(None));
        assert!(!refused(Some(&beneath(reset))));
    }

    /// A reason this side raised is no server refusing anything, though it
    /// names the very reason a server refuses with.
    #[test]
    fn a_reason_raised_locally_is_never_a_refusal() {
        for reason in [Reason::REFUSED_STREAM, Reason::NO_ERROR] {
            assert!(
                !refused(Some(&beneath(h2::Error::from(reason)))),
                "{reason:?}"
            );
        }
    }
}
