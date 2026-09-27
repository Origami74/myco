//! The **account**: who is logged in, and the work that follows a login.
//!
//! Owns the user key's slot (read by the napplet [`UserSigner`]), the view
//! the Settings header shows, and the avatar bytes behind it. Every change of
//! account bumps a generation; background work started for one account checks
//! it before writing, so a slow profile fetch cannot land on the next account.
//!
//! Two kinds of background work:
//!
//! - **A new guest** is published: the tinted logo goes to the local store and
//!   to a few public Blossom servers, then the kind 0 and relay lists go to the
//!   public relays. Offline, it waits and retries, and the sidecar remembers
//!   it is pending across launches.
//! - **An imported key** has its kind 0 looked up: the local store first, then
//!   the public relays, and its picture fetched for the header.
//!
//! [`UserSigner`]: crate::user_key::UserSigner

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use nostr::{Event, EventBuilder, Filter, Keys, Kind, PublicKey, Tag, Timestamp, ToBech32};
use nsite_deck::seams::{BlobStore, RelayBackend};
use nsite_deck::sha256_hex;

use crate::state::AccountView;
use crate::user_key::{ExternalUser, Identity, Origin, Slot, Startup, UserKey};

/// Where the guest picture is uploaded, first one first. The profile names
/// the first server that accepted it.
pub fn default_avatar_servers() -> Vec<String> {
    [
        "https://blossom.primal.net",
        "https://blossom.band",
        "https://blossom.ditto.pub",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Retry backoff for a guest profile that could not be published.
const RETRY_FIRST: Duration = Duration::from_secs(30);
const RETRY_MAX: Duration = Duration::from_secs(30 * 60);
/// One relay or Blossom server, one attempt.
const NET_TIMEOUT: Duration = Duration::from_secs(10);
/// An imported profile's picture is for a 40 dp circle; anything bigger is
/// not worth the download.
const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;

/// What the account needs from the rest of Myco.
#[derive(Clone)]
pub struct AccountContext {
    /// The store the user's own events go to (the embedded relay, or the
    /// configured custom one).
    pub relay: Arc<dyn RelayBackend>,
    pub blobs: Arc<dyn BlobStore>,
    /// Read before every internet attempt, so flipping "offline only" takes
    /// effect on the next retry.
    pub offline_only: Arc<dyn Fn() -> bool + Send + Sync>,
    pub relays: Vec<String>,
    pub avatar_servers: Vec<String>,
}

#[derive(Default)]
struct Shared {
    view: AccountView,
    avatar: Option<Arc<Vec<u8>>>,
}

#[derive(Clone)]
pub struct Account {
    data_dir: PathBuf,
    slot: Slot,
    shared: Arc<Mutex<Shared>>,
    generation: Arc<AtomicU64>,
    ctx: AccountContext,
    handle: tokio::runtime::Handle,
    /// Where a signer-app login's requests wait for Kotlin.
    bridge: Arc<crate::external_signer::SignerBridge>,
}

impl Account {
    /// Load (or, on a first launch, create) the account and start whatever
    /// work it needs.
    pub fn start(data_dir: PathBuf, ctx: AccountContext, handle: tokio::runtime::Handle) -> Self {
        let account = Self {
            data_dir,
            slot: Arc::new(RwLock::new(None)),
            shared: Arc::new(Mutex::new(Shared::default())),
            generation: Arc::new(AtomicU64::new(0)),
            ctx,
            handle,
            bridge: Arc::new(crate::external_signer::SignerBridge::default()),
        };
        match crate::user_key::startup(&account.data_dir) {
            Ok(Startup::Existing(user)) => account.activate(user, false),
            Ok(Startup::NewGuest(user)) => {
                tracing::info!("generated a guest account: {}", user.guest_name());
                account.activate(user, true)
            }
            Ok(Startup::Signer(user)) => account.activate_signer(user),
            Ok(Startup::LoggedOut) => account.set_logged_out(String::new()),
            Err(e) => {
                // A key that cannot be read is not silently replaced: the
                // person may still have a copy that can be pasted back.
                tracing::error!(error = %e, "user key unreadable");
                account.set_logged_out(format!("Your saved key could not be read: {e}"));
            }
        }
        account
    }

    /// The slot the napplet signer reads.
    pub fn slot(&self) -> Slot {
        self.slot.clone()
    }

    /// The queue a signer-app login's requests wait in, for the JNI pump.
    pub fn bridge(&self) -> Arc<crate::external_signer::SignerBridge> {
        self.bridge.clone()
    }

    pub fn view(&self) -> AccountView {
        self.lock().view.clone()
    }

    pub fn avatar(&self) -> Option<Arc<Vec<u8>>> {
        self.lock().avatar.clone()
    }

    pub fn public_key(&self) -> Option<PublicKey> {
        self.identity().map(|i| i.public_key())
    }

    /// The logged-in secret, as `nsec1…`, for the Account page's reveal.
    /// `None` for a signer-app login: the secret is not here to reveal.
    pub fn reveal_nsec(&self) -> Option<String> {
        self.keys()?.secret_key().to_bech32().ok()
    }

    /// Log out: the key leaves the disk and the slot, and napplets have no
    /// identity until the next login.
    pub fn logout(&self) -> anyhow::Result<()> {
        crate::user_key::logout(&self.data_dir)?;
        self.set_logged_out(String::new());
        tracing::info!("logged out");
        Ok(())
    }

    /// Log in as a new guest.
    pub fn new_guest(&self) -> anyhow::Result<()> {
        let user = crate::user_key::generate_guest(&self.data_dir)?;
        tracing::info!("generated a guest account: {}", user.guest_name());
        self.activate(user, true);
        Ok(())
    }

    /// Log in with a pasted secret. A failure is kept on the view, for the
    /// login sheet to show, as well as returned.
    pub fn login_nsec(&self, secret: &str) -> anyhow::Result<()> {
        match crate::user_key::import(&self.data_dir, secret) {
            Ok(user) => {
                self.activate(user, false);
                Ok(())
            }
            Err(e) => {
                self.lock().view.error = e.to_string();
                Err(e)
            }
        }
    }

    /// Log in with a signer app (NIP-55): its answer to `get_public_key` and
    /// its package, from the intent Kotlin ran. A failure is kept on the view.
    pub fn login_signer(&self, pubkey: &str, package: &str) -> anyhow::Result<()> {
        match crate::user_key::login_signer(&self.data_dir, pubkey, package) {
            Ok(user) => {
                tracing::info!(package = %user.package, "logged in with a signer app");
                self.activate_signer(user);
                Ok(())
            }
            Err(e) => {
                self.lock().view.error = e.to_string();
                Err(e)
            }
        }
    }

    /// Re-read the profile from the local store — a napplet may have
    /// published a new kind 0 since.
    pub fn refresh(&self) {
        let Some(pk) = self.public_key() else { return };
        let this = self.clone();
        let generation = self.generation.load(Ordering::SeqCst);
        self.handle.spawn(async move {
            if let Some(profile) = this.local_profile(&pk).await {
                this.apply_profile(generation, &profile);
            }
        });
    }

    fn activate(&self, user: UserKey, new_guest: bool) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.slot.write().unwrap_or_else(|p| p.into_inner()) =
            Some(Identity::Local(user.keys.clone()));
        let pk = user.keys.public_key();
        let pending = user.origin == Origin::Guest
            && (new_guest || crate::user_key::profile_pending(&self.data_dir));
        {
            let mut shared = self.lock();
            let avatar_rev = shared.view.avatar_rev + 1;
            shared.avatar = None;
            shared.view = AccountView {
                status: user.origin.as_str().to_string(),
                npub: pk.to_bech32().unwrap_or_default(),
                pubkey_hex: pk.to_hex(),
                name: match user.origin {
                    Origin::Guest => user.guest_name(),
                    Origin::Nsec => String::new(),
                },
                about: String::new(),
                picture: String::new(),
                avatar_rev,
                publish_pending: pending,
                profile_loading: user.origin == Origin::Nsec,
                signer_package: String::new(),
                error: String::new(),
            };
        }

        let this = self.clone();
        match user.origin {
            Origin::Guest => {
                self.handle.spawn(async move {
                    this.run_guest(generation, user, new_guest, pending).await
                });
            }
            Origin::Nsec => {
                self.handle
                    .spawn(async move { this.run_imported(generation, pk).await });
            }
        }
    }

    /// A signer-app login: no key here, the profile looked up like an
    /// imported one's.
    fn activate_signer(&self, user: ExternalUser) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let pk = user.pubkey;
        *self.slot.write().unwrap_or_else(|p| p.into_inner()) =
            Some(Identity::External(user.clone()));
        {
            let mut shared = self.lock();
            let avatar_rev = shared.view.avatar_rev + 1;
            shared.avatar = None;
            shared.view = AccountView {
                status: "signer".to_string(),
                npub: pk.to_bech32().unwrap_or_default(),
                pubkey_hex: pk.to_hex(),
                avatar_rev,
                profile_loading: true,
                signer_package: user.package,
                ..AccountView::default()
            };
        }
        let this = self.clone();
        self.handle
            .spawn(async move { this.run_imported(generation, pk).await });
    }

    fn set_logged_out(&self, error: String) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.slot.write().unwrap_or_else(|p| p.into_inner()) = None;
        let mut shared = self.lock();
        let avatar_rev = shared.view.avatar_rev + 1;
        shared.avatar = None;
        shared.view = AccountView {
            status: "logged_out".to_string(),
            avatar_rev,
            error,
            ..AccountView::default()
        };
    }

    // --- a guest ----------------------------------------------------------

    async fn run_guest(&self, generation: u64, user: UserKey, new_guest: bool, pending: bool) {
        let pk = user.keys.public_key();
        let avatar = match crate::guest_avatar::render(&pk) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::warn!(error = %e, "could not draw the guest avatar");
                None
            }
        };
        let sha = avatar.as_deref().map(sha256_hex);
        if let Some(bytes) = &avatar {
            if let Err(e) = self.ctx.blobs.put(bytes).await {
                tracing::warn!(error = %e, "could not store the guest avatar");
            }
        }
        let default_picture = sha.as_deref().and_then(|sha| {
            self.ctx
                .avatar_servers
                .first()
                .map(|server| avatar_url(server, sha))
        });

        if new_guest {
            // Stored first, published later: a guest has a name and a face on
            // this phone from the first second, internet or not.
            for (what, signed) in [
                (
                    "profile",
                    sign_guest_profile(&user, default_picture.as_deref()),
                ),
                ("relay list", crate::outbox::own_relay_list(&user.keys)),
                (
                    "DM relay list",
                    crate::outbox::own_dm_relay_list(&user.keys),
                ),
                ("follow list", sign_guest_follows(&user.keys)),
            ] {
                match signed {
                    Ok(event) => {
                        if let Err(e) = self.ctx.relay.publish(event).await {
                            tracing::warn!("could not store the guest {what}: {e}");
                        }
                    }
                    Err(e) => tracing::warn!("could not sign the guest {what}: {e}"),
                }
            }
        }

        let profile = self.local_profile(&pk).await;
        if !self.is_current(generation) {
            return;
        }
        {
            let mut shared = self.lock();
            // The guest picture stands in for a profile that names none — a
            // guest from before pictures — on this phone only.
            shared.avatar = avatar.clone().map(Arc::new);
            shared.view.avatar_rev += 1;
        }
        if let Some(profile) = &profile {
            self.apply_profile(generation, profile);
        }

        if pending {
            if let (Some(bytes), Some(sha)) = (avatar, sha) {
                self.publish_guest_until_done(generation, &user, &bytes, &sha)
                    .await;
            }
        }
    }

    /// Keep trying until the guest profile reaches a public relay, or the
    /// account changes.
    async fn publish_guest_until_done(
        &self,
        generation: u64,
        user: &UserKey,
        avatar: &[u8],
        sha: &str,
    ) {
        let mut delay = RETRY_FIRST;
        loop {
            if !self.is_current(generation) {
                return;
            }
            if !(self.ctx.offline_only)() {
                match self.publish_guest_once(user, avatar, sha).await {
                    Ok(true) => {
                        crate::user_key::mark_profile_published(&self.data_dir);
                        if self.is_current(generation) {
                            self.lock().view.publish_pending = false;
                        }
                        tracing::info!("guest profile published");
                        return;
                    }
                    Ok(false) => tracing::debug!("guest profile not published yet; retrying"),
                    Err(e) => tracing::warn!(error = %e, "guest profile publish failed"),
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(RETRY_MAX);
        }
    }

    /// One attempt: upload the picture, then publish the profile and relay
    /// lists. `Ok(true)` once the profile and each stored relay list have
    /// each reached at least one relay; anything less stays pending and is
    /// tried again.
    async fn publish_guest_once(
        &self,
        user: &UserKey,
        avatar: &[u8],
        sha: &str,
    ) -> anyhow::Result<bool> {
        let accepted = upload_everywhere(&user.keys, &self.ctx.avatar_servers, avatar, sha).await;
        let pk = user.keys.public_key();

        let mut profile = self.local_profile(&pk).await;
        // The stored profile names the first server on the list. If that one
        // refused the upload and another took it, point the profile there —
        // but only while it is still the untouched guest profile: one the
        // person edited through a napplet is theirs.
        if let Some(first_ok) = accepted.first() {
            let url = avatar_url(first_ok, sha);
            let stale = match &profile {
                Some(event) => {
                    is_untouched_guest(event, user)
                        && picture_of(event).as_deref() != Some(url.as_str())
                }
                None => true,
            };
            if stale {
                let event = sign_guest_profile(user, Some(&url))?;
                let _ = self.ctx.relay.publish(event.clone()).await;
                profile = Some(event);
            }
        }
        let Some(profile) = profile else {
            // A custom relay that is down and no picture uploaded: there is
            // nothing to publish yet.
            return Ok(false);
        };
        let events = self.guest_events(&pk, profile).await;
        let relays = guest_publish_relays(&self.ctx.relays);
        let sent = publish_everywhere(&relays, &events).await;
        let counts: Vec<String> = events
            .iter()
            .zip(&sent)
            .map(|(event, n)| format!("{}:{n}", event.kind.as_u16()))
            .collect();
        tracing::info!(relays = relays.len(), sent = ?counts, "guest publish attempt");
        Ok(guest_publish_done(&events, &sent))
    }

    /// What a guest publishes: the kind 0 first, then whichever of its relay
    /// lists (10002, 10050) and follows are in the local store, sent as they
    /// are — the person may have changed them. Nothing is signed here: a
    /// guest from before a list existed keeps what it has, and gets the
    /// current defaults only by starting a new guest.
    async fn guest_events(&self, pk: &PublicKey, profile: Event) -> Vec<Event> {
        let mut events = vec![profile];
        for kind in [Kind::RelayList, Kind::InboxRelays, Kind::ContactList] {
            if let Ok(mut found) = self
                .ctx
                .relay
                .query(&[Filter::new().author(*pk).kind(kind).limit(1)])
                .await
            {
                if !found.is_empty() {
                    events.push(found.remove(0));
                }
            }
        }
        events
    }

    // --- an imported key --------------------------------------------------

    async fn run_imported(&self, generation: u64, pk: PublicKey) {
        let mut profile = self.local_profile(&pk).await;
        if profile.is_none() && !(self.ctx.offline_only)() {
            profile = fetch_profile(&self.ctx.relays, &pk).await;
            if let Some(event) = &profile {
                let _ = self.ctx.relay.publish(event.clone()).await;
            }
        }
        if !self.is_current(generation) {
            return;
        }
        self.lock().view.profile_loading = false;
        if let Some(profile) = profile {
            self.apply_profile(generation, &profile);
        }
    }

    /// Show a kind 0, and fetch its picture when it names one we do not have.
    fn apply_profile(&self, generation: u64, event: &Event) {
        let Ok(meta) = serde_json::from_str::<serde_json::Value>(&event.content) else {
            return;
        };
        let text = |key: &str| {
            meta.get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        let name = text("display_name").or_else(|| text("name"));
        let picture = text("picture").unwrap_or_default();
        {
            let mut shared = self.lock();
            if !self.is_current(generation) {
                return;
            }
            if let Some(name) = name {
                shared.view.name = name;
            }
            shared.view.about = text("about").unwrap_or_default();
            if shared.view.picture == picture {
                return;
            }
            shared.view.picture = picture.clone();
        }
        if picture.is_empty() {
            return;
        }
        let this = self.clone();
        self.handle.spawn(async move {
            if let Some(bytes) = this.load_picture(&picture).await {
                if this.is_current(generation) {
                    let mut shared = this.lock();
                    shared.avatar = Some(Arc::new(bytes));
                    shared.view.avatar_rev += 1;
                }
            }
        });
    }

    /// A profile picture: from the local store when the URL names a blob by
    /// its hash (the guest picture always does), otherwise over the internet.
    async fn load_picture(&self, url: &str) -> Option<Vec<u8>> {
        let sha = blob_hash_in(url);
        if let Some(sha) = &sha {
            if let Ok(Some(bytes)) = self.ctx.blobs.get(sha).await {
                return Some(bytes);
            }
        }
        if (self.ctx.offline_only)() || !url.starts_with("https://") {
            return None;
        }
        let bytes = download(url).await?;
        if let Some(sha) = &sha {
            if sha256_hex(&bytes) != *sha {
                tracing::debug!(url, "a profile picture did not match its hash");
                return None;
            }
            let _ = self.ctx.blobs.put(&bytes).await;
        }
        Some(bytes)
    }

    // --- helpers ----------------------------------------------------------

    async fn local_profile(&self, pk: &PublicKey) -> Option<Event> {
        let found = self
            .ctx
            .relay
            .query(&[Filter::new().author(*pk).kind(Kind::Metadata).limit(1)])
            .await
            .ok()?;
        found.into_iter().max_by_key(|e| e.created_at)
    }

    fn identity(&self) -> Option<Identity> {
        self.slot.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The local key, for a guest or an imported nsec; `None` for a
    /// signer-app login.
    fn keys(&self) -> Option<Keys> {
        match self.identity()? {
            Identity::Local(keys) => Some(keys),
            Identity::External(_) => None,
        }
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Who a new guest follows, so a napplet's friends feed is not empty on day
/// one. Only ever signed when a guest is created: an existing follow list —
/// a guest's, after edits, or an imported identity's — is never touched.
pub const GUEST_FOLLOWS: [&str; 3] = [
    "npub1hw6amg8p24ne08c9gdq8hhpqx0t0pwanpae9z25crn7m9uy7yarse465gr",
    "npub1ye5ptcxfyyxl5vjvdjar2ua3f0hynkjzpx552mu5snj3qmx5pzjscpknpr",
    "npub1uac67zc9er54ln0kl6e4qp2y6ta3enfcg7ywnayshvlw9r5w6ehsqq99rx",
];

fn sign_guest_follows(keys: &Keys) -> anyhow::Result<Event> {
    use nostr::FromBech32;
    let tags = GUEST_FOLLOWS
        .iter()
        .map(|npub| Ok(Tag::public_key(PublicKey::from_bech32(npub)?)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(EventBuilder::new(Kind::ContactList, "")
        .tags(tags)
        .sign_with_keys(keys)?)
}

fn sign_guest_profile(user: &UserKey, picture: Option<&str>) -> anyhow::Result<Event> {
    Ok(EventBuilder::new(
        Kind::Metadata,
        crate::user_key::guest_profile_json(user, picture),
    )
    .sign_with_keys(&user.keys)?)
}

/// Whether a stored kind 0 is still the one Myco made for this guest.
fn is_untouched_guest(event: &Event, user: &UserKey) -> bool {
    serde_json::from_str::<serde_json::Value>(&event.content)
        .ok()
        .map(|meta| {
            meta.get("name").and_then(|n| n.as_str()) == Some(user.guest_name().as_str())
                && meta.get("about").and_then(|a| a.as_str()) == Some(crate::user_key::GUEST_ABOUT)
        })
        .unwrap_or(false)
}

fn picture_of(event: &Event) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&event.content)
        .ok()?
        .get("picture")?
        .as_str()
        .map(String::from)
}

fn avatar_url(server: &str, sha: &str) -> String {
    format!(
        "{}/{sha}.{}",
        server.trim_end_matches('/'),
        crate::guest_avatar::EXTENSION
    )
}

/// The sha256 a Blossom-style URL names in its last path segment, if any.
fn blob_hash_in(url: &str) -> Option<String> {
    let last = url.split(['?', '#']).next()?.rsplit('/').next()?;
    let stem = last.split('.').next()?;
    (stem.len() == 64 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| stem.to_ascii_lowercase())
}

/// BUD-02 upload to every server at once; the servers that took it, in list
/// order.
async fn upload_everywhere(
    keys: &Keys,
    servers: &[String],
    bytes: &[u8],
    sha: &str,
) -> Vec<String> {
    let auth = match blossom_upload_auth(keys, sha) {
        Ok(auth) => auth,
        Err(e) => {
            tracing::warn!(error = %e, "could not sign the Blossom upload");
            return Vec::new();
        }
    };
    let http = reqwest::Client::builder()
        .timeout(NET_TIMEOUT)
        .build()
        .unwrap_or_default();
    let results = futures_util::future::join_all(servers.iter().map(|server| {
        let http = http.clone();
        let auth = auth.clone();
        let body = bytes.to_vec();
        async move {
            let url = format!("{}/upload", server.trim_end_matches('/'));
            let response = http
                .put(&url)
                .header("Authorization", auth)
                .header("Content-Type", crate::guest_avatar::MIME)
                .body(body)
                .send()
                .await;
            match response {
                Ok(r) if r.status().is_success() => true,
                Ok(r) => {
                    tracing::debug!(server, status = %r.status(), "Blossom refused the avatar");
                    false
                }
                Err(e) => {
                    tracing::debug!(server, error = %e, "Blossom upload failed");
                    false
                }
            }
        }
    }))
    .await;
    servers
        .iter()
        .zip(results)
        .filter(|(_, ok)| *ok)
        .map(|(s, _)| s.clone())
        .collect()
}

/// The `Authorization` header for a BUD-02 upload of `sha`.
fn blossom_upload_auth(keys: &Keys, sha: &str) -> anyhow::Result<String> {
    use base64::Engine;
    let expires = Timestamp::from(Timestamp::now().as_secs() + 600);
    let event = EventBuilder::new(Kind::BlossomAuth, "Upload a Myco guest avatar")
        .tags([
            Tag::parse(["t", "upload"])?,
            Tag::parse(["x", sha])?,
            Tag::expiration(expires),
        ])
        .sign_with_keys(keys)?;
    let json = serde_json::to_string(&event)?;
    Ok(format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(json)
    ))
}

/// Where a guest's profile and relay lists go: the configured relays, and
/// every relay its own lists name — a list naming an outbox that never got
/// the events would point readers at nothing. Deduplicated, first one first.
fn guest_publish_relays(configured: &[String]) -> Vec<String> {
    let mut relays: Vec<String> = Vec::new();
    let named = crate::outbox::USER_OUTBOX_RELAYS
        .iter()
        .chain(crate::outbox::USER_INBOX_RELAYS.iter())
        .map(|url| url.to_string());
    for url in configured.iter().cloned().chain(named) {
        let key = url.trim_end_matches('/');
        if !relays.iter().any(|r| r.trim_end_matches('/') == key) {
            relays.push(url);
        }
    }
    relays
}

/// Whether a guest publish is done, from [`publish_everywhere`]'s count for
/// each event: the profile and every relay list sent must each have reached a
/// relay. The follows are best-effort.
fn guest_publish_done(events: &[Event], sent: &[usize]) -> bool {
    events.len() == sent.len()
        && events.iter().zip(sent).all(|(event, n)| {
            *n > 0
                || !matches!(
                    event.kind,
                    Kind::Metadata | Kind::RelayList | Kind::InboxRelays
                )
        })
}

/// Send each event to every relay; how many relays took each one.
async fn publish_everywhere(relays: &[String], events: &[Event]) -> Vec<usize> {
    let mut counts = Vec::with_capacity(events.len());
    for event in events {
        let results = futures_util::future::join_all(relays.iter().map(|url| async move {
            matches!(
                tokio::time::timeout(NET_TIMEOUT, crate::ip_source::publish_to_relay(url, event))
                    .await,
                Ok(Ok(true))
            )
        }))
        .await;
        counts.push(results.into_iter().filter(|ok| *ok).count());
    }
    counts
}

/// The newest kind 0 for `pk` any relay has.
async fn fetch_profile(relays: &[String], pk: &PublicKey) -> Option<Event> {
    let filter = serde_json::json!({ "kinds": [0], "authors": [pk.to_hex()], "limit": 1 });
    let answers = futures_util::future::join_all(relays.iter().map(|url| {
        let filter = filter.clone();
        async move {
            tokio::time::timeout(NET_TIMEOUT, crate::ip_source::query_relay(url, filter))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default()
        }
    }))
    .await;
    answers
        .into_iter()
        .flatten()
        .filter(|e| e.pubkey == *pk && e.kind == Kind::Metadata && e.verify().is_ok())
        .max_by_key(|e| e.created_at)
}

async fn download(url: &str) -> Option<Vec<u8>> {
    let http = reqwest::Client::builder()
        .timeout(NET_TIMEOUT)
        .build()
        .ok()?;
    let mut response = http.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_AVATAR_BYTES {
            return None;
        }
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsite_deck::testing::{MemBlobs, MemRelay};

    fn temp_dir() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "myco-account-{}-{}-{n}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// No internet in tests: offline-only on, no relays, no servers.
    fn offline(relay: Arc<MemRelay>, blobs: Arc<MemBlobs>) -> AccountContext {
        AccountContext {
            relay,
            blobs,
            offline_only: Arc::new(|| true),
            relays: Vec::new(),
            avatar_servers: default_avatar_servers(),
        }
    }

    async fn settle<F: Fn() -> bool>(done: F) {
        for _ in 0..200 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("never settled");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_first_launch_is_a_guest_with_a_stored_profile_and_picture() {
        let relay = Arc::new(MemRelay::new());
        let blobs = Arc::new(MemBlobs::new());
        let dir = temp_dir();
        let account = Account::start(
            dir.clone(),
            offline(relay.clone(), blobs.clone()),
            tokio::runtime::Handle::current(),
        );

        let view = account.view();
        assert_eq!(view.status, "guest");
        assert!(view.name.starts_with("Myco Guest "));
        assert!(
            view.publish_pending,
            "offline, the profile is still to go out"
        );
        settle(|| account.avatar().is_some() && !account.view().picture.is_empty()).await;

        let view = account.view();
        let sha = blob_hash_in(&view.picture).expect("the picture names its blob");
        assert!(view.picture.starts_with("https://blossom.primal.net/"));
        assert!(blobs.has(&sha).await, "the picture is in the local store");
        assert_eq!(sha256_hex(&account.avatar().unwrap()), sha);

        let pk = account.public_key().unwrap();
        let stored = relay
            .query(&[Filter::new().author(pk).kind(Kind::Metadata)])
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].content.contains("https://getmyco.app"));

        let follows = relay
            .query(&[Filter::new().author(pk).kind(Kind::ContactList)])
            .await
            .unwrap();
        assert_eq!(follows.len(), 1, "a new guest follows the defaults");
        let followed: Vec<String> = follows[0]
            .tags
            .public_keys()
            .map(|p| nostr::ToBech32::to_bech32(p).unwrap())
            .collect();
        assert_eq!(followed, GUEST_FOLLOWS);

        for kind in [Kind::RelayList, Kind::InboxRelays] {
            let lists = relay
                .query(&[Filter::new().author(pk).kind(kind)])
                .await
                .unwrap();
            assert_eq!(lists.len(), 1, "a new guest has a kind {kind} stored");
        }

        // Still pending on the next launch, and the same account.
        let again = Account::start(
            dir,
            offline(relay, blobs),
            tokio::runtime::Handle::current(),
        );
        assert_eq!(again.public_key(), Some(pk));
        assert!(again.view().publish_pending);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn logout_forgets_the_key_and_stays_logged_out() {
        let relay = Arc::new(MemRelay::new());
        let blobs = Arc::new(MemBlobs::new());
        let dir = temp_dir();
        let account = Account::start(
            dir.clone(),
            offline(relay.clone(), blobs.clone()),
            tokio::runtime::Handle::current(),
        );
        let nsec = account.reveal_nsec().unwrap();
        assert!(nsec.starts_with("nsec1"));

        account.logout().unwrap();
        assert_eq!(account.view().status, "logged_out");
        assert!(account.reveal_nsec().is_none());
        assert!(
            account.slot().read().unwrap().is_none(),
            "the signer still has a key"
        );

        let relaunched = Account::start(
            dir,
            offline(relay, blobs),
            tokio::runtime::Handle::current(),
        );
        assert_eq!(relaunched.view().status, "logged_out");

        // The revealed nsec brings the same identity back.
        relaunched.login_nsec(&nsec).unwrap();
        assert_eq!(relaunched.view().status, "nsec");
        assert_eq!(relaunched.public_key(), account_pk(&nsec));
    }

    fn account_pk(nsec: &str) -> Option<PublicKey> {
        Keys::parse(nsec).ok().map(|k| k.public_key())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_imported_key_shows_its_stored_profile() {
        let relay = Arc::new(MemRelay::new());
        let blobs = Arc::new(MemBlobs::new());
        let theirs = Keys::generate();
        let profile = EventBuilder::new(
            Kind::Metadata,
            r#"{"name":"alice","display_name":"Alice","about":"hi"}"#,
        )
        .sign_with_keys(&theirs)
        .unwrap();
        relay.publish(profile).await.unwrap();

        let dir = temp_dir();
        crate::user_key::logout(&dir).unwrap();
        let account = Account::start(
            dir,
            offline(relay.clone(), blobs),
            tokio::runtime::Handle::current(),
        );
        assert_eq!(account.view().status, "logged_out");

        assert!(account.login_nsec("npub1whatever").is_err());
        assert!(!account.view().error.is_empty(), "the failure is not shown");

        account
            .login_nsec(&theirs.secret_key().to_bech32().unwrap())
            .unwrap();
        settle(|| account.view().name == "Alice").await;
        let view = account.view();
        assert_eq!(view.about, "hi");
        assert!(!view.profile_loading);
        assert!(view.error.is_empty());
        let follows = relay
            .query(&[Filter::new()
                .author(theirs.public_key())
                .kind(Kind::ContactList)])
            .await
            .unwrap();
        assert!(
            follows.is_empty(),
            "an imported identity got default follows"
        );
        let lists = relay
            .query(&[Filter::new()
                .author(theirs.public_key())
                .kinds([Kind::RelayList, Kind::InboxRelays])])
            .await
            .unwrap();
        assert!(lists.is_empty(), "an imported identity got relay lists");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_signer_login_has_no_secret_to_reveal_and_survives_a_relaunch() {
        let relay = Arc::new(MemRelay::new());
        let blobs = Arc::new(MemBlobs::new());
        let dir = temp_dir();
        let account = Account::start(
            dir.clone(),
            offline(relay.clone(), blobs.clone()),
            tokio::runtime::Handle::current(),
        );
        account.logout().unwrap();
        let theirs = Keys::generate().public_key();

        assert!(account
            .login_signer("garbage", "com.greenart7c3.nostrsigner")
            .is_err());
        assert!(!account.view().error.is_empty());

        account
            .login_signer(&theirs.to_bech32().unwrap(), "com.greenart7c3.nostrsigner")
            .unwrap();
        let view = account.view();
        assert_eq!(view.status, "signer");
        assert_eq!(view.signer_package, "com.greenart7c3.nostrsigner");
        assert_eq!(account.public_key(), Some(theirs));
        assert!(
            account.reveal_nsec().is_none(),
            "a signer login revealed a secret"
        );

        let relaunched = Account::start(
            dir,
            offline(relay, blobs),
            tokio::runtime::Handle::current(),
        );
        assert_eq!(relaunched.view().status, "signer");
        assert_eq!(relaunched.public_key(), Some(theirs));
    }

    fn tag_rows(event: &Event) -> Vec<Vec<String>> {
        event.tags.iter().map(|t| t.as_slice().to_vec()).collect()
    }

    /// A new guest's publish sends the lists stored at creation, with the
    /// default relays.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_guest_publishes_its_stored_relay_lists() {
        let relay = Arc::new(MemRelay::new());
        let blobs = Arc::new(MemBlobs::new());
        let account = Account::start(
            temp_dir(),
            offline(relay.clone(), blobs),
            tokio::runtime::Handle::current(),
        );
        let pk = account.public_key().unwrap();
        settle(|| !account.view().picture.is_empty()).await;
        let profile = account.local_profile(&pk).await.unwrap();

        let events = account.guest_events(&pk, profile).await;
        let kinds: Vec<Kind> = events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Metadata,
                Kind::RelayList,
                Kind::InboxRelays,
                Kind::ContactList
            ]
        );
        assert_eq!(
            tag_rows(&events[1]),
            [
                ["r", "wss://relay.damus.io"],
                ["r", "wss://relay.ditto.pub"],
                ["r", "wss://relay.primal.net"],
            ]
        );
        assert_eq!(
            tag_rows(&events[2]),
            [
                ["relay", "wss://relay.damus.io"],
                ["relay", "wss://relay.ditto.pub"],
                ["relay", "wss://relay.primal.net"],
            ]
        );
    }

    /// The publish path sends the stored lists as they are — the person may
    /// have changed them — and never signs one: a guest from before a list
    /// existed keeps what it has.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_guest_publish_reuses_the_stored_relay_lists_and_signs_none() {
        let relay = Arc::new(MemRelay::new());
        let blobs = Arc::new(MemBlobs::new());
        let dir = temp_dir();
        crate::user_key::logout(&dir).unwrap();
        let account = Account::start(
            dir,
            offline(relay.clone(), blobs),
            tokio::runtime::Handle::current(),
        );

        // Nothing stored: only the profile goes, and nothing is signed.
        let old = Keys::generate();
        let profile = EventBuilder::new(Kind::Metadata, "{}")
            .sign_with_keys(&old)
            .unwrap();
        let events = account.guest_events(&old.public_key(), profile).await;
        let kinds: Vec<Kind> = events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [Kind::Metadata]);
        let signed = relay
            .query(&[Filter::new().author(old.public_key())])
            .await
            .unwrap();
        assert!(signed.is_empty(), "the publish path stored a new list");

        // Stored ones — edited by the person — go out unchanged.
        let keys = Keys::generate();
        let profile = EventBuilder::new(Kind::Metadata, "{}")
            .sign_with_keys(&keys)
            .unwrap();
        let edited_relays = EventBuilder::new(Kind::RelayList, "")
            .tags([Tag::parse(["r", "wss://mine.example"]).unwrap()])
            .sign_with_keys(&keys)
            .unwrap();
        let edited_dm = EventBuilder::new(Kind::InboxRelays, "")
            .tags([Tag::parse(["relay", "wss://dm.example"]).unwrap()])
            .sign_with_keys(&keys)
            .unwrap();
        let follows = sign_guest_follows(&keys).unwrap();
        for event in [&edited_relays, &edited_dm, &follows] {
            relay.publish(event.clone()).await.unwrap();
        }
        let events = account.guest_events(&keys.public_key(), profile).await;
        let ids: Vec<_> = events.iter().skip(1).map(|e| e.id).collect();
        assert_eq!(ids, [edited_relays.id, edited_dm.id, follows.id]);
    }

    /// A publish is done only when the profile and every relay list sent
    /// each reached a relay; the follows are best-effort. The network send
    /// itself is not host-tested — the mocks have no internet relay.
    #[test]
    fn a_failed_list_publish_keeps_the_guest_pending() {
        let keys = Keys::generate();
        let event = |kind: Kind| EventBuilder::new(kind, "").sign_with_keys(&keys).unwrap();
        let all = [
            event(Kind::Metadata),
            event(Kind::RelayList),
            event(Kind::InboxRelays),
            event(Kind::ContactList),
        ];
        assert!(guest_publish_done(&all, &[1, 1, 1, 1]));
        assert!(
            guest_publish_done(&all, &[2, 1, 3, 0]),
            "follows are best-effort"
        );
        assert!(
            !guest_publish_done(&all, &[1, 0, 1, 1]),
            "the relay list failed"
        );
        assert!(
            !guest_publish_done(&all, &[1, 1, 0, 1]),
            "the DM list failed"
        );
        assert!(
            !guest_publish_done(&all, &[0, 1, 1, 1]),
            "the profile failed"
        );
        // An older guest with no lists stored: the profile alone decides.
        assert!(guest_publish_done(&all[..1], &[1]));
        assert!(!guest_publish_done(&all[..1], &[0]));
    }

    /// The events go to the configured relays and to every relay the lists
    /// name, once each.
    #[test]
    fn a_guest_publishes_to_the_relays_its_lists_name() {
        let configured = vec![
            "wss://relay.damus.io/".to_string(),
            "wss://purplepag.es".to_string(),
        ];
        assert_eq!(
            guest_publish_relays(&configured),
            [
                "wss://relay.damus.io/",
                "wss://purplepag.es",
                "wss://relay.ditto.pub",
                "wss://relay.primal.net",
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_guest_after_logout_is_someone_else() {
        let relay = Arc::new(MemRelay::new());
        let blobs = Arc::new(MemBlobs::new());
        let account = Account::start(
            temp_dir(),
            offline(relay, blobs),
            tokio::runtime::Handle::current(),
        );
        let first = account.public_key().unwrap();
        account.logout().unwrap();
        account.new_guest().unwrap();
        let second = account.public_key().unwrap();
        assert_ne!(first, second);
        assert_eq!(account.view().status, "guest");
    }

    #[test]
    fn a_blossom_url_names_its_hash() {
        let sha = "a".repeat(64);
        assert_eq!(
            blob_hash_in(&format!("https://x.example/{sha}.jpg")),
            Some(sha.clone())
        );
        assert_eq!(
            blob_hash_in(&format!("https://x.example/{sha}")),
            Some(sha.clone())
        );
        assert_eq!(blob_hash_in("https://x.example/me.jpg"), None);
        assert_eq!(
            avatar_url("https://x.example/", &sha),
            format!("https://x.example/{sha}.jpg")
        );
    }

    #[test]
    fn the_upload_auth_is_a_signed_blossom_event() {
        use base64::Engine;
        let keys = Keys::generate();
        let header = blossom_upload_auth(&keys, &"b".repeat(64)).unwrap();
        let json = base64::engine::general_purpose::STANDARD
            .decode(header.strip_prefix("Nostr ").unwrap())
            .unwrap();
        let event: Event = serde_json::from_slice(&json).unwrap();
        assert!(event.verify().is_ok());
        assert_eq!(event.kind, Kind::BlossomAuth);
        assert!(event.tags.iter().any(|t| t.as_slice() == ["t", "upload"]));
    }
}
