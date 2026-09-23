//! NDJSON (Newline-Delimited JSON) codec for protocol messages.
//!
//! Each message is a single JSON object followed by a newline character.

use bytes::{Buf, BufMut, BytesMut};
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;
use tokio_util::codec::{Decoder, Encoder};

use crate::error::ProtocolError;

/// Maximum line length to prevent memory exhaustion
const MAX_LINE_LENGTH: usize = 16 * 1024 * 1024;

/// NDJSON codec that serializes/deserializes JSON messages with newline delimiters.
///
/// Each message is encoded as a single JSON object followed by `\n`.
/// Decoding reads lines and parses them as JSON.
pub struct NdjsonCodec<T> {
    /// Maximum allowed line length
    max_length: usize,
    /// Marker for the message type
    _phantom: PhantomData<T>,
}

impl<T> NdjsonCodec<T> {
    /// Create a new NDJSON codec with default max line length
    pub fn new() -> Self {
        Self {
            max_length: MAX_LINE_LENGTH,
            _phantom: PhantomData,
        }
    }

    /// Create a new NDJSON codec with custom max line length
    pub fn with_max_length(max_length: usize) -> Self {
        Self {
            max_length,
            _phantom: PhantomData,
        }
    }
}

impl<T> Default for NdjsonCodec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for NdjsonCodec<T> {
    fn clone(&self) -> Self {
        Self {
            max_length: self.max_length,
            _phantom: PhantomData,
        }
    }
}

impl<T: DeserializeOwned> Decoder for NdjsonCodec<T> {
    type Item = T;
    type Error = ProtocolError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let newline_pos = src.iter().position(|&b| b == b'\n');

        match newline_pos {
            Some(pos) => {
                if pos > self.max_length {
                    src.advance(pos + 1);
                    return Err(ProtocolError::LineTooLong {
                        length: pos,
                        max: self.max_length,
                    });
                }

                let line = src.split_to(pos);
                src.advance(1);

                if line.is_empty() {
                    return Ok(None);
                }

                let msg: T =
                    serde_json::from_slice(&line).map_err(|e| ProtocolError::JsonParse {
                        source: e,
                        line: String::from_utf8_lossy(&line).to_string(),
                    })?;

                Ok(Some(msg))
            }
            None => {
                if src.len() > self.max_length {
                    return Err(ProtocolError::LineTooLong {
                        length: src.len(),
                        max: self.max_length,
                    });
                }

                Ok(None)
            }
        }
    }
}

impl<T: Serialize> Encoder<T> for NdjsonCodec<T> {
    type Error = ProtocolError;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let json = serde_json::to_string(&item).map_err(ProtocolError::JsonSerialize)?;

        dst.reserve(json.len() + 1);
        dst.put_slice(json.as_bytes());
        dst.put_u8(b'\n');

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::messages::{ErrorCode, InboundMessage, LogLevel, OutboundMessage};

    #[test]
    fn test_decode_single_message() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from(
            r#"{"type":"Hello","protocol_version":"1.0","capabilities":[]}"#.as_bytes(),
        );
        buf.extend_from_slice(b"\n");

        let result = codec.decode(&mut buf).unwrap();
        assert!(result.is_some());

        match result.unwrap() {
            InboundMessage::Hello {
                protocol_version, ..
            } => {
                assert_eq!(protocol_version, "1.0");
            }
            _ => panic!("Wrong message type"),
        }

