//! Mail as a caller's own code sends it: recorded by the mock mailer, or
//! through a relay on loopback that speaks just enough SMTP to tell how the
//! client reached it.

#[cfg(feature = "testing")]
mod mock {
    use std::sync::Arc;

    use abnegate_notify::{Mail, MockMailer, SentMail};

    #[tokio::test]
    async fn a_caller_asserts_on_exactly_what_would_have_gone_out() {
        let mailer = MockMailer::new();
        let held: Arc<dyn Mail> = Arc::new(mailer.clone());

        held.send(
            "person@example.test",
            "Verify your email address",
            "Open https://example.test/verify?t=abc",
        )
        .await
        .expect("recorded");

        assert_eq!(
            mailer.sent().await,
            vec![SentMail::new(
                "person@example.test",
                "Verify your email address",
                "Open https://example.test/verify?t=abc",
            )]
        );
    }
}

#[cfg(feature = "smtp")]
mod relay {
    use std::io::BufRead;
    use std::io::BufReader;
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;
    use std::net::TcpStream;
    use std::thread;
    use std::thread::JoinHandle;

    use abnegate_notify::Email;
    use abnegate_notify::Error;
    use abnegate_notify::Mail;
    use abnegate_notify::Mailer;
    use abnegate_notify::Notification;
    use abnegate_notify::Notifier;
    use abnegate_notify::Sender;
    use abnegate_notify::SmtpConfig;
    use abnegate_notify::SmtpSecurity;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    const LOOPBACK: &str = "127.0.0.1";
    const TLS_HANDSHAKE_RECORD: u8 = 0x16;

    fn listen() -> (TcpListener, u16) {
        let listener = TcpListener::bind((LOOPBACK, 0)).expect("bound");
        let port = listener.local_addr().expect("address").port();
        (listener, port)
    }

