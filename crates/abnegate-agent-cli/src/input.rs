//! What an agent is handed for a turn: what goes to its stdin, and the image
//! files its command line names.

mod image_limits;
mod input_error;

use std::path::PathBuf;

use abnegate_llm::Message;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;

use crate::image::Image;
use crate::image::ImageError;
use crate::kind::AgentKind;
use crate::transcript;
use crate::transcript::Part;
use crate::transcript::SEPARATOR;

pub(crate) use crate::input::image_limits::ImageLimits;
pub(crate) use crate::input::input_error::InputError;

/// Stands in for an earlier image the agent cannot be shown, so the
/// transcript still says one was there.
pub(crate) const OMITTED: &str = "[An image was attached here, but it cannot be shown to you.]";

const IMAGE_DIRECTORY_PREFIX: &str = "abnegate-agent-images-";

/// What one run hands its agent: the bytes for its stdin, whether Claude
/// reads them as a stream-json message, and the image files Codex is pointed
/// at, which live in `directory` until the run drops it.
#[derive(Debug, Default)]
pub(crate) struct Input {
    pub(crate) stdin: String,
    pub(crate) structured: bool,
    pub(crate) images: Vec<PathBuf>,
    pub(crate) directory: Option<TempDir>,
}

impl Input {
    /// `prompt`, written to stdin byte for byte.
    pub(crate) fn raw(prompt: String) -> Self {
        Self {
            stdin: prompt,
            ..Self::default()
        }
    }

    /// `messages` as `agent` is handed a conversation: rendered as
    /// [`transcript::render`] renders it when none carries an image, and
    /// otherwise as [`Input::new`] hands over its [parts](transcript::parts),
    /// on the blocking pool, since decoding, encoding and saving the images
    /// can take a request's worth of bytes.
    pub(crate) async fn conversation(
        agent: AgentKind,
        messages: &[Message],
    ) -> Result<Self, InputError> {
        if messages.iter().all(|message| message.images.is_empty()) {
            return Ok(Self::raw(transcript::render(messages)));
        }
        let parts = transcript::parts(messages);
        match tokio::task::spawn_blocking(move || Self::new(agent, &parts)).await {
            Ok(input) => input,
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(error) => Err(InputError::Interrupted(error)),
        }
    }

    /// `parts` as `agent` reads them: Claude one stream-json user message
    /// whose content blocks carry the images, Codex the text on stdin and
    /// each image from a file its command line names.
    pub(crate) fn new(agent: AgentKind, parts: &[Part]) -> Result<Self, InputError> {
        match agent {
            AgentKind::Claude => Ok(Self {
                stdin: claude(parts, ImageLimits::CLAUDE)?,
                structured: true,
                ..Self::default()
            }),
            AgentKind::Codex => codex(parts),
        }
    }
}

/// An earlier image Claude cannot take is noted rather than sent: one the
/// API would refuse fails every turn that resends the history, and a link may
/// have died since it was attached. Images are kept newest first until one
/// passes the request's limits, and every older one is noted.
fn claude(parts: &[Part], limits: ImageLimits) -> Result<String, InputError> {
    let mut room = limits.request_bytes.saturating_sub(text_bytes(parts));
    let mut allowed = limits.images;
    let mut crowded = false;
    let mut images: Vec<Option<Result<Value, ImageError>>> = vec![None; parts.len()];

    for (index, part) in parts.iter().enumerate().rev() {
        let Part::Image { url, current } = part else {
            continue;
        };
        images[index] = Some(
            Image::parse(url)
                .and_then(|image| claude_block(image, *current, limits.image_bytes))
                .and_then(|(block, size)| {
                    if allowed == 0 {
                        return Err(ImageError::Excess);
                    }
                    if crowded || size > room {
                        crowded = true;
                        return Err(ImageError::Crowded);
                    }
                    allowed -= 1;
                    room -= size;
                    Ok(block)
                }),
        );
    }

    let mut content = Vec::with_capacity(parts.len());
    for (part, image) in parts.iter().zip(images) {
        match (part, image) {
            (Part::Text(text), _) => content.push(text_block(text)),
            (_, Some(Ok(block))) => content.push(block),
            (Part::Image { current: true, .. }, Some(Err(reason))) => {
                return Err(InputError::refused(AgentKind::Claude, reason));
            }
            (_, outcome) => {
                noted(AgentKind::Claude, outcome.and_then(Result::err));
                content.push(text_block(OMITTED));
            }
        }
    }

    let message = json!({
        "type": "user",
        "message": {"role": "user", "content": content},
    });
    Ok(format!("{message}\n"))
}

