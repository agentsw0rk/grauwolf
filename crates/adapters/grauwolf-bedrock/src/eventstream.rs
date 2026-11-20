//! Decoder of the AWS event stream framing (`application/vnd.amazon.eventstream`) that
//! `converse-stream` answers with.
//!
//! A message is: total length (u32), headers length (u32), CRC32 of these 8 bytes (u32), the
//! headers, the payload, CRC32 of everything before it (u32); all numbers big endian.

/// A decoded message: its headers that matter and the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventMessage {
    /// `:message-type`: `event`, `exception` or `error`.
    pub message_type: String,
    /// `:event-type` (events) or `:exception-type` (exceptions) or `:error-code` (errors).
    pub kind: String,
    /// `:content-type`, e.g. `application/json`.
    pub content_type: Option<String>,
    pub payload: Vec<u8>,
}

/// The stream is damaged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The CRC32 of the prelude or of the message does not match.
    BadChecksum { part: &'static str },
    /// Lengths that cannot be right, or headers that cannot be read.
    Malformed(&'static str),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::BadChecksum { part } => write!(f, "checksum of the {part} does not match"),
            DecodeError::Malformed(reason) => write!(f, "malformed event stream: {reason}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// The smallest possible message: prelude (12 bytes) and message CRC (4 bytes).
const MIN_MESSAGE: usize = 16;
/// Larger messages are not plausible (the service limits events to 24 MiB at most).
const MAX_MESSAGE: usize = 32 * 1024 * 1024;

/// CRC-32 (IEEE 802.3, reflected, as in zlib).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Collects chunks of any size and returns the messages that are complete.
#[derive(Debug, Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    /// Adds `chunk` and returns all messages that are complete now. After an error the decoder
    /// must not be used any more.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<EventMessage>, DecodeError> {
        self.feed(chunk);
        let mut messages = Vec::new();
        while let Some(message) = self.next_message()? {
            messages.push(message);
        }
        Ok(messages)
    }

    /// Adds `chunk` without decoding; read the messages with [`Decoder::next_message`].
    pub fn feed(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// Were bytes left over that do not make a whole message (a cut-off stream)?
    pub fn has_partial_message(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// The next complete message, `None` if more bytes are needed.
    pub fn next_message(&mut self) -> Result<Option<EventMessage>, DecodeError> {
        let buffer = &self.buffer;
        if buffer.len() < 12 {
            return Ok(None);
        }
        let number =
            |at: usize| u32::from_be_bytes(buffer[at..at + 4].try_into().expect("4 bytes"));
        if crc32(&buffer[..8]) != number(8) {
            return Err(DecodeError::BadChecksum { part: "prelude" });
        }
        let total = number(0) as usize;
        let headers_length = number(4) as usize;
        if !(MIN_MESSAGE..=MAX_MESSAGE).contains(&total) || headers_length > total - MIN_MESSAGE {
            return Err(DecodeError::Malformed(
                "impossible message or headers length",
            ));
        }
        if buffer.len() < total {
            return Ok(None);
        }
        if crc32(&buffer[..total - 4]) != number(total - 4) {
            return Err(DecodeError::BadChecksum { part: "message" });
        }
        let headers = &buffer[12..12 + headers_length];
        let payload = buffer[12 + headers_length..total - 4].to_vec();
        let parsed = parse_headers(headers)?;
        self.buffer.drain(..total);
        let header = |name: &str| {
            parsed
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let message_type = header(":message-type").unwrap_or_default();
        let kind = match message_type.as_str() {
            "exception" => header(":exception-type"),
            "error" => header(":error-code"),
            _ => header(":event-type"),
        }
        .unwrap_or_default();
        Ok(Some(EventMessage {
            message_type,
            kind,
            content_type: header(":content-type"),
            payload,
        }))
    }
}

/// The string headers (type 7); other types are skipped by their length.
fn parse_headers(mut bytes: &[u8]) -> Result<Vec<(String, String)>, DecodeError> {
    const BAD: DecodeError = DecodeError::Malformed("unreadable header");
    let mut headers = Vec::new();
    while !bytes.is_empty() {
        let name_length = usize::from(*bytes.first().ok_or(BAD)?);
        let name = bytes.get(1..1 + name_length).ok_or(BAD)?;
        let kind = *bytes.get(1 + name_length).ok_or(BAD)?;
        let rest = &bytes[2 + name_length..];
        let (value_length, skip) = match kind {
            0 | 1 => (0, 0),
            2 => (1, 0),
            3 => (2, 0),
            4 => (4, 0),
            5 | 8 => (8, 0),
            9 => (16, 0),
            6 | 7 => {
                let length = rest.get(..2).ok_or(BAD)?;
                (usize::from(u16::from_be_bytes([length[0], length[1]])), 2)
            }
            _ => return Err(BAD),
        };
        let value = rest.get(skip..skip + value_length).ok_or(BAD)?;
        if kind == 7 {
            headers.push((
                String::from_utf8_lossy(name).into_owned(),
                String::from_utf8_lossy(value).into_owned(),
            ));
        }
        bytes = &rest[skip + value_length..];
    }
    Ok(headers)
}
