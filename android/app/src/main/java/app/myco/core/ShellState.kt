package app.myco.core

/**
 * The part of an [AppState] the shell recomposes on.
 *
 * While anything is linked, a handful of diagnostic numbers move on almost
 * every poll: when a peer was last heard, link and path RTTs, send drops, the
 * attempt log, RSSI, public-node ping and the storage counters. Handing the raw
 * snapshot to every screen made each poll a "new" state, so every visible
 * screen and grid tile recomposed once a second even with nothing on it
 * changed. On a slow TV with an accessibility service running, that was a
 * multi-second stall.
 *
 * This projection blanks those numbers to fixed values, and puts [AppState.peers]
 * in key order (the core orders them by last-heard, which reshuffles as peers
 * speak). Two polls that differ only in diagnostics then project to equal
 * states, and the shell keeps the one it has.
 *
 * Screens that **show** these numbers must not read them from here: they read
 * the full latest snapshot through `LocalLiveState` instead. Those are the Dev
 * tab, the mesh status sheet, Settings › Internet, Settings › Storage and the
 * Storage row on the Settings root.
 *
 * Everything else is kept as is. Sync progress ([SiteStatus.filesPulled]) stays:
 * the Apps grid draws it, and it only moves while a site is downloading.
 */
fun AppState.forShell(): AppState = copy(
    blePeers = blePeers.map { if (it.rssi == null) it else it.copy(rssi = null) },
    // Only the Dev tab lists adverts; they arrive in scan order with live RSSI.
    bleAdverts = emptyList(),
    peers = peers.map { it.forShell() }.sortedBy { it.key },
    cache = cache.forShell(),
    publicNodes = publicNodes.copy(nodes = publicNodes.nodes.map { it.copy(srttMs = null) }),
)

private fun PeerDiagnostic.forShell(): PeerDiagnostic = copy(
    paths = paths.map { it.copy(minRttMs = null, rttSamples = 0, etx = 0.0, score = null) },
    lastSeenMs = 0L,
    authenticatedAtMs = 0L,
    srttMs = null,
    rssi = null,
    discoveryMs = 0L,
    sendDrops = 0L,
    attempts = emptyList(),
)

/** Counts and sizes blanked; the budgets and the custom-store flags kept. */
private fun CacheStatus.forShell(): CacheStatus = copy(
    relayEvents = 0,
    blobCount = 0,
    usedBytes = 0,
    eventCache = eventCache.copy(count = 0, bytes = 0),
    blobCache = blobCache.copy(count = 0, bytes = 0),
)
