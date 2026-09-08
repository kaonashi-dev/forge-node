//! Length-prefixed MessagePack framing (ADR-004, §10).
//!
//! Every message on the wire is a frame: a `u32` big-endian payload length
//! followed by that many bytes of MessagePack (via `rmp-serde`). The helpers
//! here are transport-agnostic — they operate on byte buffers, never on sockets
//! — so the same code serves the daemon's blocking `std::io` accept loop and
//! the GUI client's `tokio` IPC thread without this crate depending on either.
//!
//! Structs are encoded as MessagePack *maps* (field names as keys) via
//! [`rmp_serde::to_vec_named`], so an older peer skips fields a newer peer added
//! rather than misreading a positional array (§10.1, forward compatibility).

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Maximum size, in bytes, of a single frame's payload. A declared or produced
/// length above this is a hard error: the caller must close the connection
/// rather than allocate unbounded memory (ADR-004, §10.5).
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Width of the big-endian length prefix.
const LEN_PREFIX: usize = 4;

/// Errors raised while encoding, decoding, or framing protocol messages.
///
/// This is a *local* error (it never travels over the wire); request failures
/// use [`crate::error::ProtocolError`] instead.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProtocolCodecError {
    /// Serializing the message to MessagePack failed.
    #[error("failed to encode message as MessagePack")]
    Encode(#[from] rmp_serde::encode::Error),
    /// Deserializing a payload from MessagePack failed.
    #[error("failed to decode MessagePack payload")]
    Decode(#[from] rmp_serde::decode::Error),
    /// Rendering a message to JSON failed (used by `dump --json`, §10.4).
    #[error("failed to render message as JSON")]
    Json(#[from] serde_json::Error),
    /// A frame's payload length exceeds [`MAX_FRAME_SIZE`]. Fatal for the
    /// connection.
    #[error("frame payload of {size} bytes exceeds MAX_FRAME_SIZE ({max})")]
    FrameTooLarge {
        /// The offending declared or produced payload length.
        size: usize,
        /// The configured limit, always [`MAX_FRAME_SIZE`].
        max: usize,
    },
    /// A non-buffering decode ([`decode_frame`]) ran out of bytes mid-frame.
    #[error("incomplete frame: need {needed} more byte(s)")]
    Incomplete {
        /// How many additional bytes are required to complete the frame.
        needed: usize,
    },
}

/// Encode a message into a complete frame: `[u32 BE payload length][payload]`.
///
/// # Errors
/// Returns [`ProtocolCodecError::Encode`] if serialization fails, or
/// [`ProtocolCodecError::FrameTooLarge`] if the payload exceeds
/// [`MAX_FRAME_SIZE`].
#[allow(clippy::cast_possible_truncation)] // length is checked <= MAX_FRAME_SIZE < u32::MAX
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, ProtocolCodecError> {
    let payload = rmp_serde::to_vec_named(msg)?;
    if payload.len() > MAX_FRAME_SIZE {
        return Err(ProtocolCodecError::FrameTooLarge {
            size: payload.len(),
            max: MAX_FRAME_SIZE,
        });
    }
    let mut frame = Vec::with_capacity(LEN_PREFIX + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode a message from a single frame's *payload* (no length prefix).
///
/// # Errors
/// Returns [`ProtocolCodecError::Decode`] if the bytes are not valid
/// MessagePack for `T`.
pub fn decode_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ProtocolCodecError> {
    Ok(rmp_serde::from_slice(payload)?)
}

/// Decode exactly one frame from the front of `bytes`, returning the message and
/// the number of bytes consumed (prefix + payload).
///
/// Unlike [`FrameDecoder`] this does not buffer: a slice shorter than the frame
/// yields [`ProtocolCodecError::Incomplete`]. Handy for callers that already
/// hold a full frame.
///
/// # Errors
/// [`ProtocolCodecError::Incomplete`] if `bytes` is too short,
/// [`ProtocolCodecError::FrameTooLarge`] if the declared length exceeds
/// [`MAX_FRAME_SIZE`], or [`ProtocolCodecError::Decode`] on a bad payload.
pub fn decode_frame<T: DeserializeOwned>(bytes: &[u8]) -> Result<(T, usize), ProtocolCodecError> {
    if bytes.len() < LEN_PREFIX {
        return Err(ProtocolCodecError::Incomplete {
            needed: LEN_PREFIX - bytes.len(),
        });
    }
    let len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(ProtocolCodecError::FrameTooLarge {
            size: len,
            max: MAX_FRAME_SIZE,
        });
    }
    let total = LEN_PREFIX + len;
    if bytes.len() < total {
        return Err(ProtocolCodecError::Incomplete {
            needed: total - bytes.len(),
        });
    }
    let msg = decode_payload(&bytes[LEN_PREFIX..total])?;
    Ok((msg, total))
}

/// Pretty-print a message as JSON for the daemon's `dump --json` debug
/// subcommand (§10.4/§10.1). Uses `serde_json`, independent of the MessagePack
/// wire format.
///
/// # Errors
/// Returns [`ProtocolCodecError::Json`] if serialization fails.
pub fn to_json_string<T: Serialize>(msg: &T) -> Result<String, ProtocolCodecError> {
    Ok(serde_json::to_string_pretty(msg)?)
}

/// A streaming frame reassembler for byte-oriented transports.
///
/// Feed it whatever bytes arrive with [`FrameDecoder::push`] (a partial frame,
/// several frames at once, or a single byte); pull complete payloads with
/// [`FrameDecoder::next_frame`]. It enforces [`MAX_FRAME_SIZE`] against the
/// *declared* length before buffering, so a malicious or corrupt peer cannot
/// force an unbounded allocation.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    /// Offset of the first unconsumed byte in `buf`.
    ///
    /// Frames are consumed by advancing this cursor, not by draining the front
    /// of the buffer: on the terminal-delta path (§10.5) `next_frame` runs many
    /// times a second, and `Vec::drain(..n)` memmoves the whole remainder every
    /// time. The prefix is reclaimed by [`FrameDecoder::compact`].
    start: usize,
}

/// Reclaim the consumed prefix once it exceeds this many bytes.
const COMPACT_THRESHOLD: usize = 64 * 1024;

impl FrameDecoder {
    /// Create an empty decoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            start: 0,
        }
    }

    /// Append freshly received bytes to the internal buffer.
    pub fn push(&mut self, bytes: &[u8]) {
        self.compact();
        self.buf.extend_from_slice(bytes);
    }

    /// Number of bytes currently buffered but not yet yielded.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buf.len() - self.start
    }

    /// Drop the already-consumed prefix, either when it has grown past
    /// [`COMPACT_THRESHOLD`] or when nothing is left to keep.
    fn compact(&mut self) {
        if self.start == 0 {
            return;
        }
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start >= COMPACT_THRESHOLD {
            self.buf.drain(..self.start);
            self.start = 0;
        }
    }

    /// Yield the next complete frame's payload, or `None` if more bytes are
    /// needed. Call it in a loop after each [`FrameDecoder::push`] until it
    /// returns `None`.
    ///
    /// # Errors
    /// Returns [`ProtocolCodecError::FrameTooLarge`] if a frame declares a
    /// length above [`MAX_FRAME_SIZE`]; the connection must then be closed.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, ProtocolCodecError> {
        let pending = &self.buf[self.start..];
        if pending.len() < LEN_PREFIX {
            return Ok(None);
        }
        let len = u32::from_be_bytes([pending[0], pending[1], pending[2], pending[3]]) as usize;
        if len > MAX_FRAME_SIZE {
            return Err(ProtocolCodecError::FrameTooLarge {
                size: len,
                max: MAX_FRAME_SIZE,
            });
        }
        let total = LEN_PREFIX + len;
        if pending.len() < total {
            return Ok(None);
        }
        let payload = pending[LEN_PREFIX..total].to_vec();
        self.start += total;
        self.compact();
        Ok(Some(payload))
    }

    /// Convenience: yield and decode the next complete frame in one step.
    ///
    /// # Errors
    /// As [`FrameDecoder::next_frame`], plus [`ProtocolCodecError::Decode`] if
    /// the payload is not valid MessagePack for `T`.
    pub fn next_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>, ProtocolCodecError> {
        match self.next_frame()? {
            Some(payload) => Ok(Some(decode_payload(&payload)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_decoder() {
        let msg = ("hello".to_string(), 7u32);
        let frame = encode_frame(&msg).unwrap();

        let mut dec = FrameDecoder::new();
        dec.push(&frame);
        let payload = dec.next_frame().unwrap().expect("a full frame");
        let back: (String, u32) = decode_payload(&payload).unwrap();
        assert_eq!(msg, back);
        assert!(dec.next_frame().unwrap().is_none());
    }

    #[test]
    fn decoder_reassembles_split_and_batched_frames() {
        let a = encode_frame(&"first".to_string()).unwrap();
        let b = encode_frame(&"second".to_string()).unwrap();

        let mut dec = FrameDecoder::new();
        // Partial prefix first: nothing yet.
        dec.push(&a[..2]);
        assert!(dec.next_frame().unwrap().is_none());
        // Rest of `a` plus all of `b` in one push: two frames become available.
        dec.push(&a[2..]);
        dec.push(&b);
        let m1: String = dec.next_message().unwrap().unwrap();
        let m2: String = dec.next_message().unwrap().unwrap();
        assert_eq!(m1, "first");
        assert_eq!(m2, "second");
        assert!(dec.next_message::<String>().unwrap().is_none());
    }

    #[test]
    fn encode_rejects_oversized_payload() {
        // A byte vector larger than the cap serializes past MAX_FRAME_SIZE.
        let big = vec![0u8; MAX_FRAME_SIZE + 64];
        let err = encode_frame(&big).unwrap_err();
        assert!(matches!(err, ProtocolCodecError::FrameTooLarge { .. }));
    }

    #[test]
    fn decoder_reassembles_across_a_compaction() {
        // Push more than COMPACT_THRESHOLD of consumed frames so the cursor is
        // reclaimed mid-stream, then verify a frame split across the compaction
        // still reassembles.
        let mut dec = FrameDecoder::new();
        let filler = encode_frame(&"x".repeat(4096)).unwrap();
        let mut consumed = 0usize;
        while consumed < COMPACT_THRESHOLD * 2 {
            dec.push(&filler);
            assert!(dec.next_frame().unwrap().is_some());
            consumed += filler.len();
        }
        assert_eq!(dec.buffered(), 0, "nothing pending after draining");

        let split = encode_frame(&"tail".to_string()).unwrap();
        dec.push(&split[..3]);
        assert!(dec.next_frame().unwrap().is_none());
        dec.push(&split[3..]);
        let back: String = dec.next_message().unwrap().unwrap();
        assert_eq!(back, "tail");
    }

    #[test]
    fn decoder_rejects_oversized_declared_length() {
        let mut dec = FrameDecoder::new();
        // 0xFFFFFFFF declared length is ~4 GiB, far above the cap.
        dec.push(&[0xFF, 0xFF, 0xFF, 0xFF]);
        let err = dec.next_frame().unwrap_err();
        assert!(matches!(
            err,
            ProtocolCodecError::FrameTooLarge { size, max }
                if size == u32::MAX as usize && max == MAX_FRAME_SIZE
        ));
    }

    #[test]
    fn decode_frame_reports_incompleteness() {
        let frame = encode_frame(&"payload".to_string()).unwrap();
        // Only the first two bytes of the prefix.
        let err = decode_frame::<String>(&frame[..2]).unwrap_err();
        assert!(matches!(err, ProtocolCodecError::Incomplete { needed } if needed == 2));

        let (msg, consumed) = decode_frame::<String>(&frame).unwrap();
        assert_eq!(msg, "payload");
        assert_eq!(consumed, frame.len());
    }
}
