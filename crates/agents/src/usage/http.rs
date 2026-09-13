//! HTTP seam for OAuth usage sources. Production uses [`UreqClient`]; tests
//! inject a fake that serves fixture bodies, so CI never makes a real request.

use std::io::Read;
use std::time::Duration;

/// Usage JSON is a handful of numbers. A lying endpoint must not grow the
/// daemon without bound (`into_reader` has no cap; `into_string` does).
const MAX_USAGE_BODY: u64 = 1024 * 1024;

/// A minimal GET client. Returns the response body on a 2xx, `None` on anything
/// else — a non-2xx, a timeout, a transport error. Callers treat every `None`
/// the same way: no reading rather than a wrong one.
pub trait HttpClient {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Option<Vec<u8>>;
}

/// The real client, backed by `ureq` with short timeouts so a hung endpoint
/// cannot stall the usage sweep.
pub struct UreqClient {
    agent: ureq::Agent,
}

impl Default for UreqClient {
    fn default() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(3))
            .timeout_read(Duration::from_secs(5))
            .build();
        Self { agent }
    }
}

impl HttpClient for UreqClient {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Option<Vec<u8>> {
        let mut request = self.agent.get(url);
        for (name, value) in headers {
            request = request.set(name, value);
        }
        // ureq returns `Err` for non-2xx by default, so `ok()?` also filters
        // 4xx/5xx. Nothing here logs a header or a token.
        let response = request.call().ok()?;
        read_capped(response.into_reader(), MAX_USAGE_BODY)
    }
}

/// Read at most `cap` bytes. Oversize or IO failure is "no reading".
fn read_capped(reader: impl Read, cap: u64) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut limited = reader.take(cap.saturating_add(1));
    limited.read_to_end(&mut body).ok()?;
    if body.len() as u64 > cap {
        return None;
    }
    Some(body)
}

#[cfg(test)]
pub(super) mod test_support {
    use std::collections::HashMap;

    use super::HttpClient;

    /// Serves canned bodies by URL; any unknown URL yields `None`.
    #[derive(Default)]
    pub(crate) struct FakeHttp {
        responses: HashMap<String, Vec<u8>>,
    }

    impl FakeHttp {
        pub(crate) fn with(url: &str, body: &str) -> Self {
            let mut responses = HashMap::new();
            responses.insert(url.to_owned(), body.as_bytes().to_vec());
            Self { responses }
        }
    }

    impl HttpClient for FakeHttp {
        fn get(&self, url: &str, _headers: &[(&str, &str)]) -> Option<Vec<u8>> {
            self.responses.get(url).cloned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::read_capped;
    use std::io::Cursor;

    #[test]
    fn oversize_body_is_no_reading() {
        let body = vec![0u8; 8];
        assert!(read_capped(Cursor::new(&body), 4).is_none());
        assert_eq!(read_capped(Cursor::new(&body), 8).unwrap().len(), 8);
    }
}
