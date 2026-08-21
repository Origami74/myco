package app.myco.aware

/**
 * Which of the Aware lane's UDP sockets carries which peer.
 *
 * # Why slots exist
 *
 * Every Wi-Fi Aware data path is its own `android.net.Network`, and
 * `Network.bindSocket` marks a socket for exactly one of them. With a single
 * socket the most recent NDP wins and every other peer, data path still up,
 * becomes unreachable — which is why the radio used to refuse a second NDP
 * outright. The core therefore binds a *pool* of UDP transport instances
 * (`aware0`…), and this class decides which one a peer gets.
 *
 * # Allocation
 *
 * Lowest free slot, held for as long as the peer's data path is. Re-acquiring
 * for a peer that already holds one returns the same slot, so the identity
 * exchange can ask freely without spending the pool. `null` means full: the
 * caller must not fall back to another peer's slot, because the port it would
 * then advertise names a socket marked for somebody else's data path — the
 * peer's packets would arrive and the replies would leave down the wrong
 * network.
 *
 * Slot 0 is the base port, which is what a peer discovered before its identity
 * is known — and a peer on a build from before the pool existed — is told, so
 * a pair of phones lands on it without any correction.
 *
 * Thread-safe: NDP callbacks arrive on the framework's threads while discovery
 * and retries run on the radio's handler.
 */
internal class AwareSlotPool(
    /** Pool size, from the core (`wifiAwareSlots`) — the number of UDP
     *  transport instances the node actually bound. */
    private val size: Int,
) {
    private val byNpub = HashMap<String, Int>()

    /** This peer's slot, allocating the lowest free one if it has none.
     *  `null` when the pool is full — never another peer's slot. */
    @Synchronized
    fun acquire(npub: String): Int? {
        byNpub[npub]?.let { return it }
        val taken = byNpub.values.toSet()
        val free = (0 until size).firstOrNull { it !in taken } ?: return null
        byNpub[npub] = free
        return free
    }

    /** This peer's slot, or null if it holds none. Never allocates. */
    @Synchronized
    fun slotOf(npub: String): Int? = byNpub[npub]

    /** Give the slot back. Idempotent — teardown paths overlap. */
    @Synchronized
    fun release(npub: String) {
        byNpub.remove(npub)
    }

    /** Drop every allocation (the lane went down). */
    @Synchronized
    fun clear() {
        byNpub.clear()
    }

    /** Slots not currently held. */
    @Synchronized
    fun free(): Int = size - byNpub.size

    /** Slots currently held — for logging alongside the framework's own count. */
    @Synchronized
    fun inUse(): Int = byNpub.size
}
