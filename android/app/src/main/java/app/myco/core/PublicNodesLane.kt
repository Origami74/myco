package app.myco.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner

/**
 * The Android half of the public-node internet lane (roadmap N10). The core
 * decides what to dial and when (`myco-core/src/public_nodes.rs`); this does
 * the two things only Android can:
 *
 * - **Tells the core whether Myco is on screen**, from [ProcessLifecycleOwner],
 *   so advert refreshes and redials slow down in a pocket.
 * - **Pins the lane's socket to the default internet network.** The core binds
 *   an outbound-only UDP socket for public nodes (instance `internet`). Myco
 *   is subject to its own VPN, and with the SOCKS exit on that VPN claims all
 *   public IPv4 — an unpinned socket would send the mesh's own traffic into the
 *   tunnel it is supposed to carry. Pinning it to the best validated non-VPN
 *   network keeps it on the real uplink, and follows that uplink from Wi-Fi to
 *   cellular and back.
 *
 * Runs for the life of the process. With the feature off the socket is pinned
 * but carries nothing, which costs nothing; a request for an *existing*
 * network brings no radio up.
 */
object PublicNodesLane {
    private const val TAG = "myco-public-nodes"

    /** Matches `PUBLIC_UDP_INSTANCE` in the core. */
    private const val LANE = "internet"

    @Volatile
    private var started = false

    fun start(context: Context, client: AppCoreClient) {
        synchronized(this) {
            if (started) return
            started = true
        }
        val thread = HandlerThread("myco-public-nodes").apply { start() }
        val handler = Handler(thread.looper)
        val pin = UdpSocketPin(LANE, handler, TAG)
        handler.post { pin.start() }

        val connectivity = context.getSystemService(ConnectivityManager::class.java)
        val request = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            .build()
        // A *request*, not a listen: the system answers with the one network
        // it would use for this, and moves it as the uplink changes.
        runCatching {
            connectivity?.requestNetwork(
                request,
                object : ConnectivityManager.NetworkCallback() {
                    override fun onAvailable(network: Network) {
                        Log.i(TAG, "internet uplink is $network")
                        pin.bindTo(network)
                    }

                    override fun onLost(network: Network) {
                        pin.clearTarget(network)
                    }
                },
                handler,
            )
        }.onFailure { Log.w(TAG, "could not track the internet uplink", it) }

        // Lifecycle observers must be added on the main thread; registration
        // replays the current state. The dispatch itself crosses JNI, so it is
        // posted to the lane's own thread.
        Handler(context.mainLooper).post {
            ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
                override fun onStart(owner: LifecycleOwner) = report(true)
                override fun onStop(owner: LifecycleOwner) = report(false)

                private fun report(foreground: Boolean) {
                    handler.post {
                        runCatching { client.dispatch(NativeActions.setAppForeground(foreground)) }
                            .onFailure { Log.w(TAG, "foreground=$foreground", it) }
                    }
                }
            })
        }
    }
}
