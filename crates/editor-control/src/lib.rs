//! The control channel between the daemon and one `forge-editor` process.
//!
//! The PTY carries keystrokes and ANSI output; this channel carries metadata:
//! the buffer the daemon opened for the editor, reveal requests, and the
//! editor's state. It deliberately has no dependency on `protocol`, `daemon` or
//! `client`: the editor binary stays standalone, and the daemon decodes these
//! frames itself.
//!
//! Frames are a `u32` big-endian length plus a MessagePack payload, like the
//! client wire but with their own smaller cap: the declared length is checked
//! against [`MAX_CONTROL_FRAME`] before any buffer is reserved.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::{self, Read, Write};

pub mod message;
pub mod view;

pub use message::{
    version_matches, DaemonMessage, EditorMessage, EditorStateWire, WireDiagnostic, WireEdit,
    WireMark, WireMarkKind, WirePlace, WireSeverity, MAX_CARET_LINE_BYTES, MAX_PLACES,
};
pub use view::{
    modifiers, EditorInput, ViewFrame, ViewRequest, ViewRow, WireCaret, WireDecoration,
    WireDecorationKind, WireFold, WireKey, WireMouseKind, WireRange, WireScope, WireSeverityLevel,
    WireSpan, MAX_CARETS, MAX_DECORATIONS, MAX_INPUT_EVENTS, MAX_ROW_BYTES, MAX_ROW_SPANS,
    MAX_VIEW_FRAME_BYTES, MAX_VIEW_ROWS, SPAN_OVERHEAD_BYTES, VIEW_OVERSCAN, VIEW_ROW_BUDGET,
};

/// Version of the control wire. The handshake refuses any other value, the way
/// `protocol::PROTOCOL_VERSION` does for the client wire.
pub const CONTROL_VERSION: u16 = 4;

/// Hard cap for one decoded control frame.
///
/// Four times the document budget: an `Open` carries the whole file, every
/// other message is small. Checked against the declared length **before** the
/// payload is allocated (`docs/performance.md`: cap before allocate).
pub const MAX_CONTROL_FRAME: usize = 4 * 1024 * 1024;

/// Document budget shared by the editor and the daemon.
///
/// Must equal `editor_core::limits::MAX_DOCUMENT_BYTES` and
/// `fs_service::MAX_FILE_BYTES`; `editor-cli` unit-tests the first equality and
/// `crates/daemon` uses `fs-service` for the real read, so the three cannot
/// drift apart silently.
pub const MAX_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;

/// Why a control message could not travel.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// The socket itself failed.
    #[error("control io: {0}")]
    Io(#[from] io::Error),
    /// A peer declared (or tried to send) a frame over the cap.
    #[error("control frame of {len} bytes exceeds the {MAX_CONTROL_FRAME}-byte cap")]
    FrameTooLarge { len: usize },
    /// The payload could not be serialized.
    #[error("control encode: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    /// The payload was not a message this build understands.
    #[error("control decode: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
}

