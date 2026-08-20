//! One napplet's live session: who it is, what it may use, and whether the
//! handshake has happened yet.
//!
//! The session is the enforcement point. NAP-SHELL puts it plainly: by
//! withholding `shell.init`, a runtime denies a napplet every capability at
//! once. So everything a napplet is allowed to do is decided here, from two
//! inputs it does not control — the identity assigned at creation from verified
//! bytes, and the grants recorded on its library entry at install review.
//!
//! Nothing the napplet says over the wire feeds either one. `shell.ready`
//! carries no payload by design, precisely so there is nothing in it for a
//! runtime to be tricked into trusting.

use std::collections::BTreeSet;

/// NAP domains this build actually implements.
///
/// Grows a stage at a time. What a napplet is *offered* is the intersection of
/// this with what the user granted — a grant for a domain that does not exist
/// yet must not be advertised, or `shell.supports()` lies and the napplet takes
/// a branch that cannot work.
pub const IMPLEMENTED_DOMAINS: &[&str] = &["shell"];

/// Domains every napplet gets, grant or no grant.
///
/// NAP-SHELL is mandatory for a conformant runtime and is not a user decision:
/// it is the handshake itself, and a napplet may assume it is present.
pub const MANDATORY_DOMAINS: &[&str] = &["shell"];

/// A napplet's identity: the `(dTag, aggregateHash)` tuple, computed by the
/// runtime from verified bytes.
///
/// Assigned at creation and never negotiated. `d_tag` is empty for root and
/// snapshot manifests, which have none.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NappletIdentity {
    pub d_tag: String,
    pub aggregate: String,
}

impl NappletIdentity {
    pub fn new(d_tag: impl Into<String>, aggregate: impl Into<String>) -> Self {
        Self {
            d_tag: d_tag.into(),
            aggregate: aggregate.into(),
        }
    }
}

impl From<&crate::resolve::ResolvedNapplet> for NappletIdentity {
    fn from(resolved: &crate::resolve::ResolvedNapplet) -> Self {
        Self::new(resolved.d_tag.clone(), resolved.aggregate.clone())
    }
}

/// A napplet's live session.
#[derive(Debug, Clone)]
pub struct Session {
    identity: NappletIdentity,
    /// Domains the user granted at install review. Kept separate from what is
    /// offered so a grant survives a build that has not implemented it yet.
    granted: BTreeSet<String>,
    /// Domains this runtime implements. Held per session rather than read from
    /// a global so the refusal paths can be exercised for domains that are not
    /// wired up yet — and so a harness can stand up a runtime offering a
    /// deliberately narrow set.
    implemented: BTreeSet<String>,
    established: bool,
}

impl Session {
    /// Open a session for a napplet, with the grants recorded on its library
    /// entry. Not yet established — that happens on the first `shell.ready`.
    pub fn new(
        identity: NappletIdentity,
        granted: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::with_implemented(identity, granted, IMPLEMENTED_DOMAINS.iter().copied())
    }

    /// As [`Session::new`], over an explicit set of implemented domains.
    pub fn with_implemented(
        identity: NappletIdentity,
        granted: impl IntoIterator<Item = impl Into<String>>,
        implemented: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            identity,
            granted: granted.into_iter().map(Into::into).collect(),
            implemented: implemented.into_iter().map(Into::into).collect(),
            established: false,
        }
    }

    /// Whether this runtime implements `domain` at all — regardless of grants.
    pub fn implements(&self, domain: &str) -> bool {
        self.implemented.contains(domain)
    }

    pub fn identity(&self) -> &NappletIdentity {
        &self.identity
    }

    /// Whether the handshake has completed. Until it has, no capability call is
    /// serviceable — NAP-SHELL requires the runtime refuse them.
    pub fn is_established(&self) -> bool {
        self.established
    }

    /// The domains offered to this napplet: what it was granted, intersected
    /// with what this build implements, plus the mandatory ones.
    ///
    /// This is the set `shell.supports()` answers from, and it has to be
    /// truthful in both directions — `true` for everything offered, `false` for
    /// everything else.
    pub fn offered_domains(&self) -> Vec<String> {
        let mut out: BTreeSet<String> = MANDATORY_DOMAINS.iter().map(|d| d.to_string()).collect();
        for domain in &self.granted {
            if self.implemented.contains(domain) {
                out.insert(domain.clone());
            }
        }
        out.into_iter().collect()
    }

    /// Whether `domain` is offered to this napplet.
    pub fn offers(&self, domain: &str) -> bool {
        MANDATORY_DOMAINS.contains(&domain)
            || (self.granted.contains(domain) && self.implemented.contains(domain))
    }

    /// Whether a call in `domain` may be serviced right now.
    pub fn may_service(&self, domain: &str) -> bool {
        self.established && self.offers(domain)
    }

    /// Record the napplet's readiness signal.
    ///
    /// Returns `true` the first time, meaning `shell.init` should be sent.
    /// Every later call returns `false`: NAP-SHELL requires a duplicate
    /// `shell.ready` be idempotent — no second session, no overwrite of the
    /// first, no resent environment. That is what stops a napplet replaying the
    /// signal to escalate, re-key, or re-scope itself.
    pub fn on_ready(&mut self) -> bool {
        if self.established {
            return false;
        }
        self.established = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(granted: &[&str]) -> Session {
        Session::new(
            NappletIdentity::new("chat", "aggregate-hex"),
            granted.to_vec(),
        )
    }

    #[test]
    fn no_capability_is_serviceable_before_the_handshake() {
        let s = session(&["shell"]);
        assert!(!s.is_established());
        assert!(!s.may_service("shell"));
    }

    #[test]
    fn the_first_ready_establishes_the_session_and_asks_for_init() {
        let mut s = session(&[]);
        assert!(s.on_ready(), "the first ready must trigger shell.init");
        assert!(s.is_established());
        assert!(s.may_service("shell"));
    }

    /// A replayed `shell.ready` must not re-establish anything, or a napplet
    /// could use it to re-scope its own session.
    #[test]
    fn a_replayed_ready_changes_nothing_and_resends_nothing() {
        let mut s = session(&[]);
        assert!(s.on_ready());
        for _ in 0..5 {
            assert!(!s.on_ready(), "shell.init must be sent exactly once");
        }
        assert!(s.is_established());
    }

    /// The truthfulness rule: a grant for a domain this build does not
    /// implement must not be advertised, or `supports()` promises something
    /// that cannot be delivered.
    #[test]
    fn a_grant_for_an_unimplemented_domain_is_not_offered() {
        let s = session(&["relay", "storage"]);
        assert!(!s.offers("relay"));
        assert!(!s.offers("storage"));
        assert_eq!(s.offered_domains(), vec!["shell".to_string()]);
    }

    /// NAP-SHELL is mandatory, not a user decision — a napplet granted nothing
    /// still gets the handshake.
    #[test]
    fn shell_is_offered_without_a_grant() {
        let s = session(&[]);
        assert!(s.offers("shell"));
        assert!(s.offered_domains().contains(&"shell".to_string()));
    }

    #[test]
    fn an_ungranted_domain_is_never_offered() {
        let s = session(&[]);
        assert!(!s.offers("relay"));
        assert!(!s.offers("nonsense"));
        assert!(!s.offers(""));
    }

    /// Identity comes from the verified bytes and nothing else touches it.
    #[test]
    fn the_handshake_does_not_change_identity() {
        let mut s = session(&[]);
        let before = s.identity().clone();
        s.on_ready();
        assert_eq!(s.identity(), &before);
    }
}
