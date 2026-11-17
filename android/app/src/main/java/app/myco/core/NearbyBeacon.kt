package app.myco.core

import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import app.myco.ble.BleRadio

/**
 * Carries the nearby secret between the core and the BLE radio.
 *
 * - **Out:** the secret the core wants advertised (`neighborhoods.nearby.secret`)
 *   is read every [EVERY_MS] and handed to [BleRadio.nearbySecret], which
 *   re-advertises when it changes. A secret is used once, so a join is
 *   followed by a new advert within that interval.
 * - **In:** a secret heard in another phone's advert goes to the core as
 *   `nearby_heard`, which offers it as a proof when it sets up nearby links.
 *
 * Runs for the life of the process, on its own thread: both directions cross
 * JNI.
 */
object NearbyBeacon {
    private const val TAG = "myco-nearby-beacon"
    private const val EVERY_MS = 2_000L

    @Volatile
    private var started = false

    fun start(client: AppCoreClient) {
        synchronized(this) {
            if (started) return
            started = true
        }
        val thread = HandlerThread("myco-nearby-beacon").apply { start() }
        val handler = Handler(thread.looper)
        BleRadio.onNearbySecret = { secret ->
            handler.post {
                runCatching { client.dispatch(NativeActions.nearbyHeard(secret)) }
                    .onFailure { Log.w(TAG, "could not pass on a heard secret", it) }
            }
        }
        val poll = object : Runnable {
            override fun run() {
                runCatching {
                    val secret = client.state().neighborhoods.nearby.secret
                    BleRadio.nearbySecret = secret.ifEmpty { null }
                }.onFailure { Log.w(TAG, "could not read the nearby secret", it) }
                handler.postDelayed(this, EVERY_MS)
            }
        }
        handler.post(poll)
    }
}