/// Encode one message as a framed payload.
pub fn encode<M: Serialize>(message: &M) -> Result<Vec<u8>, ControlError> {
    let payload = rmp_serde::to_vec_named(message)?;
    if payload.len() > MAX_CONTROL_FRAME {
        return Err(ControlError::FrameTooLarge { len: payload.len() });
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Write one message as a length-prefixed frame.
pub fn write_frame<M: Serialize>(writer: &mut impl Write, message: &M) -> Result<(), ControlError> {
    writer.write_all(&encode(message)?)?;
    Ok(())
}

/// Read one message, refusing an oversize declaration before reserving.
pub fn read_frame<M: DeserializeOwned>(reader: &mut impl Read) -> Result<M, ControlError> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    // The cap is checked on the *declared* length: a hostile 3 GiB prefix must
    // not turn into a 3 GiB allocation.
    if len > MAX_CONTROL_FRAME {
        return Err(ControlError::FrameTooLarge { len });
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok(rmp_serde::from_slice(&payload)?)
}

/// Retains partial headers and payloads when a read deadline expires.
#[derive(Default)]
pub struct FrameReader {
    header: [u8; 4],
    header_read: usize,
    payload: Vec<u8>,
    payload_read: usize,
}

impl FrameReader {
    pub fn read<M: DeserializeOwned>(&mut self, reader: &mut impl Read) -> Result<M, ControlError> {
        read_remaining(reader, &mut self.header, &mut self.header_read)?;
        let len = u32::from_be_bytes(self.header) as usize;
        if len > MAX_CONTROL_FRAME {
            return Err(ControlError::FrameTooLarge { len });
        }
        self.payload.resize(len, 0);
        read_remaining(reader, &mut self.payload, &mut self.payload_read)?;
        let message = rmp_serde::from_slice(&self.payload)?;
        self.header_read = 0;
        self.payload_read = 0;
        self.payload.clear();
        Ok(message)
    }
}

fn read_remaining(reader: &mut impl Read, bytes: &mut [u8], read: &mut usize) -> io::Result<()> {
    while *read < bytes.len() {
        match reader.read(&mut bytes[*read..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => *read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::{WireMark, WireMarkKind};
    use std::io::Cursor;

    /// A reader whose header can declare a length its body never backs; any
    /// read past the 4-byte header is an error, so a decoder that reserves
    /// before checking the cap fails here.
    struct HeaderOnly {
        header: [u8; 4],
        read: bool,
    }

    impl Read for HeaderOnly {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            assert!(!self.read, "read the body before checking the cap");
            self.read = true;
            buf[..4].copy_from_slice(&self.header);
            Ok(4)
        }
    }

    #[test]
    fn a_deadline_preserves_every_partial_header_and_payload() {
        struct UntilDeadline<'a>(&'a [u8]);
        impl Read for UntilDeadline<'_> {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                if self.0.is_empty() {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                self.0.read(out)
            }
        }
        let message = EditorMessage::SaveRequest {
            request_id: 42,
            text: "draft".repeat(100),
            document_version: 3,
        };
        let frame = encode(&message).unwrap();
        for split in 1..frame.len() {
            let mut decoder = FrameReader::default();
            assert!(
                matches!(decoder.read::<EditorMessage>(&mut UntilDeadline(&frame[..split])),
                Err(ControlError::Io(error)) if error.kind() == io::ErrorKind::TimedOut)
            );
            let mut tail = &frame[split..];
            assert_eq!(decoder.read::<EditorMessage>(&mut tail).unwrap(), message);
            assert_eq!(
                decoder
                    .read::<EditorMessage>(&mut frame.as_slice())
                    .unwrap(),
                message
            );
        }
    }

    #[test]
    fn every_message_round_trips() {
        let daemon_messages = vec![
            DaemonMessage::Retarget {
                path: "moved/file.rs".into(),
            },
            DaemonMessage::Welcome {
                version: CONTROL_VERSION,
                session_id: "s-1".into(),
                buffer_id: 7,
            },
            DaemonMessage::Open {
                request_id: 1,
                buffer_id: 7,
                path: "src/main.rs".into(),
                text: "fn main() {}\n".into(),
                revision: Some("abc".into()),
                line: Some(3),
                read_only: true,
                autosave: false,
            },
            DaemonMessage::SetAutosave {
                request_id: 8,
                autosave: true,
            },
            DaemonMessage::Reveal {
                request_id: 2,
                line: 10,
                column: Some(4),
            },
            DaemonMessage::GetState { request_id: 3 },
            DaemonMessage::GitMarks {
                request_id: 7,
                marks: vec![WireMark {
                    line: 12,
                    kind: WireMarkKind::Modified,
                }],
            },
            DaemonMessage::Saved {
                request_id: 5,
                revision: "9f8e7d6c".into(),
            },
            DaemonMessage::SaveRefused {
                request_id: 6,
                reason: "the file changed on disk".into(),
            },
            DaemonMessage::ApplyPreviewEdit {
                request_id: 4,
                expected_document_version: 5,
                edits: vec![WireEdit {
                    from: 0,
                    to: 2,
                    insert: "hi".into(),
                }],
            },
        ];
        for message in &daemon_messages {
            let mut frame = Cursor::new(Vec::new());
            write_frame(&mut frame, message).unwrap();
            frame.set_position(0);
            assert_eq!(&read_frame::<DaemonMessage>(&mut frame).unwrap(), message);
        }

        let editor_messages = vec![
            EditorMessage::Hello {
                version: CONTROL_VERSION,
                session_id: "s-1".into(),
                pid: 42,
            },
            EditorMessage::Opened {
                request_id: 1,
                document_version: 1,
            },
            EditorMessage::Revealed { request_id: 2 },
            EditorMessage::Applied {
                request_id: 4,
                document_version: 6,
            },
            EditorMessage::Refused {
                request_id: 4,
                reason: "read-only".into(),
            },
            EditorMessage::SaveRequest {
                request_id: 5,
                text: "fn main() {}\n".into(),
                document_version: 3,
            },
            EditorMessage::State {
                request_id: None,
                state: EditorStateWire {
                    path: "src/main.rs".into(),
                    line: 1,
                    column: 1,
                    dirty: false,
                    read_only: true,
                    document_version: 1,
                    top_line: 1,
                    visible_lines: 24,
                    total_lines: 200,
                    caret_line: "fn main() {}".into(),
                    selection_length: 0,
                    cursor_count: 1,
                    status: String::new(),
                },
            },
            EditorMessage::State {
                request_id: Some(3),
                state: EditorStateWire::default(),
            },
            EditorMessage::Closed {
                reason: "quit".into(),
            },
        ];
        for message in &editor_messages {
            let mut frame = Cursor::new(Vec::new());
            write_frame(&mut frame, message).unwrap();
            frame.set_position(0);
            assert_eq!(&read_frame::<EditorMessage>(&mut frame).unwrap(), message);
        }
    }

    #[test]
    fn a_frame_over_the_cap_is_refused_before_reading() {
        let mut reader = HeaderOnly {
            header: ((MAX_CONTROL_FRAME + 1) as u32).to_be_bytes(),
            read: false,
        };
        match read_frame::<EditorMessage>(&mut reader) {
            Err(ControlError::FrameTooLarge { len }) => assert_eq!(len, MAX_CONTROL_FRAME + 1),
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn a_foreign_version_is_refused() {
        assert!(message::version_matches(CONTROL_VERSION));
        assert!(!message::version_matches(CONTROL_VERSION - 1));
        assert!(!message::version_matches(CONTROL_VERSION + 1));
    }

    #[test]
    fn the_document_budget_is_two_mebibytes() {
        // One place asserts the shared number; the editor-side test ties it to
        // `editor_core::limits`.
        assert_eq!(MAX_DOCUMENT_BYTES, 2 * 1024 * 1024);
    }
}
