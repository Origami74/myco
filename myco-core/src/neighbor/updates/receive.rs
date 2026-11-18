//! Receiving: the standing update subscriptions, and the one hand-off to
//! the runtime's [`UpdateSink`].

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use nostr::{Event, Filter};
use tokio::sync::{broadcast, mpsc};

use super::super::data::AbortOnDrop;
use super::super::extensions::{MYCO_RELEASE, NAPPLET_UPDATES, NSITE_UPDATES};
use super::super::id::NeighborhoodId;
use super::super::Neighbors;
use super::{extension_for, kinds_for, UpdateSink};

/// How often the standing subscriptions are matched to the neighborhoods
/// and their policy, besides whenever links change.
const RECHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

impl Neighbors {
    /// Where updates from linked peers are handed, and start receiving them:
    /// the standing update subscriptions open, and every update that lands
    /// here from a peer goes to `sink`. Set once, from within a Tokio
    /// runtime; runs for as long as `self` lives.
    pub fn on_update(self: &Arc<Self>, sink: UpdateSink) {
        if self.on_update.set(sink).is_err() {
            return;
        }
        tokio::spawn(receive_updates(Arc::downgrade(self)));
    }

    /// Hand `event`, kept in `n`, to the runtime if it is an update under an
    /// extension this device gets there.
    fn offer_update(&self, n: NeighborhoodId, event: Event) {
        let Some(extension) = extension_for(event.kind.as_u16()) else {
            return;
        };
        let Some(sink) = self.on_update.get() else {
            return;
        };
        if self.record(n).is_some_and(|r| r.policy.gets(extension)) {
            sink(n, extension, event);
        }
    }

    /// What the standing subscription in each neighborhood asks for: the
    /// kinds of every update extension this device gets there. Neighborhoods
    /// where it gets none are left out.
    fn update_filters(&self) -> BTreeMap<NeighborhoodId, Vec<Filter>> {
        if self.links.is_none() {
            return BTreeMap::new();
        }
        self.records()
            .into_iter()
            .filter_map(|r| {
                let kinds: Vec<nostr::Kind> = [NAPPLET_UPDATES, NSITE_UPDATES, MYCO_RELEASE]
                    .into_iter()
                    .filter(|ext| r.policy.gets(ext))
                    .flat_map(kinds_for)
                    .collect();
                (!kinds.is_empty()).then(|| (r.id, vec![Filter::new().kinds(kinds)]))
            })
            .collect()
    }
}

/// One neighborhood's standing update subscription.
struct Standing {
    filters: Vec<Filter>,
    _follow: AbortOnDrop,
    _again: AbortOnDrop,
}

/// Keep the standing subscriptions matched to the neighborhoods, and hand
/// every update that arrives to the sink: what lands in a scope from a peer,
/// whichever way it came, and what a peer in reach sends again.
async fn receive_updates(neighbors: Weak<Neighbors>) {
    let (mut kept, mut changes) = {
        let Some(this) = neighbors.upgrade() else {
            return;
        };
        (this.live_events(), this.links.as_ref().map(|l| l.watch()))
    };
    let (again_tx, mut again) = mpsc::unbounded_channel::<(NeighborhoodId, Event)>();
    let mut standing: BTreeMap<NeighborhoodId, Standing> = BTreeMap::new();
    let mut recheck = tokio::time::interval(RECHECK_EVERY);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick is at once.
    let mut stale = false;
    loop {
        if stale {
            stale = false;
            let Some(this) = neighbors.upgrade() else {
                return;
            };
            let wanted = this.update_filters();
            standing.retain(|n, s| wanted.get(n) == Some(&s.filters));
            for (n, filters) in wanted {
                if standing.contains_key(&n) {
                    continue;
                }
                let Some((follow, mut rx)) = this.follow_links(n, filters.clone()) else {
                    continue;
                };
                let tx = again_tx.clone();
                let forward = tokio::spawn(async move {
                    while let Some(event) = rx.recv().await {
                        if tx.send((n, event)).is_err() {
                            return;
                        }
                    }
                });
                standing.insert(
                    n,
                    Standing {
                        filters,
                        _follow: follow,
                        _again: AbortOnDrop(forward),
                    },
                );
            }
        }
        let changed = async {
            match changes.as_mut() {
                Some(c) => c.changed().await.is_ok(),
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            got = kept.recv() => match got {
                // From a peer, the first time it is kept here: this device's
                // own shares and publishes are not offered back to it.
                Ok(k) if !k.provenance.mine && !k.again => {
                    let Some(this) = neighbors.upgrade() else { return };
                    this.offer_update(k.n, k.event);
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::debug!(missed, "update hand-off fell behind");
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            Some((n, event)) = again.recv() => {
                let Some(this) = neighbors.upgrade() else { return };
                this.offer_update(n, event);
            }
            open = changed => {
                if !open {
                    changes = None;
                }
                stale = true;
            }
            _ = recheck.tick() => stale = true,
        }
    }
}
