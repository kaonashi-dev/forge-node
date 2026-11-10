//! Reading a file the user owns without trusting its size.

use std::io::Read as _;
use std::path::Path;

/// `path`'s bytes, refused with `InvalidData` past `cap`. The size is read
/// from the open handle and the read itself is bounded, so a file that grows
/// between the check and the read still cannot pass the cap.
pub fn read(path: &Path, cap: u64) -> std::io::Result<Vec<u8>> {
    let too_big = || std::io::Error::new(std::io::ErrorKind::InvalidData, "file over its budget");
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > cap {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return Err(too_big());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_file_past_its_cap_is_refused_and_one_within_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"12345").unwrap();
        assert_eq!(super::read(&path, 5).unwrap(), b"12345");
        assert_eq!(
            super::read(&path, 4).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }
}