    /// A relay that offers no STARTTLS and accepts anything, and returns
    /// every line the client sent.
    fn plaintext_relay() -> (u16, JoinHandle<Vec<String>>) {
        let (listener, port) = listen();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accepted");
            converse(stream)
        });
        (port, handle)
    }

    fn converse(stream: TcpStream) -> Vec<String> {
        let mut writer = stream.try_clone().expect("cloned");
        let mut reader = BufReader::new(stream);
        let mut lines = Vec::new();
        let mut in_data = false;

        writer
            .write_all(b"220 relay.test ESMTP\r\n")
            .expect("greeted");
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("read") == 0 {
                return lines;
            }
            let line = line.trim_end().to_string();
            lines.push(line.clone());

            let reply: &[u8] = if in_data {
                if line != "." {
                    continue;
                }
                in_data = false;
                b"250 2.0.0 queued\r\n"
            } else {
                let command = line.to_ascii_uppercase();
                if command.starts_with("EHLO") {
                    b"250-relay.test\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n"
                } else if command.starts_with("AUTH") {
                    b"235 2.7.0 accepted\r\n"
                } else if command.starts_with("MAIL") || command.starts_with("RCPT") {
                    b"250 2.1.0 ok\r\n"
                } else if command.starts_with("DATA") {
                    in_data = true;
                    b"354 go ahead\r\n"
                } else if command.starts_with("QUIT") {
                    writer.write_all(b"221 2.0.0 bye\r\n").expect("replied");
                    return lines;
                } else {
                    b"502 5.5.1 unrecognised\r\n"
                }
            };
            writer.write_all(reply).expect("replied");
        }
    }

    /// A relay that offers STARTTLS, agrees to it and then hangs up instead
    /// of completing the handshake, and returns every line the client sent
    /// before the upgrade.
    fn failing_upgrade_relay() -> (u16, JoinHandle<Vec<String>>) {
        let (listener, port) = listen();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accepted");
            let mut writer = stream.try_clone().expect("cloned");
            let mut reader = BufReader::new(stream);
            let mut lines = Vec::new();

            writer
                .write_all(b"220 relay.test ESMTP\r\n")
                .expect("greeted");
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).expect("read") == 0 {
                    return lines;
                }
                let line = line.trim_end().to_string();
                lines.push(line.clone());

                let command = line.to_ascii_uppercase();
                if command.starts_with("EHLO") {
                    writer
                        .write_all(b"250-relay.test\r\n250-STARTTLS\r\n250 AUTH PLAIN LOGIN\r\n")
                        .expect("replied");
                } else if command.starts_with("STARTTLS") {
                    writer.write_all(b"220 2.0.0 ready\r\n").expect("replied");
                    return lines;
                } else {
                    writer
                        .write_all(b"502 5.5.1 unrecognised\r\n")
                        .expect("replied");
                }
            }
        });
        (port, handle)
    }

    /// A listener that says nothing and returns the first byte the client
    /// sent, before closing.
    fn silent_listener() -> (u16, JoinHandle<u8>) {
        let (listener, port) = listen();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accepted");
            let mut first = [0_u8; 1];
            stream.read_exact(&mut first).expect("read");
            first[0]
        });
        (port, handle)
    }

    fn sender() -> Sender {
        Sender::parse("Build Alerts <alerts@example.test>").expect("parsed")
    }

    fn signed_in(port: u16) -> SmtpConfig {
        SmtpConfig::new(
            LOOPBACK,
            port,
            "postmaster",
            "hunter2-not-a-real-password",
            sender(),
        )
    }

    fn commands(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_ascii_uppercase()
            })
            .collect()
    }

    #[tokio::test]
    async fn a_plaintext_relay_without_credentials_delivers_without_signing_in() {
        let (port, relay) = plaintext_relay();
        let config = SmtpConfig::unauthenticated(LOOPBACK, port, sender())
            .with_security(SmtpSecurity::Plaintext);

        Mailer::new(&config)
            .expect("configured")
            .send("Sam Example <sam@example.test>", "Deployed", "main is live")
            .await
            .expect("delivered");

        let lines = relay.join().expect("relay");
        let commands = commands(&lines);
        assert_eq!(
            commands.first().map(String::as_str),
            Some("EHLO"),
            "{lines:?}"
        );
        assert!(!commands.contains(&"AUTH".to_string()), "{lines:?}");
        assert!(!commands.contains(&"STARTTLS".to_string()), "{lines:?}");
        assert!(
            lines.contains(&"MAIL FROM:<alerts@example.test>".to_string()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"RCPT TO:<sam@example.test>".to_string()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"From: \"Build Alerts\" <alerts@example.test>".to_string()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"To: \"Sam Example\" <sam@example.test>".to_string()),
            "{lines:?}"
        );
        assert!(lines.contains(&"main is live".to_string()), "{lines:?}");
    }

    #[tokio::test]
    async fn a_plaintext_relay_with_credentials_signs_in() {
        let (port, relay) = plaintext_relay();
        let config = signed_in(port).with_security(SmtpSecurity::Plaintext);

        Email::new(&config, &["sam@example.test"])
            .expect("configured")
            .deliver(&Notification::new("Deployed", "main is live"))
            .await
            .expect("delivered");

        let lines = relay.join().expect("relay");
        let commands = commands(&lines);
        let auth = commands.iter().position(|command| command == "AUTH");
        let mail = commands.iter().position(|command| command == "MAIL");
        assert!(auth.is_some() && auth < mail, "{lines:?}");
        assert_eq!(
            signed_in_as(&lines),
            (
                "postmaster".to_string(),
                "hunter2-not-a-real-password".to_string()
            ),
            "{lines:?}"
        );
    }

    /// The user and password an `AUTH PLAIN` line carries, as the relay
    /// decodes them.
    fn signed_in_as(lines: &[String]) -> (String, String) {
        let encoded = lines
            .iter()
            .find_map(|line| line.strip_prefix("AUTH PLAIN "))
            .unwrap_or_else(|| panic!("no AUTH PLAIN line: {lines:?}"));
        let decoded = STANDARD.decode(encoded).expect("base64");
        let decoded = String::from_utf8(decoded).expect("utf-8");
        let mut parts = decoded.split('\0');
        assert_eq!(parts.next(), Some(""), "an authorization identity was sent");
        let user = parts.next().expect("a user").to_string();
        let password = parts.next().expect("a password").to_string();
        (user, password)
    }

    #[tokio::test]
    async fn a_relay_that_fails_the_upgrade_is_never_sent_the_credentials_or_the_message() {
        let (port, relay) = failing_upgrade_relay();
        let config = signed_in(port);
        assert_eq!(config.effective_security(), SmtpSecurity::StartTls);

        let error = Mailer::new(&config)
            .expect("configured")
            .send("sam@example.test", "Deployed", "main is live")
            .await
            .expect_err("the upgrade failed");
        assert!(matches!(error, Error::Smtp { .. }), "{error:?}");

        let lines = relay.join().expect("relay");
        let commands = commands(&lines);
        assert!(commands.contains(&"STARTTLS".to_string()), "{lines:?}");
        assert!(
            !commands.contains(&"AUTH".to_string()),
            "credentials sent: {lines:?}"
        );
        assert!(
            !commands.contains(&"MAIL".to_string()),
            "message sent: {lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("main is live")),
            "body sent: {lines:?}"
        );
    }

    #[tokio::test]
    async fn the_default_security_refuses_a_relay_that_cannot_upgrade() {
        let (port, relay) = plaintext_relay();
        let config = signed_in(port);
        assert_eq!(config.effective_security(), SmtpSecurity::StartTls);

        let error = Mailer::new(&config)
            .expect("configured")
            .send("sam@example.test", "Deployed", "main is live")
            .await
            .expect_err("no STARTTLS on offer");
        assert!(matches!(error, Error::Smtp { .. }), "{error:?}");

        let lines = relay.join().expect("relay");
        let commands = commands(&lines);
        assert!(commands.contains(&"EHLO".to_string()), "{lines:?}");
        assert!(
            !commands.contains(&"AUTH".to_string()),
            "credentials sent: {lines:?}"
        );
        assert!(
            !commands.contains(&"MAIL".to_string()),
            "message sent: {lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("main is live")),
            "{lines:?}"
        );
    }

    #[tokio::test]
    async fn implicit_tls_opens_with_a_handshake_before_any_greeting() {
        let (port, listener) = silent_listener();
        let config = signed_in(port).with_security(SmtpSecurity::Implicit);

        let error = Mailer::new(&config)
            .expect("configured")
            .send("sam@example.test", "Deployed", "main is live")
            .await
            .expect_err("no TLS on the far side");
        assert!(matches!(error, Error::Smtp { .. }), "{error:?}");

        assert_eq!(listener.join().expect("listener"), TLS_HANDSHAKE_RECORD);
    }

    #[tokio::test]
    async fn a_line_break_in_a_name_never_reaches_the_relay_as_a_header() {
        let injected = "Sam\r\nBcc: hidden@example.test <sam@example.test>";
        assert!(matches!(
            Sender::parse(injected),
            Err(Error::Malformed { .. })
        ));

        let (port, relay) = plaintext_relay();
        let config = SmtpConfig::unauthenticated(LOOPBACK, port, sender())
            .with_security(SmtpSecurity::Plaintext);
        let mailer = Mailer::new(&config).expect("configured");
        let error = mailer
            .send(injected, "Deployed", "main is live")
            .await
            .expect_err("injected recipient");
        assert!(matches!(error, Error::Malformed { .. }), "{error:?}");

        mailer
            .send(
                "=?utf-8?b?U2FtDQpCY2M6IGhpZGRlbkBleGFtcGxlLnRlc3Q=?= <sam@example.test>",
                "Deployed",
                "main is live",
            )
            .await
            .expect("delivered");

        let lines = relay.join().expect("relay");
        let headers: Vec<&String> = lines
            .iter()
            .skip_while(|line| line.as_str() != "DATA")
            .skip(1)
            .take_while(|line| !line.is_empty())
            .collect();
        assert!(
            headers.iter().any(|line| line.starts_with("To: ")),
            "{headers:?}"
        );
        assert!(
            headers
                .iter()
                .any(|line| line.ends_with(" <sam@example.test>")),
            "{headers:?}"
        );
        assert!(
            !headers
                .iter()
                .any(|line| line.to_ascii_lowercase().starts_with("bcc:")),
            "{headers:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|line| line.contains("hidden@example.test")),
            "{lines:?}"
        );
    }

    #[test]
    fn port_465_means_implicit_tls() {
        assert_eq!(signed_in(465).effective_security(), SmtpSecurity::Implicit);
        assert_eq!(SmtpSecurity::for_port(465), SmtpSecurity::Implicit);
    }

    #[test]
    fn port_587_means_starttls() {
        assert_eq!(signed_in(587).effective_security(), SmtpSecurity::StartTls);
        assert_eq!(SmtpSecurity::for_port(587), SmtpSecurity::StartTls);
    }

    #[test]
    fn a_named_mailbox_parses_into_a_sender() {
        let parsed = Sender::parse("\"Ops, Night Shift\" <ops@example.test>").expect("parsed");
        assert_eq!(parsed.address, "ops@example.test");
        assert_eq!(parsed.name.as_deref(), Some("Ops, Night Shift"));
    }

    #[test]
    fn an_invalid_mailbox_is_refused_everywhere() {
        let config = signed_in(587);
        for text in [
            "Ops <ops@example.test",
            "ops",
            "Ops <>",
            "ops@@example.test",
        ] {
            assert!(
                matches!(Sender::parse(text), Err(Error::Malformed { .. })),
                "{text}"
            );
            assert!(
                matches!(Email::new(&config, &[text]), Err(Error::Malformed { .. })),
                "{text}"
            );
        }
    }
}
