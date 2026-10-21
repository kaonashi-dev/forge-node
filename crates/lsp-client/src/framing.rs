use std::io::{self, BufRead, Read};

/// Longest header line accepted, terminator included.
pub(crate) const MAX_HEADER_LINE: usize = 8 * 1024;
const MAX_HEADERS: usize = 32;
/// Bytes of a discarded body kept to find the response id it carried.
const HINT_PREFIX: usize = 256;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    Message(Vec<u8>),
    Oversized { len: usize, prefix: Vec<u8> },
}

/// One `Content-Length` frame, or `None` on EOF at a frame boundary.
///
/// A body longer than `max_body` is read through a sink rather than held, so
/// the stream stays in sync and the claimed length never sizes an allocation.
pub(crate) fn read_frame<R: BufRead>(reader: &mut R, max_body: usize) -> io::Result<Option<Frame>> {
    let mut length = None;
    let mut line = Vec::new();
    for index in 0..=MAX_HEADERS {
        line.clear();
        let read = reader
            .by_ref()
            .take(MAX_HEADER_LINE as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return if index == 0 {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        }
        if !line.ends_with(b"\n") {
            return Err(if line.len() > MAX_HEADER_LINE {
                invalid("header line too long")
            } else {
                io::ErrorKind::UnexpectedEof.into()
            });
        }
        let text = std::str::from_utf8(&line)
            .map_err(|_| invalid("header is not UTF-8"))?
            .trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            let len = length.ok_or_else(|| invalid("missing Content-Length"))?;
            return read_body(reader, len, max_body).map(Some);
        }
        let (name, value) = text
            .split_once(':')
            .ok_or_else(|| invalid("malformed header"))?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            let len = value
                .trim()
                .parse::<usize>()
                .map_err(|_| invalid("malformed Content-Length"))?;
            length = Some(len);
        }
    }
    Err(invalid("too many headers"))
}

fn read_body<R: BufRead>(reader: &mut R, len: usize, max_body: usize) -> io::Result<Frame> {
    if len > max_body {
        let keep = len.min(HINT_PREFIX);
        let mut prefix = Vec::with_capacity(keep);
        reader.by_ref().take(keep as u64).read_to_end(&mut prefix)?;
        let rest = (len - keep) as u64;
        let skipped = io::copy(&mut reader.by_ref().take(rest), &mut io::sink())?;
        if prefix.len() < keep || skipped < rest {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        return Ok(Frame::Oversized { len, prefix });
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    Ok(Frame::Message(body))
}

pub(crate) fn encode(body: &[u8]) -> Vec<u8> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut frame = Vec::with_capacity(header.len() + body.len());
    frame.extend_from_slice(header.as_bytes());
    frame.extend_from_slice(body);
    frame
}

/// The numeric `id` at the head of a discarded body, when that body is a
/// response: a request or notification names its `method`, which a response
/// never does.
pub(crate) fn response_id_hint(prefix: &[u8]) -> Option<i64> {
    if find(prefix, b"\"method\"").is_some() {
        return None;
    }
    let at = find(prefix, b"\"id\"")? + 4;
    let rest = skip_space(&prefix[at..]);
    let rest = skip_space(rest.strip_prefix(b":")?);
    let end = rest
        .iter()
        .enumerate()
        .position(|(i, b)| !(b.is_ascii_digit() || (i == 0 && *b == b'-')))
        .unwrap_or(rest.len());
    std::str::from_utf8(&rest[..end]).ok()?.parse().ok()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn skip_space(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frames(input: &[u8], max: usize) -> Vec<io::Result<Option<Frame>>> {
        let mut cursor = Cursor::new(input.to_vec());
        let mut out = Vec::new();
        loop {
            let frame = read_frame(&mut cursor, max);
            let stop = !matches!(frame, Ok(Some(_)));
            out.push(frame);
            if stop {
                return out;
            }
        }
    }

    #[test]
    fn consecutive_frames_and_a_clean_end() {
        let mut input = encode(br#"{"a":1}"#);
        input.extend_from_slice(b"content-length:2\nContent-Type: x\n\n{}");
        let got = frames(&input, 1024);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0].as_ref().unwrap(),
            &Some(Frame::Message(br#"{"a":1}"#.to_vec()))
        );
        assert_eq!(
            got[1].as_ref().unwrap(),
            &Some(Frame::Message(b"{}".to_vec()))
        );
        assert_eq!(got[2].as_ref().unwrap(), &None);
    }

    #[test]
    fn an_oversized_body_is_skipped_and_the_next_frame_still_parses() {
        let big = format!(
            r#"{{"jsonrpc":"2.0","id":42,"result":"{}"}}"#,
            "x".repeat(5000)
        );
        let mut input = encode(big.as_bytes());
        input.extend_from_slice(&encode(b"[]"));
        let got = frames(&input, 100);
        match got[0].as_ref().unwrap() {
            Some(Frame::Oversized { len, prefix }) => {
                assert_eq!(*len, big.len());
                assert_eq!(prefix.len(), HINT_PREFIX);
                assert_eq!(response_id_hint(prefix), Some(42));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            got[1].as_ref().unwrap(),
            &Some(Frame::Message(b"[]".to_vec()))
        );
    }

    /// A length claiming far more than will ever arrive must fail on EOF,
    /// not reserve the claimed size first.
    #[test]
    fn a_huge_claimed_length_is_never_allocated() {
        let got = frames(b"Content-Length: 18446744073709551615\r\n\r\n{}", 1024);
        assert_eq!(
            got[0].as_ref().unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn malformed_headers_are_errors() {
        let long = format!("X-Pad: {}\r\n", "y".repeat(MAX_HEADER_LINE));
        let many = "X: y\r\n".repeat(MAX_HEADERS + 1);
        for input in [
            b"\r\n{}".to_vec(),
            b"Content-Length: two\r\n\r\n".to_vec(),
            b"garbage\r\n\r\n".to_vec(),
            long.into_bytes(),
            many.into_bytes(),
        ] {
            let got = frames(&input, 1024);
            assert_eq!(
                got[0].as_ref().unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "{:?}",
                String::from_utf8_lossy(&input[..input.len().min(40)])
            );
        }
    }

    #[test]
    fn truncated_input_is_unexpected_eof() {
        for input in [
            &b"Content-Length: 5\r\n"[..],
            b"Content-Length: 5\r\n\r\n{}",
            b"Content-Le",
        ] {
            let got = frames(input, 1024);
            assert_eq!(
                got[0].as_ref().unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
    }

    #[test]
    fn the_id_hint_only_names_responses() {
        assert_eq!(response_id_hint(br#"{"id": -7, "result":"#), Some(-7));
        assert_eq!(response_id_hint(br#"{"jsonrpc":"2.0","id" :12,"#), Some(12));
        assert_eq!(response_id_hint(br#"{"id":3,"method":"x","#), None);
        assert_eq!(response_id_hint(br#"{"id":"str","#), None);
        assert_eq!(response_id_hint(br#"{"result":[1,2,3"#), None);
    }
}
