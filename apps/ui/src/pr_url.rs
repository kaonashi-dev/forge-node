//! A pasted pull-request link, read into what the daemon asks GitHub for.
//! Whether the host is one Forge may ask is the daemon's call, not this one's.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestLink {
    pub host: String,
    /// `owner/name`.
    pub repository: String,
    pub number: u32,
}

impl PullRequestLink {
    /// Matches a cached pull request the way GitHub does, ignoring case.
    #[must_use]
    pub fn names(&self, pr: &domain::PullRequest) -> bool {
        self.is(&pr.host, &pr.repository, pr.number)
    }

    /// The daemon coalesces reads case-insensitively and echoes the first
    /// request's spelling, so an answer is matched the same way.
    #[must_use]
    pub fn is(&self, host: &str, repository: &str, number: u32) -> bool {
        number == self.number
            && host.eq_ignore_ascii_case(&self.host)
            && repository.eq_ignore_ascii_case(&self.repository)
    }
}

const SHAPE: &str = "Paste a link like https://github.com/owner/repo/pull/123";

/// `https://<host>/<owner>/<repo>/pull/<n>`, with any tab of the pull request
/// (`/files`, `/commits`, `/checks`), a query, a fragment or a trailing slash.
///
/// # Errors
/// A sentence for the person when the text is not such a link.
pub fn parse(input: &str) -> Result<PullRequestLink, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(SHAPE.into());
    }
    let rest = input
        .strip_prefix("https://")
        .ok_or_else(|| format!("{SHAPE}; the link must start with https://"))?;
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let mut parts = rest.split('/');
    let host = parts.next().unwrap_or_default().to_ascii_lowercase();
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    let kind = parts.next().unwrap_or_default();
    let number = parts.next().unwrap_or_default();
    let tab = parts.next().unwrap_or_default();
    let tail_is_empty = parts.all(str::is_empty);
    let host_ok = !host.is_empty() && !host.contains(['@', ':']);
    let tab_ok = matches!(tab, "" | "files" | "commits" | "checks") && tail_is_empty;
    if !host_ok || owner.is_empty() || name.is_empty() || kind != "pull" || !tab_ok {
        return Err(SHAPE.into());
    }
    let number = number
        .parse::<u32>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| format!("{SHAPE}; the number after /pull/ is missing"))?;
    Ok(PullRequestLink {
        host,
        repository: format!("{owner}/{name}"),
        number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(host: &str, repository: &str, number: u32) -> PullRequestLink {
        PullRequestLink {
            host: host.into(),
            repository: repository.into(),
            number,
        }
    }

    #[test]
    fn a_pull_request_link_is_read_from_any_of_its_tabs() {
        let expected = link("github.com", "Acme/Widgets", 42);
        for input in [
            "https://github.com/Acme/Widgets/pull/42",
            "  https://github.com/Acme/Widgets/pull/42/  \n",
            "https://GitHub.com/Acme/Widgets/pull/42/files",
            "https://github.com/Acme/Widgets/pull/42/commits/",
            "https://github.com/Acme/Widgets/pull/42/checks?check_run_id=1",
            "https://github.com/Acme/Widgets/pull/42#issuecomment-9",
            "https://github.com/Acme/Widgets/pull/42?w=1#diff",
        ] {
            assert_eq!(parse(input), Ok(expected.clone()), "{input:?}");
        }
        assert_eq!(
            parse("https://ghe.example.test/team/svc/pull/7"),
            Ok(link("ghe.example.test", "team/svc", 7))
        );
    }

    #[test]
    fn anything_else_is_refused_with_a_sentence() {
        for input in [
            "",
            "github.com/Acme/Widgets/pull/42",
            "http://github.com/Acme/Widgets/pull/42",
            "https://github.com/Acme/Widgets/issues/42",
            "https://github.com/Acme/Widgets/pull/",
            "https://github.com/Acme/Widgets/pull/0",
            "https://github.com/Acme/Widgets/pull/abc",
            "https://github.com/Acme/Widgets/pull/42/files/extra",
            "https://github.com/Acme/Widgets/pull/42/blame",
            "https://github.com/Acme/pull/42",
            "https://user@github.com/Acme/Widgets/pull/42",
            "https://github.com:443/Acme/Widgets/pull/42",
        ] {
            let error = parse(input).expect_err(input);
            assert!(error.starts_with("Paste a link"), "{input:?}: {error}");
        }
    }
}
