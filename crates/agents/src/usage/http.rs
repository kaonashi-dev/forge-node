//! A tiny HTTP seam so the OAuth usage sources can be tested without a network
//! (§16.2). Production uses [`UreqClient`]; tests inject a fake that serves
//! fixture bodies, so CI never makes a real request.

use std::io::Read;
use std::time::Duration;

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
        let mut body = Vec::new();
        response.into_reader().read_to_end(&mut body).ok()?;
        Some(body)
    }
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
