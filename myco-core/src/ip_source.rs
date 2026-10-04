//! `IpPeerSource` — the **online fallback** [`PeerSource`]: fetch an externally-
//! authored nsite from **public** relays + Blossom over normal IP. This is the
//! tier-3 source in `docs/design/nsite/nsite-layer.md` §5 and, in P2, the way content
//! enters the device: a user pastes `<npub>.nsite.lol` (or a bare npub) and Myco
//! downloads the signed manifest + blobs, verifies, and mirrors them locally so
//! the site then serves offline forever. The FIPS-peer source (P3) implements the
//! same trait; the sync engine doesn't care which.
//!
//! Gated by `sync.offline_only`: when set, no IP source is installed and Myco
//! never reaches the internet (`docs/reference/config.md`).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::future::join_all;
use futures_util::StreamExt;
use nostr::{Event, PublicKey};
use nsite_deck::seams::PeerSource;
use nsite_deck::{kind_for, sha256_hex};

/// A small, sensible default set of public relays that carry nsite manifests.
///
/// The fallback, not the plan: a lookup asks the pointer's hints and the
/// author's own NIP-65 relays too (see [`AuthorOutbox`]). `relay.ditto.pub`
/// carries most published napplets and nsites. `relay.nostr.band` is gone —
/// it stopped accepting connections — and a dead relay here costs every
/// lookup that waits for all relays its full timeout. Indexers such as
/// `purplepag.es` are not here: they hold profiles and relay lists, not apps,
/// and are asked for those alone ([`indexer_relays`]).
pub fn default_relays() -> Vec<String> {
    [
        "wss://relay.damus.io",
        "wss://nos.lol",
        "wss://relay.primal.net",
        "wss://relay.ditto.pub",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Indexer relays: they specialise in profiles and relay lists (kinds 0,
/// 3, 10002), not in apps. Asked for an author's list only when it is not
/// stored here yet, and published to with the user's own profile and lists
/// so other clients find them.
pub fn indexer_relays() -> Vec<String> {
    [
        "wss://purplepag.es",
        "wss://index.hzrd149.com",
        "wss://indexer.coracle.social",
        "wss://user.kindpag.es",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// At most this many of an author's NIP-65 write relays are asked, in the
/// order the list gives them. A list can name dozens; the first few are the
/// ones the author put first, and a lookup must stay a handful of sockets.
pub const MAX_AUTHOR_RELAYS: usize = 5;

/// How long [`AuthorOutbox::fetch_lists`] waits for the relays it asks.
pub(crate) const LIST_FETCH_TIMEOUT: Duration = Duration::from_secs(4);

/// At most this many authors in one relay-list filter; more go in another
/// filter of the same `REQ`.
pub(crate) const LIST_AUTHORS_PER_FILTER: usize = 100;

/// How long an author with no relay list anywhere is remembered as such, so
/// the indexers are not asked about them on every lookup — and asked again
/// soon enough that a list published today is found today.
pub const LIST_MISS_REMEMBERED_FOR: Duration = Duration::from_secs(10 * 60);

/// How long a lookup that was not a clean "no" — a relay timed out, or the
/// napplet named relays of its own — keeps the author from being looked up
/// again. Not a verdict, just not every call.
pub const LIST_SOFT_MISS_FOR: Duration = Duration::from_secs(60);

/// Authors looked for and not found, with until when that holds.
/// Process-wide: sources are built per lookup, and the answer is about the
/// author, not the source.
fn list_misses(
) -> &'static std::sync::Mutex<std::collections::HashMap<PublicKey, std::time::Instant>> {
    static MISSES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PublicKey, std::time::Instant>>,
    > = std::sync::OnceLock::new();
    MISSES.get_or_init(Default::default)
}

/// Whether two relay URLs name the same relay: equal but for a trailing
/// slash, which relay lists spell both ways.
pub fn same_relay(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Append `url` unless an equal relay (see [`same_relay`]) is already there.
fn push_relay(out: &mut Vec<String>, url: &str) {
    if !out.iter().any(|r| same_relay(r, url)) {
        out.push(url.to_string());
    }
}

/// The relays a manifest lookup asks, in order: the pointer's own `hints`,
/// then the `author`'s NIP-65 write relays, then the `defaults`. Deduplicated
/// by URL, first spelling kept.
///
/// Hints lead because whoever made the pointer knew where the manifest is;
/// the author's list is where the author says they publish; the defaults are
/// a guess that covers the common case.
pub fn lookup_relays(hints: &[String], author: &[String], defaults: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for url in hints.iter().chain(author).chain(defaults) {
        push_relay(&mut out, url);
    }
    out
}

/// An author's NIP-65 relay list (kind 10002), for finding their manifests.
///
/// The local relay is the cache: a list is read from it first, and one found
/// on the network is stored there, so the second lookup for an author costs
/// no round trip. [`IpPeerSource::with_author_outbox`] is how a lookup uses
/// it; the update check reads stored lists with
/// [`AuthorOutbox::stored_write_relays`] and fetches missing ones beside it.
///
/// A relay list is signed by its author, not trusted. Only public `ws(s)://`
/// URLs are used — `crate::outbox`'s gate, which refuses `.fips` mesh URLs,
/// userinfo and private hosts — at most [`MAX_AUTHOR_RELAYS`] of them, and
/// each is resolved and refused if its name points at a private address
/// before it is dialled (`outbox::dials_public`).
pub struct AuthorOutbox {
    store: std::sync::Arc<dyn nsite_deck::seams::RelayBackend>,
    indexers: Vec<String>,
    /// Off only in host tests, whose "public relays" are mocks on 127.0.0.1.
    guard_private_dials: bool,
}

impl AuthorOutbox {
    pub fn new(store: std::sync::Arc<dyn nsite_deck::seams::RelayBackend>) -> Self {
        Self {
            store,
            indexers: indexer_relays(),
            guard_private_dials: true,
        }
    }

    /// Ask these relays for missing lists instead of [`indexer_relays`] —
    /// for tests, which must never reach the internet.
    #[cfg(test)]
    pub fn with_indexers(mut self, indexers: Vec<String>) -> Self {
        self.indexers = indexers;
        self
    }

    /// Accept loopback relays in a list and dial them — for tests only.
    #[cfg(test)]
    pub fn allowing_private_dials(mut self) -> Self {
        self.guard_private_dials = false;
        self
    }

    /// The author's newest stored relay list, if this device has one.
    pub async fn stored_list(&self, author: &PublicKey) -> Option<Event> {
        let filter = nostr::Filter::new()
            .kind(nostr::Kind::RelayList)
            .author(*author)
            .limit(1);
        self.store
            .query(&[filter])
            .await
            .ok()?
            .into_iter()
            .filter(|e| e.kind == nostr::Kind::RelayList && e.pubkey == *author)
            .max_by_key(|e| e.created_at)
    }

    /// Each of `authors`' newest stored relay list, read in **one** query
    /// rather than one per author: a check over a follow list of hundreds
    /// made that many store reads in a row.
    pub async fn stored_lists_for(
        &self,
        authors: &[PublicKey],
    ) -> std::collections::HashMap<PublicKey, Event> {
        let mut out: std::collections::HashMap<PublicKey, Event> = std::collections::HashMap::new();
        if authors.is_empty() {
            return out;
        }
        let filter = nostr::Filter::new()
            .kind(nostr::Kind::RelayList)
            .authors(authors.iter().copied());
        let Ok(found) = self.store.query(&[filter]).await else {
            return out;
        };
        for e in found {
            if e.kind != nostr::Kind::RelayList || !authors.contains(&e.pubkey) {
                continue;
            }
            match out.get(&e.pubkey) {
                Some(have) if have.created_at >= e.created_at => {}
                _ => {
                    out.insert(e.pubkey, e);
                }
            }
        }
        out
    }

    /// Keep a list fetched from the network. The local relay keeps the newest
    /// of a replaceable kind on its own, so an older one is a no-op.
    pub async fn remember(&self, list: Event) {
        list_misses().lock().unwrap().remove(&list.pubkey);
        if let Err(e) = self.store.publish(list).await {
            tracing::debug!(error = %e, "could not store an author's relay list");
        }
    }

    /// Whether `author` was looked for recently and had no list anywhere.
    pub fn recently_missed(&self, author: &PublicKey) -> bool {
        list_misses()
            .lock()
            .unwrap()
            .get(author)
            .is_some_and(|until| std::time::Instant::now() < *until)
    }

    /// Remember that `author` had no list on any relay asked.
    pub fn note_miss(&self, author: &PublicKey) {
        self.miss_for(author, LIST_MISS_REMEMBERED_FOR);
    }

    /// Remember, briefly, that a lookup for `author` found nothing but was
    /// not a clean "no" ([`LIST_SOFT_MISS_FOR`]). A longer miss already
    /// held is kept.
    pub fn note_soft_miss(&self, author: &PublicKey) {
        self.miss_for(author, LIST_SOFT_MISS_FOR);
    }

    fn miss_for(&self, author: &PublicKey, how_long: Duration) {
        let until = std::time::Instant::now() + how_long;
        let mut misses = list_misses().lock().unwrap();
        let held = misses.entry(*author).or_insert(until);
        *held = (*held).max(until);
    }

    /// How much longer `author` counts as missed — for tests.
    #[cfg(test)]
    pub fn miss_left(&self, author: &PublicKey) -> Option<Duration> {
        list_misses()
            .lock()
            .unwrap()
            .get(author)
            .and_then(|until| until.checked_duration_since(std::time::Instant::now()))
    }

    /// The indexer relays this outbox asks for missing lists.
    pub fn indexers(&self) -> &[String] {
        &self.indexers
    }

    /// The relays `list` says its author writes to — `r` tags with no marker
    /// or `write` — that may be asked from here: public `ws(s)://` only, at
    /// most [`MAX_AUTHOR_RELAYS`], in list order, deduplicated.
    pub fn write_relays(&self, list: &Event) -> Vec<String> {
        use myco_napplet_runtime::seams::{Direction, RelayLane};
        let mut out = Vec::new();
        for url in crate::outbox::relay_list_urls(list, Direction::Read) {
            if out.len() >= MAX_AUTHOR_RELAYS {
                break;
            }
            let usable = match myco_napplet_runtime::nap::outbox::validate_relay_url(url) {
                Ok(RelayLane::Internet { .. }) => true,
                // A mesh relay is reached through the Circle pool, never by a
                // public lookup.
                Ok(_) => false,
                Err(_) => {
                    !self.guard_private_dials
                        && (url.starts_with("ws://") || url.starts_with("wss://"))
                        && !url.contains(".fips")
                }
            };
            if usable {
                push_relay(&mut out, url);
            }
        }
        out
    }

    /// Whether a relay taken from a list may be dialled right now: its name
    /// resolves to public addresses only.
    pub(crate) async fn may_dial(&self, url: &str) -> bool {
        !self.guard_private_dials || crate::outbox::dials_public(url).await
    }

    /// The union of `authors`' write relays from the lists **stored here**,
    /// at most `cap` of them, and the authors that have none stored. Never
    /// touches the network — for a check that asks every author at once and
    /// must not wait on list fetches ([`AuthorOutbox::fetch_lists`] runs
    /// beside it, for the next check).
    pub async fn stored_write_relays(
        &self,
        authors: &[PublicKey],
        cap: usize,
    ) -> (Vec<String>, Vec<PublicKey>) {
        let mut out = Vec::new();
        let mut missing = Vec::new();
        let lists = self.stored_lists_for(authors).await;
        for author in authors {
            let Some(list) = lists.get(author) else {
                missing.push(*author);
                continue;
            };
            for url in self.write_relays(list) {
                if out.len() >= cap {
                    break;
                }
                push_relay(&mut out, &url);
            }
        }
        (out, missing)
    }

    /// Fetch `authors`' lists from `relays` in **one** `REQ` per relay,
    /// bounded by [`LIST_FETCH_TIMEOUT`], and store what comes back. Authors
    /// recently found to have none are skipped; one still without a list
    /// afterwards is remembered as a miss — but only when every relay
    /// answered: a relay that timed out may have had it.
    pub async fn fetch_lists(&self, authors: &[PublicKey], relays: &[String]) {
        let wanted: Vec<PublicKey> = authors
            .iter()
            .filter(|a| !self.recently_missed(a))
            .copied()
            .collect();
        if self.fetch_lists_from(&wanted, relays).await {
            self.note_misses_among(&wanted).await;
        } else {
            self.note_soft_misses_among(&wanted).await;
        }
    }

    /// The round behind [`AuthorOutbox::fetch_lists`], with no miss memory
    /// either way: `authors`' lists from `relays`, one `REQ` per relay
    /// (chunked by [`LIST_AUTHORS_PER_FILTER`]), stored. Returns whether
    /// every relay answered to the end — the caller's condition for calling
    /// a list not found a miss.
    pub async fn fetch_lists_from(&self, authors: &[PublicKey], relays: &[String]) -> bool {
        if authors.is_empty() || relays.is_empty() {
            return true;
        }
        let filters: Vec<serde_json::Value> = authors
            .chunks(LIST_AUTHORS_PER_FILTER)
            .map(|chunk| {
                serde_json::json!({
                    "kinds": [nostr::Kind::RelayList.as_u16()],
                    "authors": chunk.iter().map(|a| a.to_hex()).collect::<Vec<_>>(),
                })
            })
            .collect();
        // Each list is remembered the moment any relay sends it, so a plan
        // waiting on it goes ahead on the first relay to have it, not the
        // slowest to finish.
        let (found, mut arriving) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let asked = join_all(relays.iter().map(|url| {
            let (filters, found) = (filters.clone(), found.clone());
            async move {
                // Configured and indexer relays: not resolved first. A relay
                // someone else named goes through the caller's private-host
                // guard instead (see `OutboxService::fetch_relay_lists_now`).
                matches!(
                    crate::relay_health::timeout(
                        url,
                        LIST_FETCH_TIMEOUT,
                        query_relay_each(url, filters, |ev| {
                            let _ = found.send(ev);
                        }),
                    )
                    .await,
                    Ok(Ok(()))
                )
            }
        }));
        drop(found);
        let remember = async {
            while let Some(ev) = arriving.recv().await {
                if ev.kind == nostr::Kind::RelayList && authors.contains(&ev.pubkey) {
                    self.remember(ev).await;
                }
            }
        };
        let (answered, ()) = futures_util::future::join(asked, remember).await;
        answered.into_iter().all(|ok| ok)
    }

    /// Remember as a miss each of `authors` with no list stored here — by
    /// this fetch or another path (a list riding along with a manifest
    /// query).
    pub async fn note_misses_among(&self, authors: &[PublicKey]) {
        let lists = self.stored_lists_for(authors).await;
        for author in authors {
            if !lists.contains_key(author) {
                self.note_miss(author);
            }
        }
    }

    /// As [`AuthorOutbox::note_misses_among`], for a lookup that was not a
    /// clean "no": remembered for [`LIST_SOFT_MISS_FOR`] only.
    pub async fn note_soft_misses_among(&self, authors: &[PublicKey]) {
        let lists = self.stored_lists_for(authors).await;
        for author in authors {
            if !lists.contains_key(author) {
                self.note_soft_miss(author);
            }
        }
    }
}

/// Default public Blossom servers, tried after a manifest's own `["server",…]`
/// hints.
pub fn default_blossom_servers() -> Vec<String> {
    // A fixed list is not a resolution policy — a `blossom:sha256:` URI names
    // no server, and BUD-03 (kind 10063) is how an author says where their
    // blobs live; reading it is roadmap. Until then the list has to cover the
    // large public replicas napplets are actually published to: `blssm.us`
    // and `blossom.ditto.pub` hold the letsmap release set, which none of the
    // first three do.
    [
        "https://blossom.primal.net",
        "https://cdn.satellite.earth",
        "https://blossom.band",
        "https://blssm.us",
        "https://blossom.ditto.pub",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Fetches manifests from public relays and blobs from public Blossom over IP.
pub struct IpPeerSource {
    relays: Vec<String>,
    blossom_servers: Vec<String>,
    http: reqwest::Client,
    timeout: Duration,
    /// When true, blob fetches ignore the manifest's `["server", …]` hints and
    /// use only `blossom_servers` — so a **mesh** source never reaches out to
    /// public Blossom over the internet (it stays on the peer's `[fd00::]:24243`).
    ignore_manifest_servers: bool,
    /// For a **mesh** source: the shared peer-relay pool + the holder's npub, so a
    /// manifest REQ reuses the one persistent WS connection to the peer instead of
    /// opening a fresh `query_relay` socket. `None` for a public-relay source.
    peer_relay: Option<(std::sync::Arc<crate::peer_relay::PeerRelayPool>, String)>,
    /// Fetch manifests of this kind instead of the nsite kind implied by the
    /// `d` tag. Set for napplets — see [`IpPeerSource::with_kind`].
    kind_override: Option<u16>,
    /// How long to keep waiting for other relays after the first answers.
    /// `None` waits for every relay. See [`IpPeerSource::with_first_answer_grace`].
    first_answer_grace: Option<Duration>,
    /// Refuse a blob larger than this while it downloads. `None` accepts any
    /// size, which is what nsite sync wants — its manifests say what to
    /// expect. See [`IpPeerSource::with_max_blob_bytes`].
    max_blob_bytes: Option<usize>,
    /// Relays the pointer named, asked ahead of the author's list and
    /// `relays`. See [`IpPeerSource::with_relay_hints`].
    hints: Vec<String>,
    /// Look up the author's NIP-65 relays too. See
    /// [`IpPeerSource::with_author_outbox`].
    author_outbox: Option<std::sync::Arc<AuthorOutbox>>,
    /// Give up on a blob when nothing arrives for this long. See
    /// [`IpPeerSource::with_stall_timeout`].
    stall_timeout: Option<Duration>,
}

impl IpPeerSource {
    pub fn new(relays: Vec<String>, blossom_servers: Vec<String>) -> Self {
        Self {
            relays,
            hints: Vec::new(),
            author_outbox: None,
            blossom_servers,
            http: reqwest::Client::builder()
                // Generous: a blob can be MBs over a slow BLE mesh link.
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
            timeout: Duration::from_secs(8),
            ignore_manifest_servers: false,
            peer_relay: None,
            kind_override: None,
            first_answer_grace: None,
            max_blob_bytes: None,
            stall_timeout: None,
        }
    }

    /// Route this (mesh) source's manifest REQs through the shared peer-relay pool,
    /// reusing the persistent WS connection to `npub` instead of a one-shot socket.
    pub fn over_peer_relay(
        mut self,
        pool: std::sync::Arc<crate::peer_relay::PeerRelayPool>,
        npub: &str,
    ) -> Self {
        self.peer_relay = Some((pool, npub.to_string()));
        self
    }

    /// Use `http` rather than a client of this source's own, so repeated
    /// fetches share its connection pool — one TLS handshake per server, not
    /// one per blob.
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Fetch blobs only from this source's own servers, never the manifest's
    /// public `server` hints (used by the mesh source — keep it on the mesh).
    pub fn ignoring_manifest_servers(mut self) -> Self {
        self.ignore_manifest_servers = true;
        self
    }

    /// The defaults (public relays + Blossom).
    pub fn with_defaults() -> Self {
        Self::new(default_relays(), default_blossom_servers())
    }

    /// Override the per-relay timeout (mesh links want a longer one than IP).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Stop waiting for the remaining relays this long after the first one
    /// answers.
    ///
    /// Without it a fetch takes as long as the *slowest* relay, because every
    /// relay is queried in parallel and all are awaited. In practice one relay
    /// answers in a few hundred milliseconds while another holds the connection
    /// open until the timeout, so the user waits the full timeout for an answer
    /// that arrived almost immediately.
    ///
    /// The grace period is what keeps "newest wins" meaningful: relays that
    /// hold the event answer at similar speeds, so a short wait after the first
    /// still collects the others, while a relay that has nothing to say no
    /// longer sets the pace.
    pub fn with_first_answer_grace(mut self, grace: Duration) -> Self {
        self.first_answer_grace = Some(grace);
        self
    }

    /// Give up on a blob the moment it is known to exceed `max` bytes — from
    /// the `Content-Length` when there is one, else as the body streams in.
    ///
    /// For fetches a napplet asked for by hash: it names the blob, not the
    /// size, and a cap checked on the finished body has already paid for the
    /// body, over BLE if the holder is a peer in the room.
    pub fn with_max_blob_bytes(mut self, max: usize) -> Self {
        self.max_blob_bytes = Some(max);
        self
    }

    /// Give up on a blob when the server sends nothing — no response, no
    /// next chunk — for `stall`, rather than when the whole download has
    /// taken the client's 60 s.
    ///
    /// A peer that walked out of range sends nothing at all; one on a slow
    /// link still sends something every few hundred milliseconds. Telling
    /// the two apart by silence, not total time, lets a caller with other
    /// peers to ask (see [`FirstOf`]) move on in seconds without cutting off
    /// a large download that is getting there.
    pub fn with_stall_timeout(mut self, stall: Duration) -> Self {
        self.stall_timeout = Some(stall);
        self
    }

    /// Fetch manifests of an explicit kind instead of the nsite kind implied by
    /// the `d` tag.
    ///
    /// NIP-5D napplets share NIP-5A's manifest shape at their own kinds, so the
    /// fetch is identical but for the number. Without this the source would ask
    /// for 15128/35128 and find nothing, which looks exactly like a napplet that
    /// is not published.
    pub fn with_kind(mut self, kind: u16) -> Self {
        self.kind_override = Some(kind);
        self
    }

    /// Ask these relays first — a pointer's own hints, e.g. an `naddr`'s.
    pub fn with_relay_hints(mut self, hints: Vec<String>) -> Self {
        self.hints = hints;
        self
    }

    /// Find manifests where their author says they publish: the NIP-65
    /// outbox model.
    ///
    /// The relays asked are, in order, the hints, the author's write relays
    /// (from their kind 10002), and this source's own relays (the defaults) —
    /// see [`lookup_relays`]. When the list is stored here it is used at once
    /// and every relay is asked in one parallel round.
    ///
    /// When it is not, the lookup does **not** wait for it. The hints and the
    /// defaults are asked for the manifest at once, and the relay list rides
    /// along as a second filter in the same `REQ` to them — no extra socket —
    /// while the indexer relays are asked for the list alone. The moment a
    /// list arrives, its write relays that are not already being asked join
    /// the same round, and the list is stored for next time. So a manifest on
    /// a default relay costs what it always did, and one only on the author's
    /// relay costs one relay round trip more, not a fixed list timeout. With
    /// a first-answer grace, the grace starts at the first *manifest*, never
    /// at a relay list.
    ///
    /// The round has **one** deadline, the per-relay timeout counted from its
    /// start: a relay that joins late gets only what is left of it, so the
    /// worst case is what a lookup without the list took. An author found to
    /// have no list anywhere is remembered for [`LIST_MISS_REMEMBERED_FOR`],
    /// and the indexers are not asked about them again until then.
    ///
    /// A mesh source (`over_peer_relay`) ignores this: it asks one peer.
    pub fn with_author_outbox(mut self, outbox: std::sync::Arc<AuthorOutbox>) -> Self {
        self.author_outbox = Some(outbox);
        self
    }

    /// Ask the hints, then the author's relays (when the list is stored or
    /// arrives), then the defaults — all in one round. See
    /// [`IpPeerSource::with_author_outbox`].
    async fn fetch_via_outbox(
        &self,
        outbox: &AuthorOutbox,
        author: &PublicKey,
        manifest_filter: serde_json::Value,
    ) -> Vec<Event> {
        use futures_util::future::{BoxFuture, FutureExt};
        use futures_util::stream::FuturesUnordered;

        let list_filter = serde_json::json!({
            "kinds": [nostr::Kind::RelayList.as_u16()],
            "authors": [author.to_hex()],
            "limit": 1,
        });
        let stored = outbox.stored_list(author).await;
        let author_relays = stored
            .as_ref()
            .map(|l| outbox.write_relays(l))
            .unwrap_or_default();
        let is_ours = |url: &str| {
            self.hints
                .iter()
                .chain(&self.relays)
                .any(|r| same_relay(r, url))
        };

        // One deadline for the whole round, from its start: a relay that joins
        // late gets what is left, never a fresh timeout, so the lookup takes
        // no longer than one without the author's list.
        let round_end = tokio::time::Instant::now() + self.timeout;
        // `guard`: the URL came from a relay list only, so resolve it and
        // refuse a private address before dialling.
        fn ask_relay(
            outbox: &AuthorOutbox,
            url: String,
            filters: Vec<serde_json::Value>,
            guard: bool,
            round_end: tokio::time::Instant,
        ) -> BoxFuture<'_, Vec<Event>> {
            async move {
                let dial = async {
                    if guard && !outbox.may_dial(&url).await {
                        return Ok(Vec::new());
                    }
                    query_relay_filters(&url, filters).await
                };
                // The round's shared cutoff, not this relay's: not counted
                // against it (see `relay_health`).
                match tokio::time::timeout_at(round_end, dial).await {
                    Ok(Ok(events)) => events,
                    _ => Vec::new(),
                }
            }
            .boxed()
        }
        let ask = |url: String, filters: Vec<serde_json::Value>, guard: bool| {
            ask_relay(outbox, url, filters, guard, round_end)
        };

        let mut asked: Vec<String> = Vec::new();
        let mut pending: FuturesUnordered<BoxFuture<'_, Vec<Event>>> = FuturesUnordered::new();
        let mut from_list = 0usize;
        for url in lookup_relays(&self.hints, &author_relays, &self.relays) {
            let filters = if is_ours(&url) {
                // The list rides along: it refreshes a stored one for free.
                vec![manifest_filter.clone(), list_filter.clone()]
            } else {
                from_list += 1;
                vec![manifest_filter.clone()]
            };
            pending.push(ask(url.clone(), filters, !is_ours(&url)));
            asked.push(url);
        }
        // The indexers only for a list not stored here and not recently
        // looked for in vain; the defaults are asked for it either way.
        let seek_list = stored.is_none() && !outbox.recently_missed(author);
        if seek_list {
            for url in &outbox.indexers {
                if !asked.iter().any(|r| same_relay(r, url)) {
                    pending.push(ask(url.clone(), vec![list_filter.clone()], false));
                    asked.push(url.clone());
                }
            }
        }

        let mut newest_list = stored;
        let mut manifests = Vec::new();
        let mut deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
        let mut exhausted = false;
        loop {
            let next = match deadline.as_mut() {
                None => pending.next().await,
                Some(sleep) => tokio::select! {
                    next = pending.next() => next,
                    _ = sleep.as_mut() => break,
                },
            };
            let Some(events) = next else {
                exhausted = true;
                break;
            };
            let mut answered = false;
            for ev in events {
                if ev.pubkey != *author {
                    continue;
                }
                if ev.kind == nostr::Kind::RelayList {
                    if newest_list
                        .as_ref()
                        .is_none_or(|n| ev.created_at > n.created_at)
                    {
                        // A newer list than any seen: keep it, and ask the
                        // relays it adds, within the cap.
                        outbox.remember(ev.clone()).await;
                        for url in outbox.write_relays(&ev) {
                            if from_list >= MAX_AUTHOR_RELAYS {
                                break;
                            }
                            if asked.iter().any(|r| same_relay(r, &url)) {
                                continue;
                            }
                            from_list += 1;
                            tracing::debug!(url, "manifest lookup: asking the author's relay");
                            pending.push(ask(url.clone(), vec![manifest_filter.clone()], true));
                            asked.push(url);
                        }
                        newest_list = Some(ev);
                    }
                } else {
                    answered = true;
                    manifests.push(ev);
                }
            }
            if answered && deadline.is_none() {
                if let Some(grace) = self.first_answer_grace {
                    deadline = Some(Box::pin(tokio::time::sleep(grace)));
                }
            }
        }
        // Every relay asked and none had a list: do not ask the indexers
        // again for a while. A round cut short by the grace proves nothing.
        if seek_list && exhausted && newest_list.is_none() {
            outbox.note_miss(author);
        }
        manifests
    }
}

/// A [`PeerSource`] that pulls from a specific **holder's** embedded relay +
/// A peer's mesh relay, addressed by **name**: `ws://<npub>.fips:4870`.
///
/// Always name, never the `fd00::` literal the name resolves to. Resolving it
/// is what teaches the node the address→pubkey mapping, and without that the
/// node has no pubkey to open a session with and the dial fails as unroutable
/// for anyone who is not already a direct neighbour. The literal happens to
/// work for adjacent peers, which is exactly what made this hard to spot.
pub(crate) fn mesh_relay_url(npub: &str) -> String {
    format!("ws://{npub}.fips:4870")
}

/// Run every query concurrently, but stop `grace` after the first one comes
/// back with something.
///
/// A relay with nothing to say is indistinguishable from a slow one until it
/// answers, so waiting for all of them means paying for the worst. Waiting a
/// little past the first real answer collects the relays that also have the
/// event without paying for the relays that never will.
async fn collect_with_grace<F>(
    queries: impl IntoIterator<Item = F>,
    grace: Duration,
) -> Vec<Vec<Event>>
where
    F: std::future::Future<Output = Vec<Event>>,
{
    use futures_util::stream::{FuturesUnordered, StreamExt};

    let mut pending: FuturesUnordered<F> = queries.into_iter().collect();
    let mut out = Vec::new();
    let mut deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;

    loop {
        match deadline.as_mut() {
            None => match pending.next().await {
                Some(events) => {
                    let answered = !events.is_empty();
                    out.push(events);
                    if answered {
                        deadline = Some(Box::pin(tokio::time::sleep(grace)));
                    }
                }
                None => break,
            },
            Some(sleep) => {
                tokio::select! {
                    next = pending.next() => match next {
                        Some(events) => out.push(events),
                        None => break,
                    },
                    _ = sleep.as_mut() => break,
                }
            }
        }
    }
    out
}

/// A peer's mesh Blossom endpoint, by name. See [`mesh_relay_url`].
pub(crate) fn mesh_blossom_url(npub: &str) -> String {
    format!("http://{npub}.fips:24243")
}

/// A peer's **auth service** endpoint, by name — the only port an unpaired peer
/// can reach. See [`mesh_relay_url`] for why this is addressed by npub.
pub(crate) fn mesh_auth_url(npub: &str) -> String {
    format!("http://{npub}.fips:{}/pair", crate::auth_service::AUTH_PORT)
}

/// What came back from a peer's auth service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairDelivery {
    /// They took it: paired, unpaired, or pending a human. Carries the reported
    /// status so the caller can log which.
    Accepted(&'static str),
    /// They answered and said no — a bad signature, an expired event, or a rate
    /// limit. Retrying will not change it.
    Refused(&'static str),
    /// Never got an answer. The mesh session is probably still coming up, so
    /// this is the case worth retrying.
    Unreachable,
}

/// POST a signed pair event to a peer's auth service.
///
/// Unlike a relay `OK`, the reply distinguishes "they have it and a human is
/// deciding" from "we never reached them", which is what lets the pairing retry
/// loop stop early instead of re-sending to a peer that already answered.
pub(crate) async fn post_pair_event(
    url: &str,
    event: &nostr::Event,
    timeout: Duration,
) -> PairDelivery {
    let Ok(client) = reqwest::Client::builder().timeout(timeout).build() else {
        return PairDelivery::Unreachable;
    };
    let body = match serde_json::to_string(event) {
        Ok(b) => b,
        Err(_) => return PairDelivery::Unreachable,
    };
    match client.post(url).body(body).send().await {
        Ok(resp) if resp.status().is_success() => PairDelivery::Accepted("paired"),
        Ok(resp) if resp.status() == reqwest::StatusCode::ACCEPTED => {
            PairDelivery::Accepted("pending")
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::FORBIDDEN => {
            PairDelivery::Refused("declined")
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::BAD_REQUEST => {
            PairDelivery::Refused("rejected")
        }
        // 429 / 503 are "not now" rather than "no" — worth another attempt.
        Ok(_) | Err(_) => PairDelivery::Unreachable,
    }
}

/// Blossom over the FIPS mesh, addressed by the holder's npub — see
/// [`mesh_relay_url`]. Requires the app-owned TUN to be up so the socket routes
/// over the mesh. A longer timeout than the IP source absorbs BLE latency +
/// first-contact session setup.
pub fn mesh_source_for(
    pool: std::sync::Arc<crate::peer_relay::PeerRelayPool>,
    holder_npub: &str,
) -> anyhow::Result<IpPeerSource> {
    fips::PeerIdentity::from_npub(holder_npub)
        .map_err(|e| anyhow::anyhow!("invalid holder npub {holder_npub}: {e}"))?;
    Ok(IpPeerSource::new(
        vec![mesh_relay_url(holder_npub)],
        vec![mesh_blossom_url(holder_npub)],
    )
    .with_timeout(Duration::from_secs(20))
    .ignoring_manifest_servers()
    .over_peer_relay(pool, holder_npub))
}

/// Several sources asked at once — the sharer and the Circle members in
/// reach — for one napplet or nsite slot, where the first good answer wins.
///
/// Asking them in turn would cost a dead peer's whole timeout (20 s for a
/// mesh source) before the next is tried, while the user watches "Looking
/// for this app". Every answer is still untrusted: a manifest that is not
/// signed by the slot's author for the slot, or bytes that do not hash to
/// the asked name, are passed over as if the source had said nothing, so one
/// bad peer cannot shadow a good one. The caller verifies what wins as it
/// would any single source's answer.
///
/// Manifests are raced: a `REQ` is small. Blobs are not — racing a blob
/// across every peer downloads it from all of them at once, over BLE. A blob
/// comes from one source at a time: the one that answered last, or, when
/// none has (install starts from a fresh source), the winner of a manifest
/// race — in reach, quick, and holding this app. When that source has
/// nothing, fails, or stalls (give mesh sources a
/// [`IpPeerSource::with_stall_timeout`]), it is dropped and the next is
/// picked the same way from the sources not yet asked for this blob.
pub struct FirstOf {
    sources: Vec<Box<dyn PeerSource>>,
    author: PublicKey,
    d_tag: Option<String>,
    /// Only manifests of this kind count.
    kind: u16,
    /// Index of the source that answered last; `usize::MAX` for none yet.
    preferred: std::sync::atomic::AtomicUsize,
}

impl FirstOf {
    pub fn new(
        sources: Vec<Box<dyn PeerSource>>,
        author: PublicKey,
        d_tag: Option<String>,
        kind: u16,
    ) -> Self {
        Self {
            sources,
            author,
            d_tag,
            kind,
            preferred: std::sync::atomic::AtomicUsize::new(usize::MAX),
        }
    }

    fn prefer(&self, index: usize) {
        self.preferred
            .store(index, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether `event` is a signed manifest by `author` for the asked slot.
    fn answers(&self, event: &Event, author: &PublicKey, d_tag: Option<&str>) -> bool {
        event.verify().is_ok()
            && event.pubkey == *author
            && event.kind.as_u16() == self.kind
            && (d_tag.is_none() || event_d_tag(event).as_deref() == d_tag)
    }

    /// Race the manifest across every source not in `skip`; the first good
    /// answer and whose it was. The winner becomes the preferred source.
    async fn race_manifest(
        &self,
        author: &PublicKey,
        d_tag: Option<&str>,
        skip: &[usize],
    ) -> Option<(usize, Event)> {
        let mut asks: futures_util::stream::FuturesUnordered<_> = self
            .sources
            .iter()
            .enumerate()
            .filter(|(i, _)| !skip.contains(i))
            .map(|(i, source)| async move { (i, source.fetch_manifest(author, d_tag).await) })
            .collect();
        while let Some((i, answer)) = asks.next().await {
            match answer {
                Ok(Some(event)) if self.answers(&event, author, d_tag) => {
                    self.prefer(i);
                    return Some((i, event));
                }
                Ok(Some(_)) => tracing::debug!("source {i} answered with the wrong manifest"),
                Ok(None) => {}
                Err(e) => tracing::debug!("source {i}: {e}"),
            }
        }
        None
    }
}

#[async_trait]
impl PeerSource for FirstOf {
    async fn fetch_manifest(
        &self,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>> {
        Ok(self
            .race_manifest(author, d_tag, &[])
            .await
            .map(|(_, event)| event))
    }

    async fn fetch_blob(
        &self,
        sha256_hex_want: &str,
        servers: &[String],
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let mut asked: Vec<usize> = Vec::new();
        while asked.len() < self.sources.len() {
            let preferred = self.preferred.load(std::sync::atomic::Ordering::Relaxed);
            let pick = if preferred < self.sources.len() && !asked.contains(&preferred) {
                preferred
            } else {
                match self
                    .race_manifest(&self.author, self.d_tag.as_deref(), &asked)
                    .await
                {
                    Some((i, _)) => i,
                    // No one left holds the app; the caller moves on to its
                    // next source.
                    None => break,
                }
            };
            asked.push(pick);
            match self.sources[pick]
                .fetch_blob(sha256_hex_want, servers)
                .await
            {
                Ok(Some(bytes)) if sha256_hex(&bytes) == sha256_hex_want => {
                    self.prefer(pick);
                    return Ok(Some(bytes));
                }
                Ok(_) => tracing::debug!("source {pick} has no {sha256_hex_want}"),
                Err(e) => tracing::debug!("source {pick}: {e}"),
            }
        }
        Ok(None)
    }
}

/// Dev-menu **speedtest** against a mesh peer: PUT a fresh `bytes`-sized payload to
/// the peer's Blossom (`http://[fd00::peer]:24243/upload`), then GET it back, timing
/// each leg. Returns `(up_mbps, down_mbps)` — upload (this device → peer) and
/// download (peer → this device) throughput in megabits per second. The whole call
/// is bounded by `timeout`. The peer's Blossom gates non-loopback sources by Circle
/// membership, so this only succeeds against a paired, reachable peer.
///
/// Note: the payload is content-addressed and there is no Blossom DELETE, so a run
/// leaves a `bytes`-sized blob on the peer until its next cache wipe — fine for the
/// occasional dev measurement, but keep `bytes` modest.
pub async fn speedtest_peer(
    npub: &str,
    bytes: usize,
    timeout: Duration,
) -> anyhow::Result<(f64, f64)> {
    // Addressed by name, resolved by the system resolver like any other host: the
    // tunnel advertises the in-mesh sentinel as its DNS server, so `<npub>.fips`
    // resolves for every process on the device, this one included. (It used to be
    // mapped to the peer's `fd00::` literal by hand via `.resolve`, from before
    // that resolver existed. Doing so skipped resolution, which is also what
    // registers the peer's identity with the node — so the literal only ever
    // worked for a peer the node already knew: a direct neighbour.)
    fips::PeerIdentity::from_npub(npub)
        .map_err(|e| anyhow::anyhow!("invalid peer npub {npub}: {e}"))?;
    let host = format!("{npub}.fips");
    let client = reqwest::Client::builder()
        // A short connect timeout so an unroutable/unreachable peer fails fast
        // instead of burning the whole `timeout` budget; the total still bounds the
        // (potentially slow over BLE) transfer.
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .build()?;
    speedtest_blossom(&client, &format!("http://{host}:24243"), bytes).await
}

/// The Blossom round-trip itself, against an already-built `client` + `base` URL —
/// split out so it can be exercised against a local server in tests.
async fn speedtest_blossom(
    client: &reqwest::Client,
    base: &str,
    bytes: usize,
) -> anyhow::Result<(f64, f64)> {
    // A fresh, incompressible payload each run so the peer can't already hold it
    // (which would make the GET a local hit and the hash collide across runs).
    let payload = random_bytes(bytes);

    let t_up = Instant::now();
    let resp = client
        .put(format!("{base}/upload"))
        .body(payload)
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        // The speedtest is the only thing that pushes blobs to a peer, and blob
        // upload is off by default (`reference/thinning-custom-relay.md`, D10).
        // Say so plainly — this is a permission the peer has to grant, not a
        // network fault to retry.
        anyhow::bail!("peer does not allow uploads (blob write permission is off on their device)");
    }
    if !resp.status().is_success() {
        anyhow::bail!("upload rejected ({})", resp.status());
    }
    let up_mbps = throughput_mbps(bytes, t_up.elapsed());
    let descriptor: serde_json::Value = serde_json::from_str(&resp.text().await?)?;
    let hash = descriptor["sha256"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("upload descriptor missing sha256"))?
        .to_string();

    let t_down = Instant::now();
    let got = client.get(format!("{base}/{hash}")).send().await?;
    if !got.status().is_success() {
        anyhow::bail!("download rejected ({})", got.status());
    }
    let body = got.bytes().await?;
    let down_mbps = throughput_mbps(body.len(), t_down.elapsed());

    Ok((up_mbps, down_mbps))
}

fn throughput_mbps(bytes: usize, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return 0.0;
    }
    (bytes as f64 * 8.0) / secs / 1_000_000.0
}

/// `n` pseudo-random bytes from a time-seeded xorshift64 — cheap and dependency-
/// free; we only need the bytes to be fresh per run, not cryptographically random.
pub(crate) fn random_bytes(n: usize) -> Vec<u8> {
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15)
        | 1;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(n);
    out
}

/// Publish one signed event to one relay and wait for its `OK`. `Ok(true)`
/// means accepted, `Ok(false)` means the relay said no (the message is
/// logged), `Err` means it never answered. Bound the whole call with a
/// timeout at the call site — a dead relay must not hold a fan-out task open.
///
/// Over the shared relay pool (`relay_pool`): the relay's one connection,
/// opened if nothing holds it yet.
pub async fn publish_to_relay(url: &str, event: &Event) -> anyhow::Result<bool> {
    crate::relay_pool::publish(url, event).await
}

/// Keep a `REQ` open on `url` and send every event it delivers — the stored
/// ones and then the live ones — to `out`, verified, until the relay closes
/// the connection or the subscription, the socket fails, or `out` is gone.
///
/// The long-lived half of a napplet's subscription: a feed napplet reads a
/// subscription as an endless stream, and a relay that has a note published
/// in an hour must deliver it in an hour, not never. Reconnecting is the
/// caller's (with its backoff); this is one subscription's life. Returns `Ok`
/// when the relay or the reader ended it, `Err` when it could not be set up
/// or the connection failed.
///
/// `saw_eose` is set when the relay said its stored events were over — the
/// caller's cue that everything up to now has been heard, so a reconnect
/// may ask only for what is newer. A connection that never got that far
/// leaves it unset, and the next one asks again from the start. It is a
/// flag rather than the return value because the caller may drop this
/// future (the user went offline only) after EOSE.
///
/// A silent socket is not trusted: the pool pings it and gives up on it after
/// [`crate::relay_pool::SILENT_FOR`] without a frame — a phone that changed
/// networks leaves sockets that never error, they just never speak again.
pub(crate) async fn stream_relay_filters(
    url: &str,
    filters: Vec<serde_json::Value>,
    out: tokio::sync::mpsc::Sender<Event>,
    saw_eose: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<()> {
    // Skipped relays are not re-opened until their skip is up; the caller's
    // backoff keeps asking. See `relay_health`. The relay's one pooled
    // connection carries this beside every other read (`relay_pool`), and
    // watches its own liveness.
    crate::relay_pool::subscribe(url, filters, out, saw_eose).await
}

/// Query one relay for a single filter, collecting events until EOSE. The whole
/// call (connect + REQ + read) is hard-bounded by a `timeout` at the call site,
/// so a dead relay can't hang the sync on a slow TCP/TLS connect.
pub async fn query_relay(url: &str, filter: serde_json::Value) -> anyhow::Result<Vec<Event>> {
    query_relay_filters(url, vec![filter]).await
}

/// As [`query_relay`], with several filters in **one** `REQ` on **one**
/// connection — how a multi-filter subscription is meant to travel. A
/// napplet's subscribe hands over a list of filters; opening a socket per
/// filter per relay was a TLS handshake for each, on a phone.
pub async fn query_relay_filters(
    url: &str,
    filters: Vec<serde_json::Value>,
) -> anyhow::Result<Vec<Event>> {
    let mut events = Vec::new();
    query_relay_each(url, filters, |event| events.push(event)).await?;
    Ok(events)
}

/// As [`query_relay_filters`], handing each verified event to `each` as the
/// relay sends it rather than once at `EOSE`. A caller that bounds this with
/// a timeout keeps what `each` was given before it — a relay that sent two
/// hundred events and never reached `EOSE` still sent two hundred events.
pub async fn query_relay_each(
    url: &str,
    filters: Vec<serde_json::Value>,
    each: impl FnMut(Event),
) -> anyhow::Result<()> {
    // A relay on the skip list is not dialled: it answers "nothing" at once,
    // so a round never waits on it. See `relay_health`. The read rides the
    // relay's one pooled connection (`relay_pool`).
    crate::relay_pool::request(url, filters, each).await
}

#[async_trait]
impl PeerSource for IpPeerSource {
    async fn fetch_manifest(
        &self,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>> {
        let kind = self.kind_override.unwrap_or_else(|| kind_for(d_tag));
        let mut filter = serde_json::json!({
            "kinds": [kind],
            "authors": [hex::encode(author.to_bytes())],
            "limit": 1,
        });
        if let Some(d) = d_tag {
            filter["#d"] = serde_json::json!([d]);
        }

        // Mesh source: reuse the persistent pooled WS to the peer (one socket per
        // peer, shared with chat fan-out) rather than a fresh per-fetch connect.
        let results: Vec<Vec<Event>> = if let Some((pool, npub)) = &self.peer_relay {
            let url = self.relays.first().cloned().unwrap_or_default();
            vec![pool.request(npub, &url, vec![filter], self.timeout).await]
        } else if let Some(outbox) = &self.author_outbox {
            vec![self.fetch_via_outbox(outbox, author, filter).await]
        } else {
            // Public relays: each hard-bounded by `self.timeout` (connect + read), so
            // a dead relay can't stall the whole sync on a slow TCP/TLS connect; the
            // rest still answer. A timeout/error yields an empty set for that relay.
            let relays = lookup_relays(&self.hints, &[], &self.relays);
            let queries = relays.iter().map(|url| async {
                match crate::relay_health::timeout(
                    url,
                    self.timeout,
                    query_relay(url, filter.clone()),
                )
                .await
                {
                    Ok(Ok(events)) => events,
                    _ => Vec::new(),
                }
            });
            match self.first_answer_grace {
                None => join_all(queries).await,
                Some(grace) => collect_with_grace(queries, grace).await,
            }
        };

        // Pick the newest event matching the requested slot. Signatures were
        // already checked at ingress (the pool, or `query_relay`).
        let mut newest: Option<Event> = None;
        for events in results.into_iter() {
            for ev in events {
                if ev.pubkey != *author || ev.kind.as_u16() != kind {
                    continue;
                }
                if d_tag.is_some() && event_d_tag(&ev).as_deref() != d_tag {
                    continue;
                }
                if newest.as_ref().is_none_or(|n| ev.created_at > n.created_at) {
                    newest = Some(ev);
                }
            }
        }
        Ok(newest)
    }

    async fn fetch_blob(
        &self,
        sha256_hex_want: &str,
        servers: &[String],
    ) -> anyhow::Result<Option<Vec<u8>>> {
        // Manifest hints first, then this source's own servers (deduped). A mesh
        // source skips the manifest's public hints entirely (stay on the mesh).
        let mut candidates: Vec<String> = if self.ignore_manifest_servers {
            Vec::new()
        } else {
            servers.to_vec()
        };
        for s in &self.blossom_servers {
            if !candidates.contains(s) {
                candidates.push(s.clone());
            }
        }

        for server in candidates {
            // A server that failed a moment ago (5xx; TLS, DNS or a refused
            // connection while online) is not asked again until its skip is
            // up. A 404 is an answer — "not here" — and never held against it.
            if crate::relay_health::is_skipped(&server) {
                continue;
            }
            let url = format!("{}/{}", server.trim_end_matches('/'), sha256_hex_want);
            let sent = match self.stall_timeout {
                Some(stall) => {
                    match tokio::time::timeout(stall, self.http.get(&url).send()).await {
                        Ok(sent) => sent,
                        Err(_) => {
                            tracing::debug!("{server}: no response in {stall:?}");
                            continue;
                        }
                    }
                }
                None => self.http.get(&url).send().await,
            };
            let resp = match sent {
                Ok(r) => {
                    let retry_after = r
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    crate::relay_health::record_http_status(
                        &server,
                        r.status().as_u16(),
                        retry_after.as_deref(),
                    );
                    if !r.status().is_success() {
                        // A 404 is "not here", and the server up and answering.
                        continue;
                    }
                    r
                }
                Err(e) => {
                    crate::relay_health::record_http_error(&server, &e);
                    continue;
                }
            };
            let Some(bytes) =
                read_body_bounded(resp, self.max_blob_bytes, self.stall_timeout).await
            else {
                continue;
            };
            // Self-authenticating: only accept bytes that hash to the wanted name.
            if sha256_hex(&bytes) == sha256_hex_want {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }
}

/// Read a response body, stopping early — `None` — the moment it is known to
/// exceed `max`: from `Content-Length` when the server sends one, otherwise as
/// the chunks arrive. `None` too when no chunk arrives for `stall`, and for a
/// read error; the caller tries the next server either way.
async fn read_body_bounded(
    mut resp: reqwest::Response,
    max: Option<usize>,
    stall: Option<Duration>,
) -> Option<Vec<u8>> {
    if max.is_none() && stall.is_none() {
        return resp.bytes().await.ok().map(|b| b.to_vec());
    }
    if let (Some(max), Some(len)) = (max, resp.content_length()) {
        if len > max as u64 {
            return None;
        }
    }
    let mut out = Vec::new();
    loop {
        let chunk = match stall {
            Some(stall) => tokio::time::timeout(stall, resp.chunk()).await.ok()?,
            None => resp.chunk().await,
        };
        let Some(chunk) = chunk.ok()? else {
            return Some(out);
        };
        if max.is_some_and(|max| out.len() + chunk.len() > max) {
            return None;
        }
        out.extend_from_slice(&chunk);
    }
}

fn event_d_tag(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some("d"))
            .then(|| s.get(1).cloned())
            .flatten()
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use futures_util::SinkExt;
    use nsite_deck::seams::RelayBackend;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    #[tokio::test]
    async fn speedtest_round_trips_through_blossom() {
        // A real embedded Blossom (PUT /upload + GET /<hash>) on loopback.
        let dir = std::env::temp_dir().join(format!("myco-speedtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Arc::new(myco_blossom::FsBlobStore::open(&dir).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(myco_blossom::server::serve_on(store, listener));

        let base = format!("http://{addr}");
        let client = reqwest::Client::new();
        let (up, down) = speedtest_blossom(&client, &base, 64 * 1024).await.unwrap();
        // Loopback: both legs move bytes and yield a finite, positive rate.
        assert!(up > 0.0 && up.is_finite(), "up_mbps = {up}");
        assert!(down > 0.0 && down.is_finite(), "down_mbps = {down}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A mock relay: accept one WS connection, read the REQ, reply with the given
    /// event then EOSE. Returns the `ws://` URL.
    /// The sub id of a `REQ` frame, `None` for anything else (`CLOSE`, pings).
    pub(crate) fn req_sub_id(msg: &Message) -> Option<String> {
        let Message::Text(text) = msg else {
            return None;
        };
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        (v.get(0)?.as_str()? == "REQ").then(|| v.get(1)?.as_str().map(str::to_string))?
    }

    async fn mock_relay(event_json: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                // Every REQ on the connection, answered under its own sub id,
                // and the socket held open — as a relay does. The test filter
                // always matches.
                while let Some(Ok(msg)) = ws.next().await {
                    let Some(sub) = req_sub_id(&msg) else {
                        continue;
                    };
                    let event = serde_json::json!([
                        "EVENT",
                        sub,
                        serde_json::from_str::<serde_json::Value>(&event_json).unwrap()
                    ]);
                    let _ = ws.send(Message::Text(event.to_string())).await;
                    let _ = ws
                        .send(Message::Text(serde_json::json!(["EOSE", sub]).to_string()))
                        .await;
                }
            }
        });
        format!("ws://{addr}")
    }

    /// A mock Blossom: serve `GET /<hash>` from a (hash -> bytes) map. Returns the
    /// `http://` base URL.
    /// A Blossom server that serves `bytes` in `chunks` pieces, `every` apart
    /// — a slow link — and goes silent after `send` of them, or before even
    /// the response when `send` is 0: a peer that walked out of range.
    async fn mock_blossom_dripping(
        bytes: Vec<u8>,
        chunks: usize,
        every: Duration,
        send: usize,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let bytes = bytes.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    if send == 0 {
                        std::future::pending::<()>().await;
                    }
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    for (i, piece) in bytes.chunks(bytes.len().div_ceil(chunks)).enumerate() {
                        if i == send {
                            std::future::pending::<()>().await;
                        }
                        tokio::time::sleep(every).await;
                        let _ = stream.write_all(piece).await;
                        let _ = stream.flush().await;
                    }
                });
            }
        });
        format!("http://{addr}")
    }

    /// A peer that went silent is given up on after the stall timeout, not
    /// the client's 60 s — before the response, or halfway through the body.
    #[tokio::test]
    async fn a_silent_server_is_given_up_on_after_the_stall_timeout() {
        let bytes = vec![7u8; 4096];
        let hash = sha256_hex(&bytes);
        for send in [0, 2] {
            let server =
                mock_blossom_dripping(bytes.clone(), 4, Duration::from_millis(20), send).await;
            let source = IpPeerSource::new(Vec::new(), vec![server])
                .ignoring_manifest_servers()
                .with_stall_timeout(Duration::from_millis(300));
            let got = tokio::time::timeout(Duration::from_secs(3), source.fetch_blob(&hash, &[]))
                .await
                .expect("a silent server was waited on past the stall timeout")
                .unwrap();
            assert!(got.is_none(), "after {send} chunks");
        }
    }

    /// A slow server that keeps sending is not cut off, however long the
    /// whole download takes: the timeout is for silence, not for size.
    #[tokio::test]
    async fn a_slow_server_that_keeps_sending_is_not_cut_off() {
        let bytes = vec![9u8; 4096];
        let hash = sha256_hex(&bytes);
        // Five pieces 150 ms apart: 750 ms in all, against a 300 ms stall.
        let server = mock_blossom_dripping(bytes.clone(), 5, Duration::from_millis(150), 5).await;
        let source = IpPeerSource::new(Vec::new(), vec![server])
            .ignoring_manifest_servers()
            .with_stall_timeout(Duration::from_millis(300));
        assert_eq!(source.fetch_blob(&hash, &[]).await.unwrap(), Some(bytes));
    }

    pub(crate) async fn mock_blossom(blobs: Vec<(String, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let map: Arc<std::collections::HashMap<String, Vec<u8>>> =
            Arc::new(blobs.into_iter().collect());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let map = map.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    // "GET /<hash> HTTP/1.1"
                    let hash = req
                        .split_whitespace()
                        .nth(1)
                        .map(|p| p.trim_start_matches('/').to_string())
                        .unwrap_or_default();
                    let resp = match map.get(&hash) {
                        Some(bytes) => {
                            let mut r = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                bytes.len()
                            )
                            .into_bytes();
                            r.extend_from_slice(bytes);
                            r
                        }
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = stream.write_all(&resp).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn fetches_manifest_and_blob_over_ip() {
        let site = nsite_deck::testing::build_test_site(
            &[("/index.html", b"<h1>online</h1>")],
            None,
            Some("Online Site"),
        );
        let event_json = serde_json::to_string(&site.manifest).unwrap();

        let relay_url = mock_relay(event_json).await;
        let blossom_url = mock_blossom(site.blobs.clone()).await;

        let source = IpPeerSource::new(vec![relay_url], vec![blossom_url]);

        // Manifest comes back, verified, matching the author.
        let got = source.fetch_manifest(&site.author, None).await.unwrap();
        assert_eq!(got.map(|e| e.id), Some(site.manifest.id));

        // Blob comes back, hash-verified.
        let (hash, bytes) = &site.blobs[0];
        let blob = source.fetch_blob(hash, &[]).await.unwrap();
        assert_eq!(blob.as_deref(), Some(bytes.as_slice()));

        // A wrong hash yields nothing.
        let miss = source
            .fetch_blob(
                "00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(miss, None);
    }

    /// A mock relay holding `events`: answers every REQ, on any number of
    /// connections, with the held events any of its filters match by kind and
    /// author, then EOSE. Returns the URL and a count of REQs served.
    pub(crate) async fn mock_relay_holding(
        events: Vec<Event>,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        mock_relay_delayed(events, Duration::ZERO).await
    }

    /// As [`mock_relay_holding`], answering each REQ only after `delay`.
    pub(crate) async fn mock_relay_delayed(
        events: Vec<Event>,
        delay: Duration,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let served = hits.clone();
        let events = Arc::new(events);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (events, served) = (events.clone(), served.clone());
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    // Every REQ on the connection, each under its own sub id,
                    // with the socket held open between them — as a relay does.
                    while let Some(Ok(msg)) = ws.next().await {
                        let Some(sub) = req_sub_id(&msg) else {
                            continue;
                        };
                        let Message::Text(req) = msg else { continue };
                        served.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(delay).await;
                        let req: serde_json::Value = serde_json::from_str(&req).unwrap();
                        let filters = &req.as_array().unwrap()[2..];
                        for ev in events.iter() {
                            let wanted = filters.iter().any(|f| {
                                let kind_ok = f["kinds"]
                                    .as_array()
                                    .is_none_or(|k| k.iter().any(|k| k == ev.kind.as_u16()));
                                let author_ok = f["authors"]
                                    .as_array()
                                    .is_none_or(|a| a.iter().any(|a| a == &ev.pubkey.to_hex()));
                                kind_ok && author_ok
                            });
                            if wanted {
                                let frame = serde_json::json!(["EVENT", sub, ev]);
                                let _ = ws.send(Message::Text(frame.to_string())).await;
                            }
                        }
                        let _ = ws
                            .send(Message::Text(serde_json::json!(["EOSE", sub]).to_string()))
                            .await;
                    }
                });
            }
        });
        (format!("ws://{addr}"), hits)
    }

    fn relay_list(keys: &nostr::Keys, tags: &[&[&str]]) -> Event {
        relay_list_at(keys, tags, nostr::Timestamp::now())
    }

    fn relay_list_at(keys: &nostr::Keys, tags: &[&[&str]], at: nostr::Timestamp) -> Event {
        nostr::EventBuilder::new(nostr::Kind::RelayList, "")
            .tags(tags.iter().map(|t| nostr::Tag::parse(t.to_vec()).unwrap()))
            .custom_created_at(at)
            .sign_with_keys(keys)
            .unwrap()
    }

    fn hits(counter: &Arc<std::sync::atomic::AtomicUsize>) -> usize {
        counter.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn outbox_over(
        store: Arc<nsite_deck::testing::MemRelay>,
        indexers: Vec<String>,
    ) -> Arc<AuthorOutbox> {
        Arc::new(
            AuthorOutbox::new(store)
                .with_indexers(indexers)
                .allowing_private_dials(),
        )
    }

    #[test]
    fn lookup_order_is_hints_then_author_then_defaults_deduped() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let got = lookup_relays(
            &s(&["wss://hint.example/", "wss://both.example"]),
            &s(&["wss://both.example/", "wss://author.example"]),
            &s(&["wss://hint.example", "wss://default.example"]),
        );
        assert_eq!(
            got,
            s(&[
                "wss://hint.example/",
                "wss://both.example",
                "wss://author.example",
                "wss://default.example",
            ])
        );
    }

    #[test]
    fn write_relays_follow_markers_refuse_non_public_and_cap() {
        let keys = nostr::Keys::generate();
        let list = relay_list(
            &keys,
            &[
                &["r", "wss://a.example"],
                &["r", "wss://a.example/"],
                &["r", "wss://read-only.example", "read"],
                &["r", "wss://b.example/", "write"],
                &["r", "https://not-a-relay.example"],
                &["r", "ws://127.0.0.1:4870"],
                &["r", "ws://npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.fips:4870"],
                &["r", "wss://c.example"],
                &["r", "wss://d.example"],
                &["r", "wss://e.example"],
                &["r", "wss://f.example"],
            ],
        );
        let outbox = AuthorOutbox::new(Arc::new(nsite_deck::testing::MemRelay::new()));
        assert_eq!(
            outbox.write_relays(&list),
            vec![
                "wss://a.example",
                "wss://b.example",
                "wss://c.example",
                "wss://d.example",
                "wss://e.example",
            ]
        );
    }

    /// The Minesweeper case: a pointer with no hints, a manifest on the
    /// author's relay only, and the author's list on a default relay. Found
    /// through the list — and the list is stored for next time.
    ///
    /// The author's relay answers well after the grace would have run out,
    /// so this also proves the grace starts at a manifest, not at the list.
    #[tokio::test]
    async fn manifest_only_on_the_authors_relay_is_found_through_their_list() {
        let keys = nostr::Keys::generate();
        let site = nsite_deck::testing::build_test_site_with_keys(
            &keys,
            &[("/index.html", b"<h1>hi</h1>")],
            Some("minesweeper"),
            None,
        );
        let (author_relay, _) =
            mock_relay_delayed(vec![site.manifest.clone()], Duration::from_millis(300)).await;
        let list = relay_list(
            &keys,
            &[
                &["r", &format!("{author_relay}/"), "write"],
                &["r", "wss://inbox.example", "read"],
            ],
        );
        let (default_relay, _) = mock_relay_holding(vec![list.clone()]).await;

        let store = Arc::new(nsite_deck::testing::MemRelay::new());
        let source = IpPeerSource::new(vec![default_relay], Vec::new())
            .with_author_outbox(outbox_over(store.clone(), Vec::new()))
            .with_first_answer_grace(Duration::from_millis(50));

        let got = source
            .fetch_manifest(&site.author, Some("minesweeper"))
            .await
            .unwrap();
        assert_eq!(got.map(|e| e.id), Some(site.manifest.id));

        let outbox = AuthorOutbox::new(store);
        assert_eq!(
            outbox.stored_list(&site.author).await.map(|e| e.id),
            Some(list.id),
            "the fetched relay list is kept in the local relay"
        );
    }

    /// A list stored here is used at once, and the indexers are not asked.
    #[tokio::test]
    async fn a_stored_list_is_used_without_asking_the_indexers() {
        let keys = nostr::Keys::generate();
        let site = nsite_deck::testing::build_test_site_with_keys(
            &keys,
            &[("/index.html", b"stored")],
            None,
            None,
        );
        let (author_relay, author_hits) = mock_relay_holding(vec![site.manifest.clone()]).await;
        let (default_relay, _) = mock_relay_holding(Vec::new()).await;
        let (indexer, indexer_hits) = mock_relay_holding(Vec::new()).await;

        let store = Arc::new(nsite_deck::testing::MemRelay::new());
        store
            .publish(relay_list(&keys, &[&["r", &author_relay]]))
            .await
            .unwrap();
        let source = IpPeerSource::new(vec![default_relay], Vec::new())
            .with_author_outbox(outbox_over(store, vec![indexer]));

        let got = source.fetch_manifest(&site.author, None).await.unwrap();
        assert_eq!(got.map(|e| e.id), Some(site.manifest.id));
        assert_eq!(author_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(indexer_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// No list anywhere but an indexer: the indexer's answer is enough.
    #[tokio::test]
    async fn a_missing_list_is_fetched_from_the_indexers() {
        let keys = nostr::Keys::generate();
        let site = nsite_deck::testing::build_test_site_with_keys(
            &keys,
            &[("/index.html", b"indexed")],
            None,
            None,
        );
        let (author_relay, _) = mock_relay_holding(vec![site.manifest.clone()]).await;
        let (default_relay, _) = mock_relay_holding(Vec::new()).await;
        let (indexer, _) =
            mock_relay_holding(vec![relay_list(&keys, &[&["r", &author_relay]])]).await;

        let store = Arc::new(nsite_deck::testing::MemRelay::new());
        let source = IpPeerSource::new(vec![default_relay], Vec::new())
            .with_author_outbox(outbox_over(store, vec![indexer]));

        let got = source.fetch_manifest(&site.author, None).await.unwrap();
        assert_eq!(got.map(|e| e.id), Some(site.manifest.id));
    }

    /// A relay that is a hint, in the author's list and a default is asked
    /// once, however it is spelled.
    #[tokio::test]
    async fn a_relay_named_three_ways_is_asked_once() {
        let keys = nostr::Keys::generate();
        let site = nsite_deck::testing::build_test_site_with_keys(
            &keys,
            &[("/index.html", b"once")],
            None,
            None,
        );
        let (relay, hits) = mock_relay_holding(vec![site.manifest.clone()]).await;
        let store = Arc::new(nsite_deck::testing::MemRelay::new());
        store
            .publish(relay_list(&keys, &[&["r", &format!("{relay}/")]]))
            .await
            .unwrap();
        let source = IpPeerSource::new(vec![relay.clone()], Vec::new())
            .with_relay_hints(vec![format!("{relay}/")])
            .with_author_outbox(outbox_over(store, Vec::new()));

        let got = source.fetch_manifest(&site.author, None).await.unwrap();
        assert_eq!(got.map(|e| e.id), Some(site.manifest.id));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Relays a newer list adds mid-round count against the same cap as the
    /// stored list's: never more than [`MAX_AUTHOR_RELAYS`] from lists.
    #[tokio::test]
    async fn relays_added_mid_round_stay_within_the_cap() {
        let keys = nostr::Keys::generate();
        let author = keys.public_key();
        let mut stored_relays = Vec::new();
        for _ in 0..3 {
            stored_relays.push(mock_relay_holding(Vec::new()).await);
        }
        let mut new_relays = Vec::new();
        for _ in 0..5 {
            new_relays.push(mock_relay_holding(Vec::new()).await);
        }
        let list_of = |relays: &[(String, Arc<std::sync::atomic::AtomicUsize>)], at: u64| {
            nostr::EventBuilder::new(nostr::Kind::RelayList, "")
                .tags(
                    relays
                        .iter()
                        .map(|(url, _)| nostr::Tag::parse(["r", url.as_str()]).unwrap()),
                )
                .custom_created_at(nostr::Timestamp::from(at))
                .sign_with_keys(&keys)
                .unwrap()
        };

        let now = nostr::Timestamp::now().as_secs();
        let store = Arc::new(nsite_deck::testing::MemRelay::new());
        store
            .publish(list_of(&stored_relays, now - 100))
            .await
            .unwrap();
        let newer = list_of(&new_relays, now);
        let (default_relay, _) = mock_relay_holding(vec![newer]).await;

        let source = IpPeerSource::new(vec![default_relay], Vec::new())
            .with_author_outbox(outbox_over(store, Vec::new()));
        let got = source.fetch_manifest(&author, None).await.unwrap();
        assert!(got.is_none());

        assert!(stored_relays.iter().all(|(_, h)| hits(h) == 1));
        let added: usize = new_relays.iter().map(|(_, h)| hits(h)).sum();
        assert_eq!(added, MAX_AUTHOR_RELAYS - stored_relays.len());
    }

    /// A relay that joins late gets what is left of the round, not a fresh
    /// timeout: the lookup ends at the round's deadline.
    #[tokio::test]
    async fn the_round_has_one_deadline() {
        let keys = nostr::Keys::generate();
        let site = nsite_deck::testing::build_test_site_with_keys(
            &keys,
            &[("/index.html", b"slow")],
            None,
            None,
        );
        let (author_relay, _) =
            mock_relay_delayed(vec![site.manifest.clone()], Duration::from_secs(5)).await;
        let (default_relay, _) = mock_relay_delayed(
            vec![relay_list(&keys, &[&["r", &author_relay]])],
            Duration::from_millis(450),
        )
        .await;

        let store = Arc::new(nsite_deck::testing::MemRelay::new());
        let source = IpPeerSource::new(vec![default_relay], Vec::new())
            .with_timeout(Duration::from_millis(600))
            .with_author_outbox(outbox_over(store, Vec::new()));
        let started = std::time::Instant::now();
        let got = source.fetch_manifest(&site.author, None).await.unwrap();
        let took = started.elapsed();
        assert!(got.is_none());
        // A fresh timeout for the late relay would end at ~1050ms.
        assert!(took < Duration::from_millis(850), "took {took:?}");
    }

    /// An author with no list anywhere is remembered: the next lookup does
    /// not ask the indexers again.
    #[tokio::test]
    async fn an_author_without_a_list_is_not_looked_up_again_at_once() {
        let author = nostr::Keys::generate().public_key();
        let (default_relay, _) = mock_relay_holding(Vec::new()).await;
        let (indexer, indexer_hits) = mock_relay_holding(Vec::new()).await;
        let store = Arc::new(nsite_deck::testing::MemRelay::new());
        let source = IpPeerSource::new(vec![default_relay], Vec::new())
            .with_author_outbox(outbox_over(store, vec![indexer]));

        assert!(source
            .fetch_manifest(&author, None)
            .await
            .unwrap()
            .is_none());
        assert!(source
            .fetch_manifest(&author, None)
            .await
            .unwrap()
            .is_none());
        assert_eq!(hits(&indexer_hits), 1);
    }

    /// The update check's batch: relays from stored lists only, no network;
    /// missing lists fetched separately, stored, and misses remembered.
    #[tokio::test]
    async fn stored_write_relays_and_fetch_lists_for_many_authors() {
        let (stored_keys, fetched_keys, absent_keys) = (
            nostr::Keys::generate(),
            nostr::Keys::generate(),
            nostr::Keys::generate(),
        );
        let store = Arc::new(nsite_deck::testing::MemRelay::new());
        store
            .publish(relay_list(&stored_keys, &[&["r", "ws://stored.example"]]))
            .await
            .unwrap();
        let fetched = relay_list(&fetched_keys, &[&["r", "ws://fetched.example", "write"]]);
        let (list_relay, _) = mock_relay_holding(vec![fetched.clone()]).await;

        let outbox = outbox_over(store, Vec::new());
        let authors = [
            stored_keys.public_key(),
            fetched_keys.public_key(),
            absent_keys.public_key(),
        ];
        let (relays, missing) = outbox.stored_write_relays(&authors, 10).await;
        assert_eq!(relays, vec!["ws://stored.example"]);
        assert_eq!(missing, authors[1..].to_vec());

        outbox.fetch_lists(&missing, &[list_relay]).await;
        let (relays, _) = outbox.stored_write_relays(&authors, 10).await;
        assert_eq!(relays, vec!["ws://stored.example", "ws://fetched.example"]);
        assert!(outbox.recently_missed(&absent_keys.public_key()));
        assert!(!outbox.recently_missed(&fetched_keys.public_key()));
    }

    /// Live network check against a real public nsite (the link the user gave).
    /// Ignored by default; run with:
    /// `cargo test -p myco-core fetch_real -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "hits the public internet"]
    async fn fetch_real_nsite_over_ip() {
        let link =
            "https://npub1apgedl4jczacut0dasn0mszyyhhxzlvjcshjkczms47nt2d4eymsku78ws.nsite.lol/";
        let addr = nsite_deck::parse_link(link).expect("parse link");

        let source = IpPeerSource::with_defaults();
        let manifest = source
            .fetch_manifest(&addr.author, addr.d_tag.as_deref())
            .await
            .expect("fetch ok")
            .expect("manifest found on public relays");
        let m = nsite_deck::Manifest::from_event(manifest).expect("parse manifest");
        println!("manifest: title={:?}, {} paths", m.title, m.paths.len());
        assert!(!m.paths.is_empty(), "manifest should map at least one path");

        // Pull + verify the index blob (or the first path).
        let (path, hash) = m
            .paths
            .iter()
            .find(|(p, _)| p.as_str() == "/index.html")
            .or_else(|| m.paths.iter().next())
            .expect("at least one path");
        let blob = source
            .fetch_blob(hash, &m.servers)
            .await
            .expect("blob fetch ok")
            .unwrap_or_else(|| panic!("blob for {path} not found on any Blossom"));
        println!("fetched {path} ({} bytes), sha256 verified", blob.len());
    }

    /// Full path: a Content layer with the IP source installed syncs a pasted
    /// link to `ready`, then serves it locally.
    #[tokio::test]
    async fn open_site_syncs_from_ip_source() {
        use crate::content::Content;
        use nostr::nips::nip19::ToBech32;

        let site = nsite_deck::testing::build_test_site(
            &[("/index.html", b"hello from the internet")],
            None,
            None,
        );
        let relay_url = mock_relay(serde_json::to_string(&site.manifest).unwrap()).await;
        let blossom_url = mock_blossom(site.blobs.clone()).await;

        let dir = std::env::temp_dir().join(format!("myco-ip-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        content.set_source(Arc::new(IpPeerSource::new(
            vec![relay_url],
            vec![blossom_url],
        )));

        let addr = nsite_deck::SiteAddr {
            author: site.author,
            d_tag: None,
        };
        content.clone().open_site(addr, None).await;

        let sites = content.sites_snapshot();
        assert_eq!(sites[0].state, "ready", "site should sync to ready over IP");

        let host = format!("{}.nsite", site.author.to_bech32().unwrap());
        let resp = content.gateway_get(&host, "/", None).await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello from the internet");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