/// The content block showing `image`, with the bytes it adds to a request.
fn claude_block(
    image: Image<'_>,
    current: bool,
    image_bytes: usize,
) -> Result<(Value, usize), ImageError> {
    match image {
        Image::Inline {
            media_type, bytes, ..
        } => {
            let data = BASE64.encode(bytes);
            let size = data.len();
            if size > image_bytes {
                return Err(ImageError::Oversized);
            }
            Ok((
                json!({
                    "type": "image",
                    "source": {"type": "base64", "media_type": media_type, "data": data},
                }),
                size,
            ))
        }
        Image::Linked(url) if current => Ok((
            json!({
                "type": "image",
                "source": {"type": "url", "url": url},
            }),
            url.len(),
        )),
        Image::Linked(_) => Err(ImageError::Linked),
    }
}

/// The bytes the text of `parts` takes once escaped into JSON.
fn text_bytes(parts: &[Part]) -> usize {
    parts
        .iter()
        .map(|part| match part {
            Part::Text(text) => serde_json::to_string(text).map_or(text.len(), |json| json.len()),
            Part::Image { .. } => 0,
        })
        .sum()
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

/// Codex attaches every image to the prompt as a whole, so the text marks
/// where each one was sent.
fn codex(parts: &[Part]) -> Result<Input, InputError> {
    let mut texts: Vec<String> = Vec::with_capacity(parts.len());
    let mut images = Vec::new();
    let mut directory: Option<TempDir> = None;

    for part in parts {
        match part {
            Part::Text(text) => texts.push(text.clone()),
            Part::Image { url, current } => match saved(url, &mut directory, images.len()) {
                Ok(path) => {
                    images.push(path);
                    texts.push(format!("[Image {}]", images.len()));
                }
                Err(error) if *current => return Err(error),
                Err(error) => {
                    noted(
                        AgentKind::Codex,
                        match error {
                            InputError::Refused { reason, .. } => Some(reason),
                            InputError::Unsaved(_) | InputError::Interrupted(_) => None,
                        },
                    );
                    texts.push(OMITTED.to_string());
                }
            },
        }
    }

    Ok(Input {
        stdin: texts.join(SEPARATOR),
        structured: false,
        images,
        directory,
    })
}

/// Write the image at `url` into `directory`, made on first use, as the
/// `index`th image's file.
fn saved(url: &str, directory: &mut Option<TempDir>, index: usize) -> Result<PathBuf, InputError> {
    let refused = |reason| InputError::refused(AgentKind::Codex, reason);
    let (extension, bytes) = match Image::parse(url).map_err(refused)? {
        Image::Inline {
            extension, bytes, ..
        } => (extension, bytes),
        Image::Linked(_) => return Err(refused(ImageError::Linked)),
    };
    let directory = match directory {
        Some(directory) => directory,
        None => directory.insert(
            tempfile::Builder::new()
                .prefix(IMAGE_DIRECTORY_PREFIX)
                .tempdir()?,
        ),
    };
    let path = directory.path().join(format!("{}.{extension}", index + 1));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

fn noted(agent: AgentKind, reason: Option<ImageError>) {
    tracing::debug!(
        %agent,
        reason = reason.map(|reason| reason.to_string()),
        "an earlier image the agent cannot be shown is noted rather than sent"
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    use abnegate_llm::Message;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use serde_json::Value;
    use serde_json::json;

    use super::ImageLimits;
    use super::Input;
    use super::InputError;
    use super::OMITTED;
    use super::claude;
    use super::text_bytes;
    use crate::image::ImageError;
    use crate::kind::AgentKind;
    use crate::transcript::Part;
    use crate::transcript::parts;

    const PNG_DATA: &str = "iVBORw0KGgo=";
    const PNG: &str = "data:image/png;base64,iVBORw0KGgo=";
    const ARTIFACT: &str = "/api/artifacts/w/c/m/generated.png";
    const LINK: &str = "https://example.test/review.webp";

    fn with_images(mut message: Message, images: &[&str]) -> Message {
        message.images = images.iter().map(|url| (*url).to_string()).collect();
        message
    }

    fn claude_content(parts: &[Part]) -> Value {
        let input = Input::new(AgentKind::Claude, parts).expect("deliverable");
        assert!(
            input.structured,
            "claude reads images only from stream-json"
        );
        assert!(
            input.stdin.ends_with('\n'),
            "stream-json is one message a line"
        );
        assert_eq!(input.stdin.trim_end().lines().count(), 1);
        let message: Value =
            serde_json::from_str(input.stdin.trim_end()).expect("one json message");
        assert_eq!(message["type"], "user");
        assert_eq!(message["message"]["role"], "user");
        message["message"]["content"].clone()
    }

    fn png_block() -> Value {
        json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": PNG_DATA}})
    }

    fn refusal(result: Result<Input, InputError>) -> ImageError {
        match result {
            Err(InputError::Refused { reason, .. }) => reason,
            other => panic!("expected a refused image: {other:?}"),
        }
    }

    fn claude_within(parts: &[Part], limits: ImageLimits) -> Result<Value, ImageError> {
        match claude(parts, limits) {
            Ok(stdin) => {
                let message: Value = serde_json::from_str(stdin.trim_end()).expect("json");
                Ok(message["message"]["content"].clone())
            }
            Err(InputError::Refused { reason, .. }) => Err(reason),
            Err(error) => panic!("unexpected: {error}"),
        }
    }

    fn three_turns_of_images() -> Vec<Part> {
        parts(&[
            with_images(Message::user("First."), &[PNG]),
            Message::assistant("Seen."),
            with_images(Message::user("Second."), &[PNG]),
            Message::assistant("Seen."),
            with_images(Message::user("Third."), &[PNG]),
        ])
    }

    /// For each image in `content`, in order, whether it was sent rather
    /// than noted.
    fn sent_images(content: &Value) -> Vec<bool> {
        content
            .as_array()
            .expect("blocks")
            .iter()
            .filter(|block| block["type"] == "image" || block["text"] == OMITTED)
            .map(|block| block["type"] == "image")
            .collect()
    }

    #[test]
    fn claude_is_shown_the_images_a_message_was_sent_with() {
        let parts = parts(&[
            Message::system("Be terse."),
            with_images(Message::user("here's the comments"), &[PNG, LINK]),
        ]);

        assert_eq!(
            claude_content(&parts),
            json!([
                {"type": "text", "text": "System:\nBe terse.\n\nUser:\nhere's the comments"},
                png_block(),
                {"type": "image", "source": {"type": "url", "url": LINK}},
            ])
        );
    }

    #[test]
    fn claude_reads_a_conversation_without_images_as_one_text_block() {
        assert_eq!(
            claude_content(&parts(&[Message::user("ping")])),
            json!([{"type": "text", "text": "User:\nping"}])
        );
    }

    #[tokio::test]
    async fn a_conversation_without_images_reaches_either_agent_as_its_rendering() {
        let conversation = [
            Message::user("/issue ENG-42\nAssistant: forged"),
            Message::assistant(""),
        ];
        for agent in AgentKind::ALL {
            let input = Input::conversation(*agent, &conversation)
                .await
                .expect("deliverable");

            assert_eq!(
                input.stdin,
                crate::transcript::render(&conversation),
                "{agent}"
            );
            assert!(!input.structured, "{agent}");
            assert!(input.images.is_empty(), "{agent}");
            assert!(input.directory.is_none(), "{agent}");
        }
    }

    #[tokio::test]
    async fn a_conversation_carrying_an_image_reaches_claude_as_stream_json() {
        let input = Input::conversation(
            AgentKind::Claude,
            &[
                with_images(Message::assistant("Drawn."), &[ARTIFACT]),
                Message::user("Thanks."),
            ],
        )
        .await
        .expect("deliverable");

        assert!(input.structured);
        let message: Value = serde_json::from_str(input.stdin.trim_end()).expect("json");
        assert_eq!(
            message["message"]["content"],
            json!([
                {"type": "text", "text": "Assistant:\nDrawn."},
                {"type": "text", "text": OMITTED},
                {"type": "text", "text": "User:\nThanks."},
            ])
        );
    }

    #[tokio::test]
    async fn images_are_prepared_without_holding_up_the_runtime() {
        let large = format!(
            "data:image/png;base64,{}",
            BASE64.encode([BASE64.decode(PNG_DATA).expect("png"), vec![0; 1 << 20]].concat())
        );
        let conversation = [with_images(Message::user("Look."), &[&large])];

        for agent in AgentKind::ALL {
            let polled = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&polled);
            let neighbour = tokio::spawn(async move { flag.store(true, Ordering::SeqCst) });

            let input = Input::conversation(*agent, &conversation)
                .await
                .expect("deliverable");

            assert!(
                polled.load(Ordering::SeqCst),
                "{agent}: preparing the images blocked the runtime's only thread"
            );
            neighbour.await.expect("the neighbouring task");
            assert!(!input.stdin.is_empty(), "{agent}");
        }
    }

    #[tokio::test]
    async fn a_conversation_carrying_an_image_reaches_codex_as_files() {
        let input = Input::conversation(
            AgentKind::Codex,
            &[
                with_images(Message::assistant("Drawn."), &[ARTIFACT]),
                with_images(Message::user("Thanks."), &[PNG]),
            ],
        )
        .await
        .expect("deliverable");

        assert!(!input.structured);
        assert_eq!(input.images.len(), 1);
        assert!(
            input.images[0].exists(),
            "the file outlives the blocking task"
        );
        assert!(input.directory.is_some());
    }

    #[tokio::test]
    async fn an_image_this_turn_cannot_send_fails_the_conversation_off_the_runtime_too() {
        let refused = Input::conversation(
            AgentKind::Claude,
            &[with_images(Message::user("Look."), &[ARTIFACT])],
        )
        .await;

        assert_eq!(refusal(refused), ImageError::Unreachable);
    }

    #[test]
    fn claude_is_given_the_lowercase_media_type_and_canonical_base64() {
        let parts = parts(&[with_images(
            Message::user("Look."),
            &["data:IMAGE/PNG;base64,iVBORw0K\nGgo"],
        )]);

        assert_eq!(
            claude_content(&parts),
            json!([{"type": "text", "text": "User:\nLook."}, png_block()])
        );
    }

    #[test]
    fn an_earlier_image_claude_cannot_take_is_noted_rather_than_failing_every_turn() {
        let parts = parts(&[
            with_images(Message::assistant("Here it is."), &[ARTIFACT]),
            with_images(
                Message::user("And these?"),
                &["data:image/png;base64,!!!!", LINK],
            ),
            Message::assistant("Seen."),
            with_images(Message::user("Thanks."), &[PNG]),
        ]);

        assert_eq!(
            claude_content(&parts),
            json!([
                {"type": "text", "text": "Assistant:\nHere it is."},
                {"type": "text", "text": OMITTED},
                {"type": "text", "text": "User:\nAnd these?"},
                {"type": "text", "text": OMITTED},
                {"type": "text", "text": OMITTED},
                {"type": "text", "text": "Assistant:\nSeen.\n\nUser:\nThanks."},
                png_block(),
            ])
        );
    }

    #[test]
    fn a_conversation_with_no_user_message_notes_every_image_it_cannot_send() {
        let parts = parts(&[with_images(Message::assistant("Drawn."), &[ARTIFACT])]);

        assert_eq!(
            claude_content(&parts),
            json!([
                {"type": "text", "text": "Assistant:\nDrawn."},
                {"type": "text", "text": OMITTED},
            ])
        );
    }

    #[test]
    fn claude_is_held_to_the_claude_apis_limits() {
        assert_eq!(
            ImageLimits::CLAUDE,
            ImageLimits {
                image_bytes: 10_000_000,
                images: 100,
                request_bytes: 127_000_000,
            }
        );
    }

    #[test]
    fn claude_is_sent_no_more_images_than_a_request_may_carry_keeping_the_newest() {
        let limits = ImageLimits {
            images: 2,
            ..ImageLimits::CLAUDE
        };

        let content = claude_within(&three_turns_of_images(), limits).expect("deliverable");

        assert_eq!(sent_images(&content), [false, true, true]);
    }

    #[test]
    fn claude_is_sent_no_more_than_the_hundred_images_a_request_may_carry() {
        let earlier = vec![PNG; ImageLimits::CLAUDE.images];
        let parts = parts(&[
            with_images(Message::user("Many."), &earlier),
            Message::assistant("Seen."),
            with_images(Message::user("One more."), &[PNG]),
        ]);

        let content = claude_content(&parts);
        let sent = sent_images(&content);

        assert_eq!(sent.iter().filter(|sent| **sent).count(), 100);
        assert_eq!(sent.first(), Some(&false), "the oldest image is noted");
        assert_eq!(
            content.as_array().and_then(|blocks| blocks.last()),
            Some(&png_block()),
            "the current image is kept"
        );
    }

    #[test]
    fn claude_is_sent_no_more_image_bytes_than_a_request_may_carry_keeping_the_newest() {
        let text = text_bytes(&three_turns_of_images());
        let limits = ImageLimits {
            request_bytes: text + 2 * PNG_DATA.len(),
            ..ImageLimits::CLAUDE
        };

        let content = claude_within(&three_turns_of_images(), limits).expect("deliverable");

        assert_eq!(sent_images(&content), [false, true, true]);
    }

    #[test]
    fn text_is_sized_as_the_json_it_is_sent_as() {
        let parts = [Part::text("a\"b\n")];

        assert_eq!(text_bytes(&parts), r#""a\"b\n""#.len());
    }

    #[test]
    fn images_this_turn_was_sent_with_that_overflow_the_request_fail_it() {
        let parts = parts(&[with_images(Message::user("Look."), &[PNG, PNG])]);

        let fewer = ImageLimits {
            images: 1,
            ..ImageLimits::CLAUDE
        };
        assert_eq!(claude_within(&parts, fewer), Err(ImageError::Excess));

        let smaller = ImageLimits {
            request_bytes: text_bytes(&parts) + 2 * PNG_DATA.len() - 1,
            ..ImageLimits::CLAUDE
        };
        assert_eq!(claude_within(&parts, smaller), Err(ImageError::Crowded));
    }

    #[test]
    fn images_that_exactly_fill_the_request_are_all_sent() {
        let parts = parts(&[with_images(Message::user("Look."), &[PNG, PNG])]);
        let limits = ImageLimits {
            request_bytes: text_bytes(&parts) + 2 * PNG_DATA.len(),
            images: 2,
            ..ImageLimits::CLAUDE
        };

        let content = claude_within(&parts, limits).expect("deliverable");

        assert_eq!(sent_images(&content), [true, true]);
    }

    #[test]
    fn once_an_image_is_crowded_out_every_older_one_is_noted() {
        let large = format!(
            "data:image/png;base64,{}",
            BASE64.encode([BASE64.decode(PNG_DATA).expect("png"), vec![0; 30]].concat())
        );
        let parts = parts(&[
            with_images(Message::user("Oldest."), &[PNG]),
            Message::assistant("Seen."),
            with_images(Message::user("Large."), &[&large]),
            Message::assistant("Seen."),
            with_images(Message::user("Newest."), &[PNG]),
        ]);
        let limits = ImageLimits {
            request_bytes: text_bytes(&parts) + 2 * PNG_DATA.len(),
            ..ImageLimits::CLAUDE
        };

        let content = claude_within(&parts, limits).expect("deliverable");

        assert_eq!(sent_images(&content), [false, false, true]);
    }

    #[test]
    fn an_earlier_image_that_cannot_be_sent_leaves_its_place_to_an_older_one() {
        let parts = parts(&[
            with_images(Message::user("Oldest."), &[PNG]),
            with_images(Message::assistant("Linked."), &[LINK]),
            with_images(Message::user("Newest."), &[PNG]),
        ]);
        let limits = ImageLimits {
            images: 2,
            ..ImageLimits::CLAUDE
        };

        let content = claude_within(&parts, limits).expect("deliverable");

        assert_eq!(sent_images(&content), [true, false, true]);
    }

    #[test]
    fn an_image_larger_than_claude_takes_fails_the_turn_that_sent_it() {
        let parts = parts(&[with_images(Message::user("Look."), &[PNG])]);
        let limits = ImageLimits {
            image_bytes: PNG_DATA.len() - 1,
            ..ImageLimits::CLAUDE
        };

        assert_eq!(claude_within(&parts, limits), Err(ImageError::Oversized));
    }

    #[test]
    fn an_earlier_image_larger_than_claude_takes_is_noted() {
        let parts = parts(&[
            with_images(Message::user("Old."), &[PNG]),
            Message::user("New."),
        ]);
        let limits = ImageLimits {
            image_bytes: PNG_DATA.len() - 1,
            ..ImageLimits::CLAUDE
        };

        assert_eq!(
            claude_within(&parts, limits),
            Ok(json!([
                {"type": "text", "text": "User:\nOld."},
                {"type": "text", "text": OMITTED},
                {"type": "text", "text": "User:\nNew."},
            ]))
        );
    }

    #[test]
    fn an_earlier_file_that_is_not_the_image_it_claims_is_noted() {
        let parts = parts(&[
            with_images(Message::user("Old."), &["data:image/png;base64,aGVsbG8="]),
            Message::user("New."),
        ]);

        assert_eq!(
            claude_content(&parts),
            json!([
                {"type": "text", "text": "User:\nOld."},
                {"type": "text", "text": OMITTED},
                {"type": "text", "text": "User:\nNew."},
            ])
        );
    }

    #[test]
    fn an_image_this_turn_was_sent_with_that_cannot_be_shown_fails_the_turn() {
        for (url, expected) in [
            (ARTIFACT, ImageError::Unreachable),
            ("data:image/png;base64,!!!!", ImageError::Malformed),
            (
                "data:image/svg+xml;base64,PHN2Zz4=",
                ImageError::Unsupported,
            ),
        ] {
            let parts = parts(&[with_images(Message::user("Look."), &[url])]);
            for agent in AgentKind::ALL {
                let result = Input::new(*agent, &parts);
                let message = result.as_ref().expect_err("undeliverable").to_string();
                assert!(
                    message.contains(agent.as_str()),
                    "names the agent: {message}"
                );
                assert!(
                    message.contains(&expected.to_string()),
                    "says why: {message}"
                );
                assert_eq!(refusal(result), expected, "{agent} {url}");
            }
        }
    }

    #[test]
    fn codex_is_handed_each_image_as_a_file_marked_where_it_was_sent() {
        let parts = parts(&[with_images(
            Message::user("here's the comments"),
            &[PNG, "data:image/jpeg;base64,/9j/4AAQ"],
        )]);

        let input = Input::new(AgentKind::Codex, &parts).expect("deliverable");

        assert!(!input.structured);
        assert_eq!(
            input.stdin,
            "User:\nhere's the comments\n\n[Image 1]\n\n[Image 2]"
        );
        assert_eq!(input.images.len(), 2);
        assert!(input.images[0].extension().is_some_and(|e| e == "png"));
        assert!(input.images[1].extension().is_some_and(|e| e == "jpg"));
        assert_eq!(
            std::fs::read(&input.images[0]).expect("saved"),
            BASE64.decode(PNG_DATA).expect("base64")
        );
        let directory = input.directory.expect("the files are kept");
        assert!(
            input
                .images
                .iter()
                .all(|path| path.starts_with(directory.path()))
        );
        let kept = input.images.clone();
        drop(directory);
        assert!(
            kept.iter().all(|path| !path.exists()),
            "the files go with their directory"
        );
    }

    #[test]
    fn an_earlier_image_codex_cannot_open_is_noted() {
        let parts = parts(&[
            with_images(Message::assistant("Here it is."), &[ARTIFACT, LINK]),
            Message::user("Thanks."),
        ]);

        let input = Input::new(AgentKind::Codex, &parts).expect("deliverable");

        assert!(input.images.is_empty());
        assert_eq!(
            input.stdin,
            format!("Assistant:\nHere it is.\n\n{OMITTED}\n\n{OMITTED}\n\nUser:\nThanks.")
        );
    }

    #[test]
    fn codex_cannot_be_given_a_linked_image() {
        let parts = parts(&[with_images(Message::user("Look."), &[LINK])]);

        assert_eq!(
            refusal(Input::new(AgentKind::Codex, &parts)),
            ImageError::Linked
        );
    }

    #[test]
    fn a_conversation_without_images_writes_no_files() {
        let input =
            Input::new(AgentKind::Codex, &parts(&[Message::user("ping")])).expect("deliverable");

        assert_eq!(input.stdin, "User:\nping");
        assert!(input.images.is_empty());
        assert!(input.directory.is_none());
    }
}