        assert!(buf.is_empty());
    }

    #[test]
    fn test_decode_partial_message() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from(r#"{"type":"Hello","protocol_version":"1.0""#.as_bytes());

        let result = codec.decode(&mut buf).unwrap();
        assert!(result.is_none());

        buf.extend_from_slice(r#","capabilities":[]}"#.as_bytes());
        buf.extend_from_slice(b"\n");

        let result = codec.decode(&mut buf).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_decode_multiple_messages() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from(
            r#"{"type":"Ping","id":"1"}
{"type":"Ping","id":"2"}
"#
            .as_bytes(),
        );

        let msg1 = codec.decode(&mut buf).unwrap().unwrap();
        let msg2 = codec.decode(&mut buf).unwrap().unwrap();

        match msg1 {
            InboundMessage::Ping { id } => assert_eq!(id, "1"),
            _ => panic!("Wrong message type"),
        }

        match msg2 {
            InboundMessage::Ping { id } => assert_eq!(id, "2"),
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_decode_many_messages_sequentially() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::new();

        for i in 0..100 {
            buf.extend_from_slice(format!(r#"{{"type":"Ping","id":"{}"}}"#, i).as_bytes());
            buf.extend_from_slice(b"\n");
        }

        for i in 0..100 {
            let msg = codec.decode(&mut buf).unwrap().unwrap();
            match msg {
                InboundMessage::Ping { id } => assert_eq!(id, i.to_string()),
                _ => panic!("Wrong message type"),
            }
        }

        assert!(buf.is_empty());
    }

    #[test]
    fn test_encode_message() {
        let mut codec: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::new();

        let msg = OutboundMessage::Pong {
            id: "test".to_string(),
        };

        codec.encode(msg, &mut buf).unwrap();

        let s = String::from_utf8(buf.to_vec()).unwrap();
        assert!(s.ends_with('\n'));
        assert!(s.contains(r#""type":"Pong""#));
        assert!(s.contains(r#""id":"test""#));
    }

    #[test]
    fn test_encode_multiple_messages() {
        let mut codec: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::new();

        codec
            .encode(
                OutboundMessage::Pong {
                    id: "1".to_string(),
                },
                &mut buf,
            )
            .unwrap();
        codec
            .encode(
                OutboundMessage::Pong {
                    id: "2".to_string(),
                },
                &mut buf,
            )
            .unwrap();
        codec
            .encode(
                OutboundMessage::Pong {
                    id: "3".to_string(),
                },
                &mut buf,
            )
            .unwrap();

        let s = String::from_utf8(buf.to_vec()).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_encode_all_outbound_message_types() {
        let mut codec: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();

        let messages = vec![
            OutboundMessage::hello_ack(),
            OutboundMessage::RunStarted {
                job_id: "j1".to_string(),
                pid: 123,
            },
            OutboundMessage::RunStdout {
                job_id: "j1".to_string(),
                data: "dGVzdA==".to_string(),
                sequence: 1,
            },
            OutboundMessage::RunStderr {
                job_id: "j1".to_string(),
                data: "ZXJy".to_string(),
                sequence: 1,
            },
            OutboundMessage::RunLog {
                job_id: "j1".to_string(),
                level: LogLevel::Info,
                message: "test".to_string(),
                details: None,
            },
            OutboundMessage::RunExit {
                job_id: "j1".to_string(),
                exit_code: Some(0),
                signal: None,
                duration_ms: 100,
            },
            OutboundMessage::RunError {
                job_id: "j1".to_string(),
                error_code: ErrorCode::Timeout,
                message: "timeout".to_string(),
            },
            OutboundMessage::Pong {
                id: "p1".to_string(),
            },
        ];

        for msg in messages {
            let mut buf = BytesMut::new();
            assert!(codec.encode(msg, &mut buf).is_ok());
            assert!(!buf.is_empty());
            assert!(buf.last() == Some(&b'\n'));
        }
    }

    #[test]
    fn test_decode_empty_line() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from("\n".as_bytes());

        let result = codec.decode(&mut buf).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_decode_multiple_empty_lines() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from("\n\n\n".as_bytes());

        assert!(codec.decode(&mut buf).unwrap().is_none());
        assert!(codec.decode(&mut buf).unwrap().is_none());
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn test_decode_message_after_empty_lines() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from("\n\n{\"type\":\"Ping\",\"id\":\"1\"}\n".as_bytes());

        assert!(codec.decode(&mut buf).unwrap().is_none());
        assert!(codec.decode(&mut buf).unwrap().is_none());

        let msg = codec.decode(&mut buf).unwrap().unwrap();
        match msg {
            InboundMessage::Ping { id } => assert_eq!(id, "1"),
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_decode_invalid_json() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from("not valid json\n".as_bytes());

        let result = codec.decode(&mut buf);
        assert!(result.is_err());

        match result.unwrap_err() {
            ProtocolError::JsonParse { line, .. } => {
                assert_eq!(line, "not valid json");
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_decode_truncated_json() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from("{\"type\":\"Ping\"\n".as_bytes());

        let result = codec.decode(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_wrong_type() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from("{\"type\":\"InvalidType\",\"foo\":\"bar\"}\n".as_bytes());

        let result = codec.decode(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_missing_required_fields() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from("{\"type\":\"RunStart\"}\n".as_bytes());

        let result = codec.decode(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_line_too_long() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::with_max_length(10);
        let mut buf = BytesMut::from("this line is way too long\n".as_bytes());

        let result = codec.decode(&mut buf);
        assert!(result.is_err());

        match result.unwrap_err() {
            ProtocolError::LineTooLong { length, max } => {
                assert!(length > max);
                assert_eq!(max, 10);
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_line_at_max_length() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::with_max_length(100);
        let json = r#"{"type":"Ping","id":"test"}"#;
        assert!(json.len() < 100);

        let mut buf = BytesMut::from(format!("{}\n", json).as_bytes());
        let result = codec.decode(&mut buf);
        assert!(result.is_ok());
    }

    #[test]
    fn test_buffer_growing_without_newline() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::with_max_length(50);
        let mut buf = BytesMut::new();

        buf.extend_from_slice("a".repeat(60).as_bytes());

        let result = codec.decode(&mut buf);
        assert!(result.is_err());

        match result.unwrap_err() {
            ProtocolError::LineTooLong { length, max } => {
                assert_eq!(length, 60);
                assert_eq!(max, 50);
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_roundtrip() {
        let mut encoder: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
        let mut decoder: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::new();

        let original = OutboundMessage::RunExit {
            job_id: "test-job".to_string(),
            exit_code: Some(0),
            signal: None,
            duration_ms: 1234,
        };

        encoder.encode(original.clone(), &mut buf).unwrap();
        let decoded = decoder.decode(&mut buf).unwrap().unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_roundtrip_all_message_types() {
        let messages = vec![
            OutboundMessage::hello_ack(),
            OutboundMessage::RunStarted {
                job_id: "j1".to_string(),
                pid: 12345,
            },
            OutboundMessage::RunStdout {
                job_id: "j1".to_string(),
                data: "SGVsbG8gV29ybGQ=".to_string(),
                sequence: 42,
            },
            OutboundMessage::RunStderr {
                job_id: "j1".to_string(),
                data: "RXJyb3I=".to_string(),
                sequence: 1,
            },
            OutboundMessage::RunLog {
                job_id: "j1".to_string(),
                level: LogLevel::Warn,
                message: "Test warning".to_string(),
                details: Some(serde_json::json!({"key": "value"})),
            },
            OutboundMessage::RunExit {
                job_id: "j1".to_string(),
                exit_code: Some(1),
                signal: None,
                duration_ms: 5000,
            },
            OutboundMessage::RunExit {
                job_id: "j2".to_string(),
                exit_code: None,
                signal: Some(9),
                duration_ms: 100,
            },
            OutboundMessage::RunError {
                job_id: "j1".to_string(),
                error_code: ErrorCode::Cancelled,
                message: "Cancelled".to_string(),
            },
            OutboundMessage::Pong {
                id: "ping-123".to_string(),
            },
        ];

        for original in messages {
            let mut encoder: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
            let mut decoder: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
            let mut buf = BytesMut::new();

            encoder.encode(original.clone(), &mut buf).unwrap();
            let decoded = decoder.decode(&mut buf).unwrap().unwrap();

            assert_eq!(original, decoded);
        }
    }

    #[test]
    fn test_decode_unicode_content() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::from(r#"{"type":"Ping","id":"测试🎉"}"#.as_bytes());
        buf.extend_from_slice(b"\n");

        let result = codec.decode(&mut buf).unwrap().unwrap();
        match result {
            InboundMessage::Ping { id } => {
                assert_eq!(id, "测试🎉");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_encode_unicode_content() {
        let mut codec: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
        let mut buf = BytesMut::new();

        let msg = OutboundMessage::Pong {
            id: "Привет мир 🌍".to_string(),
        };

        codec.encode(msg, &mut buf).unwrap();

        let s = String::from_utf8(buf.to_vec()).unwrap();
        assert!(s.contains("Привет") || s.contains("\\u"));
    }

    #[test]
    fn test_codec_clone() {
        let codec1: NdjsonCodec<InboundMessage> = NdjsonCodec::with_max_length(1000);
        let codec2 = codec1.clone();

        assert_eq!(codec1.max_length, codec2.max_length);
    }

    #[test]
    fn test_codec_default() {
        let codec1: NdjsonCodec<InboundMessage> = NdjsonCodec::default();
        let codec2: NdjsonCodec<InboundMessage> = NdjsonCodec::new();

        assert_eq!(codec1.max_length, codec2.max_length);
    }

    #[test]
    fn test_decode_large_message() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();

        let large_data = "x".repeat(100_000);
        let json = format!(
            r#"{{"type":"RunStdin","job_id":"j1","data":"{}"}}"#,
            large_data
        );
        let mut buf = BytesMut::from(format!("{}\n", json).as_bytes());

        let result = codec.decode(&mut buf);
        assert!(result.is_ok());

        match result.unwrap().unwrap() {
            InboundMessage::RunStdin { data, .. } => {
                assert_eq!(data.len(), 100_000);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_incremental_decode() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let full_message = r#"{"type":"Ping","id":"test123"}"#;
        let mut buf = BytesMut::new();

        for (i, ch) in full_message.bytes().enumerate() {
            buf.extend_from_slice(&[ch]);

            if i < full_message.len() - 1 {
                assert!(codec.decode(&mut buf).unwrap().is_none());
            }
        }

        buf.extend_from_slice(b"\n");

        let result = codec.decode(&mut buf).unwrap().unwrap();
        match result {
            InboundMessage::Ping { id } => assert_eq!(id, "test123"),
            _ => panic!("Wrong message type"),
        }
    }
}
