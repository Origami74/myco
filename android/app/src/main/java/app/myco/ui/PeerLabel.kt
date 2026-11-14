package app.myco.ui

import app.myco.core.AppState
import app.myco.share.DeviceName

/**
 * The name a peer has actually told us, or null when they have told us nothing.
 *
 * A device's chosen name reaches us only through a neighborhood invitation:
 * the app peer entry carries the name they gave when the link was made.
 *
 * Below that sits the name an older build broadcasts for itself in its BLE scan
 * response. It is deliberately last: it is an unauthenticated plaintext
 * broadcast that anyone in range can forge, so it may fill a gap but must never
 * displace a name that arrived signed. It is also BLE-only — a peer found over
 * Wi-Fi Aware or the LAN carries none.
 *
 * Null means "we do not know", which is not the same as the npub-derived
 * placeholder [peerLabel] falls back to. Anything we *store* about a peer must
 * use this, never the placeholder: a stored placeholder outranks the real name
 * when it finally arrives.
 */
fun peerNameOrNull(state: AppState, npub: String): String? {
    if (npub.isEmpty()) return null
    val told = state.appPeers.firstOrNull { it.npub == npub }?.name
    told?.trim()?.ifBlank { null }?.let { return it }
    return state.peers.firstOrNull { it.npub == npub }?.advertisedName?.trim()?.ifBlank { null }
}

/**
 * What to call a peer on screen — the name *they* chose whenever we have
 * actually been told it (see [peerNameOrNull]), and otherwise the npub-derived
 * name, which is the floor rather than the default. It is at least the same two
 * words on both screens.
 */
fun peerLabel(state: AppState, npub: String): String =
    peerNameOrNull(state, npub) ?: DeviceName.generated(npub)
