package app.myco.ui

import app.myco.core.AppState
import app.myco.share.DeviceName

/**
 * What to call a peer on screen — the name *they* chose, whenever we have
 * actually been told it.
 *
 * A device's chosen name reaches us only inside pair traffic: the Circle entry
 * written when pairing completed, the pending request they sent us, or the
 * invite we sent them (which echoes the name they were showing at the time).
 * Those are checked in that order — most recently confirmed first — and the
 * npub-derived name is the floor, not the default.
 *
 * Below those sits the name a peer broadcasts for itself in its BLE scan
 * response. It is deliberately last-but-one: it is an unauthenticated plaintext
 * broadcast that anyone in range can forge, so it may fill a gap but must never
 * displace a name that arrived signed. It is also BLE-only — a peer found over
 * Wi-Fi Aware or the LAN carries none.
 *
 * The npub-derived name remains the floor, for a peer that has told us nothing
 * at all. It is at least the same two words on both screens.
 */
fun peerLabel(state: AppState, npub: String): String {
    if (npub.isEmpty()) return DeviceName.generated(npub)
    val told = state.circle.firstOrNull { it.npub == npub }?.name
        ?: state.pendingPairRequests.firstOrNull { it.npub == npub }?.name
        ?: state.outboundPairs.firstOrNull { it.npub == npub }?.name
    told?.trim()?.ifBlank { null }?.let { return it }
    val advertised = state.peers.firstOrNull { it.npub == npub }?.advertisedName
    return advertised?.trim()?.ifBlank { null } ?: DeviceName.generated(npub)
}
