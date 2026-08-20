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

use std::collections::{BTreeMap, BTreeSet};

use nostr::{Event, Filter};

/// NAP domains this build actually implements.
///
/// Grows a stage at a time. What a napplet is *offered* is the intersection of
/// this with what the user granted — a grant for a domain that does not exist
/// yet must not be advertised, or `shell.supports()` lies and the napplet takes
/// a branch that cannot work.
pub const IMPLEMENTED_DOMAINS: &[&str] = &["shell", "identity", "relay"];

/// Domains every napplet gets, grant or no grant.
///
/// NAP-SHELL is mandatory for a conformant runtime and is not a user decision:
/// it is the handshake itself, and a napplet may assume it is present.
pub const MANDATORY_DOMAINS: &[&str] = &["shell"];

/// Domains a napplet is granted by default when it is installed.
///
/// A napplet is only useful if it can do something, and the manifest's
/// `requires` tags cannot be relied on to say what: they are a statement of
/// intent, and a real napplet published with a toolchain that dropped them
/// arrives declaring nothing at all. Waiting for a napplet to ask for a
/// capability it never declared meant it could never be granted one.
///
/// These are **defaults, not secrets**. The install screen lists every one of
/// them in words before anything is agreed to — a `relay` grant lets a napplet
/// post as you without asking again, and a default that was not shown would be
/// a grant nobody made.
pub const DEFAULT_GRANTS: &[&str] = &["identity", "relay"];

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
    /// Live subscriptions: `subId` to the filters it asked for.
    ///
    /// Without this a subscription is a query wearing a subscription's name —
    /// it answers with what is already stored and then nothing arriving later
    /// has anywhere to be delivered, however well the rest of the system
    /// carries it.
    subscriptions: BTreeMap<String, Vec<Filter>>,
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
            subscriptions: BTreeMap::new(),
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

    /// The domains available to this napplet: everything this build implements.
    ///
    /// **Not** filtered by what the user granted. Every API is injected and
    /// every API answers `shell.supports()` truthfully, because `supports` asks
    /// *"does this runtime do relay?"* — a fact about Myco — and not *"am I
    /// allowed?"*, which is a fact about this napplet and this user, and which
    /// can change while the napplet is running.
    ///
    /// Gating injection on grants looked safer and was worse. A napplet that
    /// finds no `window.napplet.relay` concludes the runtime cannot do relay at
    /// all and takes its permanent fallback path — the DingDong napplet
    /// answered exactly that way, printing "this shell did not grant NAP-RELAY"
    /// and giving up, when what had happened was that nobody had been asked
    /// yet. The permission belongs behind the call, where a refusal is one
    /// failed action the napplet can react to, rather than in the namespace,
    /// where absence is indistinguishable from a runtime that will never
    /// support it.
    pub fn available_domains(&self) -> Vec<String> {
        let mut out: BTreeSet<String> = MANDATORY_DOMAINS.iter().map(|d| d.to_string()).collect();
        out.extend(self.implemented.iter().cloned());
        out.into_iter().collect()
    }

    /// Whether this runtime exposes `domain` at all.
    pub fn offers(&self, domain: &str) -> bool {
        MANDATORY_DOMAINS.contains(&domain) || self.implemented.contains(domain)
    }

    /// Whether the user granted `domain` to this napplet.
    ///
    /// This is the permission, and [`Session::may_service`] is where it is
    /// enforced — on the call, not on the namespace.
    pub fn is_granted(&self, domain: &str) -> bool {
        MANDATORY_DOMAINS.contains(&domain) || self.granted.contains(domain)
    }

    /// Whether a call in `domain` may be serviced right now: the handshake has
    /// happened, the runtime implements it, and the user granted it.
    pub fn may_service(&self, domain: &str) -> bool {
        self.established && self.offers(domain) && self.is_granted(domain)
    }

    /// Register a live subscription, replacing any with the same `subId`.
    pub fn subscribe(&mut self, sub_id: impl Into<String>, filters: Vec<Filter>) {
        self.subscriptions.insert(sub_id.into(), filters);
    }

    /// Drop a subscription. Unknown ids are ignored: a napplet closing twice,
    /// or closing after teardown, is not an error worth reporting.
    pub fn unsubscribe(&mut self, sub_id: &str) {
        self.subscriptions.remove(sub_id);
    }

    /// How many subscriptions are live — for state reporting and tests.
    pub fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    /// The `subId`s whose filters match `event`.
    ///
    /// Returns nothing unless the session is established and still granted
    /// `relay`: a subscription registered before a grant was revoked must stop
    /// delivering, and the check belongs here rather than at each caller, where
    /// forgetting it would leak events to a napplet that may no longer read.
    pub fn matching_subscriptions(&self, event: &Event) -> Vec<String> {
        if !self.may_service("relay") {
            return Vec::new();
        }
        self.subscriptions
            .iter()
            .filter(|(_, filters)| {
                filters
                    .iter()
                    .any(|f| f.match_event(event, nostr::filter::MatchEventOptions::new()))
            })
            .map(|(sub_id, _)| sub_id.clone())
            .collect()
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

    /// `supports()` describes the runtime, not the permission. A domain this
    /// build does not implement is genuinely absent, grant or no grant —
    /// otherwise `supports()` promises something that cannot be delivered.
    #[test]
    fn an_unimplemented_domain_is_absent_even_when_granted() {
        let s = session(&["storage", "notify"]);
        assert!(!s.offers("storage"), "storage is not implemented yet");
        assert!(!s.offers("notify"));
        // Exactly what this build implements — no more, whatever was granted.
        let mut expected: Vec<String> = IMPLEMENTED_DOMAINS.iter().map(|d| d.to_string()).collect();
        expected.sort();
        assert_eq!(s.available_domains(), expected);
    }

    /// The model this whole module turns on: the API is available to every
    /// napplet, and the grant decides whether a *call* goes through.
    ///
    /// Hiding an implemented API from an ungranted napplet tells it the runtime
    /// cannot do the thing at all, and it takes its permanent fallback path
    /// rather than asking.
    #[test]
    fn an_implemented_domain_is_available_before_it_is_granted() {
        let mut s = Session::with_implemented(
            NappletIdentity::new("chat", "aggregate"),
            Vec::<String>::new(),
            ["shell", "relay"],
        );
        s.on_ready();

        // Present in the namespace and truthfully reported...
        assert!(s.offers("relay"));
        assert!(s.available_domains().contains(&"relay".to_string()));
        // ...but the call is refused, which is where the permission lives.
        assert!(!s.is_granted("relay"));
        assert!(!s.may_service("relay"));

        // Granted, the same API starts working — no reload, no re-injection.
        let mut granted = Session::with_implemented(
            NappletIdentity::new("chat", "aggregate"),
            ["relay"],
            ["shell", "relay"],
        );
        granted.on_ready();
        assert!(granted.may_service("relay"));
    }

    /// NAP-SHELL is mandatory, not a user decision — a napplet granted nothing
    /// still gets the handshake.
    #[test]
    fn shell_is_offered_without_a_grant() {
        let s = session(&[]);
        assert!(s.offers("shell"));
        assert!(s.is_granted("shell"));
        assert!(s.available_domains().contains(&"shell".to_string()));
    }

    #[test]
    fn a_domain_this_build_does_not_implement_is_never_offered() {
        let s = session(&[]);
        assert!(!s.offers("storage"));
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
