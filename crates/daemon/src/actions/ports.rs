//! Port choice for an action, by bind probe. Runs off the core lock.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use domain::{ActionPort, ACTION_PORT_SPAN};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortChoice {
    Free(u16),
    /// Nothing in range was free; the action launches on its base anyway.
    Taken(u16),
}

impl PortChoice {
    #[must_use]
    pub const fn port(self) -> u16 {
        match self {
            Self::Free(port) | Self::Taken(port) => port,
        }
    }
}

/// `reserved` holds ports live action sessions were given: a server that has
/// not bound yet still owns its port. `prior` is tried first, so a restart
/// keeps the URL a browser tab already has open.
#[must_use]
pub fn choose(spec: ActionPort, reserved: &HashSet<u16>, prior: Option<u16>) -> PortChoice {
    let usable = |port: u16| !reserved.contains(&port) && is_free(port);
    if let Some(prior) = prior.filter(|port| in_range(spec, *port)) {
        if usable(prior) {
            return PortChoice::Free(prior);
        }
    }
    if !spec.per_worktree {
        return if usable(spec.base) {
            PortChoice::Free(spec.base)
        } else {
            PortChoice::Taken(spec.base)
        };
    }
    (0..ACTION_PORT_SPAN)
        .filter_map(|offset| spec.base.checked_add(offset))
        .find(|port| usable(*port))
        .map_or(PortChoice::Taken(spec.base), PortChoice::Free)
}

fn in_range(spec: ActionPort, port: u16) -> bool {
    if spec.per_worktree {
        port >= spec.base && u32::from(port) < u32::from(spec.base) + u32::from(ACTION_PORT_SPAN)
    } else {
        port == spec.base
    }
}

/// Both loopbacks: Node resolves `localhost` to `::1` first on macOS, so a dev
/// server is as likely to be on IPv6 only as on IPv4 only.
const LOOPBACKS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

/// A refused loopback connect returns at once; this only bounds a wedged one.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(250);

/// Whether something accepts connections on either loopback.
#[must_use]
pub fn answers(port: u16) -> bool {
    LOOPBACKS
        .iter()
        .any(|ip| TcpStream::connect_timeout(&SocketAddr::new(*ip, port), CONNECT_TIMEOUT).is_ok())
}

/// Whether the port can be bound on both loopbacks and nothing answers on it.
/// The connect catches a wildcard listener, which `SO_REUSEADDR` lets a
/// loopback bind coexist with; a host without IPv6 does not count as taken.
fn is_free(port: u16) -> bool {
    let bindable = LOOPBACKS
        .iter()
        .all(|ip| match TcpListener::bind((*ip, port)) {
            Ok(_) => true,
            Err(error) => ip.is_ipv6() && error.kind() == ErrorKind::AddrNotAvailable,
        });
    bindable && !answers(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held() -> (TcpListener, u16) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    #[test]
    fn a_port_held_on_ipv6_loopback_only_is_taken() {
        let Ok(listener) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)) else {
            return;
        };
        let port = listener.local_addr().unwrap().port();
        let spec = ActionPort {
            base: port,
            per_worktree: false,
        };
        assert_eq!(choose(spec, &HashSet::new(), None), PortChoice::Taken(port));
        assert!(answers(port), "the readiness probe sees it too");
    }

    #[test]
    fn a_fixed_port_that_is_taken_is_reported_not_moved() {
        let (_hold, port) = held();
        let spec = ActionPort {
            base: port,
            per_worktree: false,
        };
        assert_eq!(choose(spec, &HashSet::new(), None), PortChoice::Taken(port));
    }

    #[test]
    fn a_per_worktree_port_skips_bound_and_reserved_ports() {
        let (_hold, port) = held();
        let spec = ActionPort {
            base: port,
            per_worktree: true,
        };
        let reserved: HashSet<u16> = [port + 1].into();
        let chosen = choose(spec, &reserved, None);
        assert!(
            matches!(chosen, PortChoice::Free(p) if p >= port + 2),
            "{chosen:?}"
        );
    }

    #[test]
    fn a_free_prior_port_is_kept() {
        let (hold, port) = held();
        drop(hold);
        let spec = ActionPort {
            base: port.saturating_sub(3),
            per_worktree: true,
        };
        assert_eq!(
            choose(spec, &HashSet::new(), Some(port)),
            PortChoice::Free(port)
        );
    }
}
