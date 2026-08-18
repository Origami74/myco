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
 * The floor still matters. A peer we are merely connected to and have never
 * exchanged pair traffic with has told us nothing but an npub: the BLE advert
 * is 27 of its 31 bytes already, and fips's own `display_name` is a local
 * alias, not something the far side sends. For those the generated name is all
 * there is, and it is at least the same two words on both screens.
 */
fun peerLabel(state: AppState, npub: String): String {
    if (npub.isEmpty()) return DeviceName.generated(npub)
    val told = state.circle.firstOrNull { it.npub == npub }?.name
        ?: state.pendingPairRequests.firstOrNull { it.npub == npub }?.name
        ?: state.outboundPairs.firstOrNull { it.npub == npub }?.name
    return told?.trim()?.ifBlank { null } ?: DeviceName.generated(npub)
}
