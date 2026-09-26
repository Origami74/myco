package app.myco.core

import android.util.Log
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.util.concurrent.TimeUnit

/**
 * Automatic update checks for installed apps: when Myco comes to the
 * foreground, and on a slow timer while the process lives
 * (`docs/design/nsite/nsite-updates.md` §3.1).
 *
 * This only *asks*. The core's update gate decides whether a check actually
 * runs — throttled, never two at once — so the triggers here can fire freely.
 * An automatic check is silent; the "Check for updates" button is the one
 * that reports.
 *
 * Nothing here wakes the device: no alarms, no WorkManager. The timer is a
 * coroutine delay, which does not advance while the phone sleeps, so a check
 * only happens while the process is alive and awake anyway.
 */
object UpdateChecks {
    private const val TAG = "myco-updates"

    /**
     * The periodic interval. nsite and napplet updates are rare, deliberate
     * publishes, and a nearby peer often gossips a new manifest before any
     * check would find it; four background checks a day keep a long-running
     * process current without polling public relays from a phone in a pocket.
     */
    private val PERIOD_MS = TimeUnit.HOURS.toMillis(6)

    /**
     * How long after coming to the foreground the check is asked for. Lets the
     * mesh reconnect first — a check at the instant of a cold start finds no
     * Circle peers to ask, and would hold the throttle shut for the next half
     * hour — and skips app switches that leave again straight away.
     */
    private const val FOREGROUND_DELAY_MS = 20_000L

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    @Volatile
    private var started = false

    /** Only touched on the main thread, where lifecycle callbacks arrive. */
    private var pendingForeground: Job? = null

    /** Start the triggers, once per process. */
    fun start(client: AppCoreClient) {
        synchronized(this) {
            if (started) return
            started = true
        }
        // Lifecycle observers must be added on the main thread. Registration
        // replays the current state, so an app already in the foreground gets
        // its onStart straight away.
        scope.launch(Dispatchers.Main) {
            ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
                override fun onStart(owner: LifecycleOwner) {
                    pendingForeground?.cancel()
                    pendingForeground = scope.launch {
                        delay(FOREGROUND_DELAY_MS)
                        request(client, "foreground")
                    }
                }

                override fun onStop(owner: LifecycleOwner) {
                    pendingForeground?.cancel()
                    pendingForeground = null
                }
            })
        }
        scope.launch {
            while (isActive) {
                delay(PERIOD_MS)
                request(client, "periodic")
            }
        }
    }

    /** Off the main thread: the dispatch crosses JNI into the core. */
    private fun request(client: AppCoreClient, why: String) {
        runCatching { client.dispatch(NativeActions.checkNsiteUpdates(auto = true)) }
            .onFailure { Log.w(TAG, "$why update check", it) }
    }
}
