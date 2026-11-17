package app.myco.core

import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.Log
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import app.myco.ble.BleRadio

/**
 * Carries the nearby secret between the core and the BLE radio.
 *
 * - **Out:** the secret the core wants advertised (`neighborhoods.nearby.secret`)
 *   is read and handed to [BleRadio.nearbySecret], which re-advertises when it
 *   changes. A secret is used once, so a join is followed by a new advert
 *   within one read interval.
 * - **In:** secrets heard in other phones' adverts go to the core as
 *   `nearby_heard`, batched: one dispatch per [FLUSH_MS] at most, whatever
 *   the radio hears.
 *
 * The state is read (a full JNI state serialize) only while the BLE radio
 * runs: every [EVERY_MS] on screen with Nearby on, every [SLOW_MS] in the
 * background or with Nearby off (empty secret). With no radio nothing
 * crosses JNI. Runs on its own thread: both directions cross JNI.
 */
object NearbyBeacon {
    private const val TAG = "myco-nearby-beacon"
    private const val EVERY_MS = 2_000L
    private const val SLOW_MS = 10_000L
    private const val FLUSH_MS = 1_000L
    /** Most heard secrets held for one flush; the core caps what it takes anyway. */
    private const val MAX_PENDING = 32

    @Volatile
    private var started = false

    @Volatile
    private var foreground = false

    private val pending = LinkedHashSet<String>()
    private var flushQueued = false

    fun start(client: AppCoreClient) {
        synchronized(this) {
            if (started) return
            started = true
        }
        val thread = HandlerThread("myco-nearby-beacon").apply { start() }
        val handler = Handler(thread.looper)

        val flush = Runnable {
            val batch = synchronized(pending) {
                flushQueued = false
                pending.joinToString(",").also { pending.clear() }
            }
            if (batch.isNotEmpty()) {
                runCatching { client.dispatch(NativeActions.nearbyHeard(batch)) }
                    .onFailure { Log.w(TAG, "could not pass on heard secrets", it) }
            }
        }
        BleRadio.onNearbySecret = { secret ->
            val post = synchronized(pending) {
                if (pending.size < MAX_PENDING) pending.add(secret)
                (!flushQueued).also { flushQueued = true }
            }
            if (post) handler.postDelayed(flush, FLUSH_MS)
        }

        val poll = object : Runnable {
            override fun run() {
                var next = EVERY_MS
                // No radio, no advert: nothing to read. A radio built later
                // gets the current secret within one interval; the one it
                // starts with is at worst spent, which a dialer is told.
                if (BleRadio.running) {
                    runCatching {
                        val secret = client.state().neighborhoods.nearby.secret
                        BleRadio.nearbySecret = secret.ifEmpty { null }
                        if (secret.isEmpty() || !foreground) next = SLOW_MS
                    }.onFailure {
                        Log.w(TAG, "could not read the nearby secret", it)
                        next = SLOW_MS
                    }
                }
                handler.postDelayed(this, next)
            }
        }
        handler.post(poll)

        // Lifecycle observers are added on the main thread; registration
        // replays the current state.
        Handler(Looper.getMainLooper()).post {
            ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
                override fun onStart(owner: LifecycleOwner) {
                    foreground = true
                    // Back on screen: read now rather than after a slow wait.
                    handler.removeCallbacks(poll)
                    handler.post(poll)
                }

                override fun onStop(owner: LifecycleOwner) {
                    foreground = false
                }
            })
        }
    }
}
