//! Read a prefix of a file without allocating the rest.
//!
//! `metadata` then `Read::take` so a result file or a spec cannot ask for the
//! whole disk under a request.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CappedBytes {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

/// Copy at most `cap` bytes from `reader`. One extra byte is peeked so the
/// caller can tell a short file from a truncated one. The reader is never
/// asked for more than `cap + 1` bytes.
pub fn read_prefix(reader: impl Read, cap: usize) -> io::Result<CappedBytes> {
    let mut limited = reader.take(cap as u64 + 1);
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes)?;
    let truncated = bytes.len() > cap;
    if truncated {
        let mut end = cap;
        while end > 0 && !is_char_boundary(&bytes, end) {
            end -= 1;
        }
        bytes.truncate(end);
    }
    Ok(CappedBytes { bytes, truncated })
}

/// Open `path` only after `metadata`, then [`read_prefix`].
pub fn read_capped(path: &Path, cap: usize) -> io::Result<CappedBytes> {
    let _meta = std::fs::metadata(path)?;
    let file = File::open(path)?;
    read_prefix(file, cap)
}

fn is_char_boundary(bytes: &[u8], index: usize) -> bool {
    if index >= bytes.len() {
        return true;
    }
    !bytes[index].is_utf8_continuation()
}

trait Utf8Byte {
    fn is_utf8_continuation(&self) -> bool;
}

impl Utf8Byte for u8 {
    fn is_utf8_continuation(&self) -> bool {
        *self & 0b1100_0000 == 0b1000_0000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Read};

    /// Fails if asked for a byte past `max`. A correct `take(cap + 1)` never does.
    struct BudgetReader {
        data: Vec<u8>,
        pos: usize,
        max: usize,
    }

    impl Read for BudgetReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pos >= self.max {
                return Err(io::Error::other("read past the cap before allocation"));
            }
            let n = buf
                .len()
                .min(self.data.len() - self.pos)
                .min(self.max - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn oversize_input_is_clamped_before_the_rest_is_read() {
        let cap = 64;
        let data = vec![b'x'; cap * 20];
        let reader = BudgetReader {
            data,
            pos: 0,
            max: cap + 1,
        };
        let capped = read_prefix(reader, cap).expect("take stops at cap + 1");
        assert!(capped.truncated);
        assert!(capped.bytes.len() <= cap);
        assert!(capped.bytes.iter().all(|byte| *byte == b'x'));
    }
}
