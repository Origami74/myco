package app.myco.core

import android.net.Network
import android.os.Handler
import android.os.ParcelFileDescriptor
import android.system.Os
import android.util.Log
import java.net.SocketAddress

/**
 * Pins one lane's UDP transport socket to one [Network].
 *
 * # Why a lane needs this at all
 *
 * Android routes by the network a socket is *marked* with, not by destination
 * address alone. A local-only network — a Wi-Fi Aware NDP, the `!FIPS` AP —
 * never passes internet validation, so an unmarked socket's replies can be
 * delivered over, or dropped in favour of, a competing validated default
 * network (typically cellular): the send succeeds locally and nothing ever
 * comes back. `Network.bindSocket` fixes that by marking the socket.
 *
 * # Why there is one of these per lane
 *
 * The mark is exclusive. A socket marked with the Wi-Fi netid cannot reach a
 * Wi-Fi Aware peer, whose NDP is a separate [Network] with its own routing
 * table, and vice versa. While both lanes shared a single core socket, whichever
 * bound last silently disabled the other — which is why Wi-Fi Aware could
 * discover peers, bring up data paths, and never complete a single handshake.
 *
 * The core therefore binds one UDP transport per lane and labels every
 * descriptor it announces with the lane it belongs to. This class asks for one
 * lane by name ([NativeCore.nextUdpTransportFd]), so it cannot be handed the
 * other's socket even if that one is announced first — the announcements are
 * unordered, since the core builds its transports from a hash map.
 *
 * # Lifecycle
 *
 * The fd is a *borrow*: the core owns the socket. Only the number is kept;
 * each pin dups it ([ParcelFileDescriptor.fromFd]), marks the dup — the mark is
 * on the socket, so it holds for the core's descriptor too — and closes the
 * dup at once. Holding the dup instead kept a stopped node's socket open and
 * bound: the next node could not bind its ports and came up with no UDP
 * transports at all, while nothing ever announced a replacement that would
 * have released the dup.
 *
 * The core withdraws the number (announces `-1`) before it closes the socket.
 * Withdrawal is asynchronous, so a pin already queued can still reach a number
 * the kernel has since reused; each pin therefore checks the descriptor still
 * has the local address it had when announced (every lane has its own port)
 * and leaves anything else alone.
 *
 * A poll loop rather than a one-shot read, because a node restart (a mesh
 * off→on cycle) replaces the socket. The core retains the latest announcement
 * per lane and versions it, so a radio the user toggles on *after* the node
 * started still learns the socket, and a replacement is recognised even when
 * the kernel hands back the same fd number.
 *
 * All mutable state is confined to [handler]'s thread.
 */
internal class UdpSocketPin(
    /** Lane name, as understood by [NativeCore.nextUdpTransportFd]. */
    private val lane: String,
    /** The owning radio's handler; every state change runs here. */
    private val handler: Handler,
    /** The owning radio's log tag, so pin messages sit with its own. */
    private val tag: String,
) {
    /** The core's descriptor number for this lane's socket, or -1 for none. */
    private var fd = -1

    /** That socket's local address when announced, to recognise a reused number. */
    private var localAddr: SocketAddress? = null

    /** The network to pin to, remembered so a later fd (or a later network) can
     *  be married up with whichever half arrived first. */
    private var target: Network? = null

    @Volatile
    private var running = false

    /** Start watching for this lane's socket. Idempotent. */
    fun start() {
        if (running) return
        running = true
        Thread({
            var version = 0L
            while (running) {
                val packed = NativeCore.nextUdpTransportFd(lane, version, POLL_TIMEOUT_MS)
                val next = packed ushr 32
                if (next == version) continue // nothing newer within the timeout
                version = next
                // -1 under a new version: the node stopped and withdrew it.
                val fd = packed.toInt()
                handler.post { onFd(fd) }
            }
        }, "myco-udpfd-$lane").apply { isDaemon = true; start() }
    }

    /** Stop watching. The core's descriptor is untouched. Must run on
     *  [handler]'s thread. */
    fun stop() {
        running = false
        fd = -1
        localAddr = null
        target = null
    }

    /**
     * Pin to `network` — now if the socket is known, otherwise as soon as it is
     * announced. Passing the same network again re-binds, which is what a
     * fresh socket for an unchanged network needs. Must run on [handler]'s
     * thread.
     */
    fun bindTo(network: Network) {
        target = network
        bind()
    }

    /** Forget the pin target without unbinding — there is no "unbind", and the
     *  mark is harmless once the network is gone. Must run on [handler]'s
     *  thread. */
    fun clearTarget(network: Network) {
        if (target == network) target = null
    }

    private fun onFd(raw: Int) {
        if (!running) return
        fd = raw
        localAddr = null
        if (raw < 0) {
            Log.i(tag, "$lane UDP transport withdrawn")
            return
        }
        localAddr = runCatching {
            ParcelFileDescriptor.fromFd(raw).use { Os.getsockname(it.fileDescriptor) }
        }.getOrElse {
            Log.w(tag, "could not read $lane UDP transport fd $raw", it)
            fd = -1
            return
        }
        Log.i(tag, "learned $lane UDP transport fd $raw ($localAddr)")
        bind()
    }

    private fun bind() {
        if (fd < 0) return             // no live socket; onFd binds when one comes
        val network = target ?: return // no network to pin to yet
        runCatching {
            ParcelFileDescriptor.fromFd(fd).use { dup ->
                if (Os.getsockname(dup.fileDescriptor) != localAddr) {
                    Log.w(tag, "$lane UDP fd $fd is no longer our socket; not pinning")
                    return
                }
                network.bindSocket(dup.fileDescriptor)
            }
        }
            .onSuccess { Log.i(tag, "pinned $lane UDP socket to $network") }
            .onFailure { Log.w(tag, "pinning $lane UDP socket to $network failed", it) }
    }

    private companion object {
        /** Blocking wait per poll. Short so a mesh off→on cycle's fresh socket
         *  is picked up promptly and so [stop] is honoured within a second. */
        const val POLL_TIMEOUT_MS = 1_000
    }
}
