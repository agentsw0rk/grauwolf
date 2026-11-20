//! The AWS event stream decoder against hand-built frames.

use grauwolf_bedrock::eventstream::{DecodeError, Decoder, EventMessage, crc32};

fn string_header(name: &str, value: &str) -> Vec<u8> {
    let mut header = vec![name.len() as u8];
    header.extend(name.as_bytes());
    header.push(7);
    header.extend((value.len() as u16).to_be_bytes());
    header.extend(value.as_bytes());
    header
}

/// A complete frame with the given headers (already encoded) and payload.
fn frame_with(headers: &[u8], payload: &[u8]) -> Vec<u8> {
    let total = 12 + headers.len() + payload.len() + 4;
    let mut frame = Vec::with_capacity(total);
    frame.extend((total as u32).to_be_bytes());
    frame.extend((headers.len() as u32).to_be_bytes());
    frame.extend(crc32(&frame[..8]).to_be_bytes());
    frame.extend(headers);
    frame.extend(payload);
    frame.extend(crc32(&frame).to_be_bytes());
    frame
}

fn event(kind: &str, payload: &str) -> Vec<u8> {
    let headers = [
        string_header(":event-type", kind),
        string_header(":content-type", "application/json"),
        string_header(":message-type", "event"),
    ]
    .concat();
    frame_with(&headers, payload.as_bytes())
}

#[test]
fn computes_the_standard_crc32() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn decodes_an_event_with_its_headers_and_payload() {
    let mut decoder = Decoder::default();

    let messages = decoder
        .push(&event("contentBlockDelta", r#"{"delta":{"text":"Hi"}}"#))
        .unwrap();

    assert_eq!(
        messages,
        vec![EventMessage {
            message_type: "event".into(),
            kind: "contentBlockDelta".into(),
            content_type: Some("application/json".into()),
            payload: br#"{"delta":{"text":"Hi"}}"#.to_vec(),
        }]
    );
    assert!(!decoder.has_partial_message());
}

#[test]
fn decodes_several_messages_in_one_chunk() {
    let chunk = [event("messageStart", "{}"), event("messageStop", "{}")].concat();

    let messages = Decoder::default().push(&chunk).unwrap();

    let kinds: Vec<_> = messages.iter().map(|m| m.kind.as_str()).collect();
    assert_eq!(kinds, ["messageStart", "messageStop"]);
}

#[test]
fn waits_for_the_rest_of_a_message_split_at_any_byte() {
    let stream = [
        event("messageStart", "{}"),
        event("contentBlockStop", r#"{"contentBlockIndex":0}"#),
    ]
    .concat();
    for split in 1..stream.len() {
        let mut decoder = Decoder::default();

        let mut messages = decoder.push(&stream[..split]).unwrap();
        messages.extend(decoder.push(&stream[split..]).unwrap());

        let kinds: Vec<_> = messages.iter().map(|m| m.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["messageStart", "contentBlockStop"],
            "split at {split}"
        );
    }
}

#[test]
fn decodes_byte_by_byte() {
    let stream = event("metadata", r#"{"usage":{"inputTokens":1}}"#);
    let mut decoder = Decoder::default();
    let mut messages = Vec::new();

    for byte in &stream {
        messages.extend(decoder.push(&[*byte]).unwrap());
    }

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, "metadata");
}

#[test]
fn rejects_a_damaged_prelude() {
    let mut frame = event("messageStart", "{}");
    frame[9] ^= 0xFF;

    let error = Decoder::default().push(&frame).unwrap_err();

    assert_eq!(error, DecodeError::BadChecksum { part: "prelude" });
}

#[test]
fn rejects_a_damaged_message() {
    let mut frame = event("messageStart", r#"{"role":"assistant"}"#);
    let at = frame.len() - 6;
    frame[at] ^= 0x01;

    let error = Decoder::default().push(&frame).unwrap_err();

    assert_eq!(error, DecodeError::BadChecksum { part: "message" });
}

#[test]
fn rejects_a_damaged_message_checksum() {
    let mut frame = event("messageStart", "{}");
    let at = frame.len() - 1;
    frame[at] ^= 0x01;

    let error = Decoder::default().push(&frame).unwrap_err();

    assert_eq!(error, DecodeError::BadChecksum { part: "message" });
}

#[test]
fn rejects_lengths_that_cannot_be_right() {
    // A valid prelude CRC but headers longer than the message.
    let mut prelude = Vec::new();
    prelude.extend(20u32.to_be_bytes());
    prelude.extend(500u32.to_be_bytes());
    prelude.extend(crc32(&prelude).to_be_bytes());

    let error = Decoder::default().push(&prelude).unwrap_err();

    assert!(matches!(error, DecodeError::Malformed(_)), "{error:?}");
}

#[test]
fn reports_exceptions_with_their_type() {
    let headers = [
        string_header(":exception-type", "throttlingException"),
        string_header(":content-type", "application/json"),
        string_header(":message-type", "exception"),
    ]
    .concat();

    let messages = Decoder::default()
        .push(&frame_with(&headers, br#"{"message":"Too many requests"}"#))
        .unwrap();

    assert_eq!(messages[0].message_type, "exception");
    assert_eq!(messages[0].kind, "throttlingException");
}

#[test]
fn skips_headers_of_other_types() {
    // A timestamp (type 8), an int (type 4), a byte array (type 6) and a bool (type 0) before the
    // string headers.
    let mut headers = Vec::new();
    headers.extend([2, b'a', b'b', 8, 0, 0, 0, 0, 0, 0, 0, 1]);
    headers.extend([1, b'c', 4, 0, 0, 0, 9]);
    headers.extend([1, b'd', 6, 0, 2, 0xAA, 0xBB]);
    headers.extend([1, b'e', 0]);
    headers.extend(string_header(":event-type", "messageStart"));
    headers.extend(string_header(":message-type", "event"));

    let messages = Decoder::default()
        .push(&frame_with(&headers, b"{}"))
        .unwrap();

    assert_eq!(messages[0].kind, "messageStart");
}

#[test]
fn notices_a_stream_that_ends_inside_a_message() {
    let frame = event("messageStart", "{}");
    let mut decoder = Decoder::default();

    let messages = decoder.push(&frame[..frame.len() - 3]).unwrap();

    assert!(messages.is_empty());
    assert!(decoder.has_partial_message());
}
