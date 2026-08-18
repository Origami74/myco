package app.myco.ble

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import app.myco.R
import app.myco.core.MycoCore
import app.myco.core.NativeActions
import app.myco.core.NativeCore

/**
 * Foreground service that keeps BLE peering alive while the app is backgrounded.
 * It owns the [BleRadio] and starts the embedded node's BLE transport:
 *
 * 1. create the radio, inject the byte-bridge into the core ([NativeCore.bleBridgeNew]),
 * 2. dispatch StartNode — the node's BLE transport then drives the radio
 *    (listen/advertise/scan) over the bridge.
 *
 * The node + radio are process-singletons ([MycoCore]); the developer UI reads
 * the same node's state.
 *
 * It also owns the **Bluetooth adapter watch**. The radio can only be built
 * against an adapter that is ON, so the service follows the adapter for the
 * radio's whole life: parking the lane when Bluetooth goes away and rebuilding
 * it when Bluetooth comes back. See [applyAdapterState].
 */
class BleService : Service() {
    private var radio: BleRadio? = null
    private var bridgeHandle: Long = 0

    /** True once [startBle] has run and not been undone, so [onDestroy] knows
     *  whether there is anything left to tear down — including in the parked
     *  case, where the radio is already gone but the notification is not. */
    private var running = false

    /**
     * The lane is down because **Bluetooth is off**, as opposed to down because
     * nobody asked for it. Only a parked lane is rebuilt on `STATE_ON`, so a
     * duplicate or spurious settle cannot bounce a healthy radio and drop its
     * peers.
     */
    private var parked = false

    /** Consecutive rebuilds that came up without an L2CAP listener. */
    private var rebuilds = 0

    private var adapterWatch: BroadcastReceiver? = null
    private val handler = Handler(Looper.getMainLooper())

    // App-visibility observer: while no activity is visible (home button, screen
    // off), drop BLE discovery from LOW_LATENCY to a duty-cycled LOW_POWER scan —
    // the dominant background battery cost. ProcessLifecycleOwner's onStop fires
    // for both cases; onStart restores full intensity. Established connections
    // are untouched either way.
    private val visibilityObserver = object : DefaultLifecycleObserver {
        override fun onStart(owner: LifecycleOwner) {
            radio?.setBackgroundMode(false)
        }

        override fun onStop(owner: LifecycleOwner) {
            radio?.setBackgroundMode(true)
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        // Registered here, exactly once per service instance, and for the whole
        // of it: the watch has to outlive the radio, because the case it exists
        // for is precisely the one where there is no working radio to hang it
        // off. Unregistered in [onDestroy].
        registerAdapterWatch()
    }

    override fun onDestroy() {
        handler.removeCallbacks(settleAdapterState)
        handler.removeCallbacks(checkRebuild)
        adapterWatch?.let { runCatching { unregisterReceiver(it) } }
        adapterWatch = null
        if (running) stopBle()
        super.onDestroy()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopBle()
                stopSelf()
                return START_NOT_STICKY
            }
            else -> startBle()
        }
        return START_STICKY
    }

    private fun startBle() {
        if (radio != null) return
        startForegroundCompat()

        val client = MycoCore.client(this)
        val r = BleRadio(this)
        val handle = NativeCore.bleBridgeNew(client.handle(), r)
        if (handle == 0L) {
            Log.e(TAG, "bleBridgeNew failed")
            stopSelf()
            return
        }
        r.bindBridge(handle)
        radio = r
        bridgeHandle = handle

        // Inject-then-start: the node's BLE transport picks up the bridge and
        // begins driving the radio (listen → advertise → scan).
        client.dispatch(NativeActions.setBleEnabled(true))
        // Node lifecycle follows the mesh "Enable" master switch, not this
        // toggle — and a running node is never bounced to adopt this radio.
        // The core resolves the injected bridge per operation, so the fresh one
        // above is picked up in place. Restarting to rebind it (which is what
        // this did) tore down every peer and session, so turning Bluetooth on
        // knocked the whole mesh out for as long as re-handshaking took.
        val meshOn = getSharedPreferences("myco_prefs", MODE_PRIVATE)
            .getBoolean(app.myco.MainActivity.PREF_MESH, true)
        if (meshOn && !client.state().nodeRunning) {
            client.dispatch(NativeActions.startNode())
        }
        // Registration replays the current state (onStart fires immediately if the
        // app is already visible), so the radio starts in the right mode.
        ProcessLifecycleOwner.get().lifecycle.addObserver(visibilityObserver)
        running = true
        // A radio built against an adapter that is off has no listener, no
        // advert and no scan — the core's `listen` returns 0 and both
        // `bluetoothLeScanner` and `bluetoothLeAdvertiser` are null. Record
        // that so `STATE_ON` knows there is a lane to recover, which is the
        // cold-start half of the bug: with Bluetooth already off at launch,
        // nothing later asked the radio to try again.
        parked = !adapterOn()
        if (parked) {
            Log.w(TAG, "BLE service started with Bluetooth off — lane parked, waiting for the adapter")
            showNotification(TEXT_WAITING)
        } else {
            Log.i(TAG, "BLE service started")
        }
    }

    /** Stop the lane **and** the service's foreground status: the radio is not
     *  coming back without another [startBle]. */
    private fun stopBle() {
        teardown(keepAlive = false)
        running = false
        stopForeground(STOP_FOREGROUND_REMOVE)
        Log.i(TAG, "BLE service stopped")
    }

    /**
     * Drop the radio and retract it from the core.
     *
     * [keepAlive] keeps the service in the foreground — used when parking for a
     * Bluetooth outage, where the service must survive to see the adapter come
     * back. Dropping foreground status there would leave the recovery watch on
     * a plain background service, which the OS is free to kill; the outage would
     * then be permanent again, just for a different reason.
     */
    private fun teardown(keepAlive: Boolean) {
        ProcessLifecycleOwner.get().lifecycle.removeObserver(visibilityObserver)
        // Node lifecycle belongs to the mesh "Enable" master switch — never stop
        // the node here. Clearing the BLE flag tells the node's BLE backend to
        // stop driving the radio; shutting the radio down closes its channels,
        // which the core observes as channel closures.
        runCatching {
            val client = MycoCore.client(this)
            client.dispatch(NativeActions.setBleEnabled(false))
        }
        radio?.shutdown()
        radio = null
        // Retract the dead radio from the core's slot. Without this the core kept
        // holding it, and the next node rebuild (a mesh toggle) installed the
        // shut-down radio into the fresh slot — so `listen` and `start_advertising`
        // ran against closed sockets until `bleBridgeNew` replaced it a second
        // later. An empty slot parks the backend instead, which is correct.
        if (bridgeHandle != 0L) runCatching { NativeCore.bleBridgeClear(bridgeHandle) }
        // Intentionally NOT freeing the bridge handle: the radio's I/O threads may
        // still reference it as they wind down, and the rebuilt node will inject a
        // fresh bridge on the next start. Freeing here risks a use-after-free; the
        // bridge is small, so leaking it on stop is the safe trade.
        bridgeHandle = 0
        if (keepAlive) Log.i(TAG, "BLE radio torn down (service stays up)")
    }

    // ---- Bluetooth adapter watch ----

    /**
     * Follow `BluetoothAdapter.ACTION_STATE_CHANGED` for the life of the
     * service.
     *
     * Without this, turning Bluetooth off and on again left the process
     * permanently deaf: scanning is only ever started by the core's
     * `RadioIntent` when a radio is installed, and nothing re-installed one.
     * The adapter is the only thing that knows it came back.
     */
    private fun registerAdapterWatch() {
        if (adapterWatch != null) return
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context?, intent: Intent?) {
                if (intent?.action != BluetoothAdapter.ACTION_STATE_CHANGED) return
                val state = intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, BluetoothAdapter.ERROR)
                // STATE_TURNING_ON / STATE_TURNING_OFF broadcast too, and acting
                // on them is worse than useless: mid-flight the stack still
                // rejects `listenUsingInsecureL2capChannel` and still hands out
                // a null scanner, so a rebuild there fails and leaves the lane
                // looking rebuilt. Settled states only.
                if (state != BluetoothAdapter.STATE_ON && state != BluetoothAdapter.STATE_OFF) return
                Log.i(TAG, "bluetooth adapter ${if (state == BluetoothAdapter.STATE_ON) "ON" else "OFF"}")
                // Coalesce. Each settled state cancels the pending apply and
                // re-arms it, so an off/on/off flurry converges on **one**
                // rebuild-or-park decided against whatever the adapter finally
                // settled on — never a stack of bounces racing each other for
                // the core's radio slot. The same delay doubles as the stack's
                // grace period after STATE_ON, before which an L2CAP listener
                // often will not bind.
                handler.removeCallbacks(settleAdapterState)
                handler.postDelayed(settleAdapterState, ADAPTER_SETTLE_MS)
            }
        }
        adapterWatch = receiver
        registerReceiver(receiver, IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED))
    }

    private val settleAdapterState = Runnable { applyAdapterState() }

    /**
     * Bring the lane into line with the adapter.
     *
     * Reads the adapter directly rather than the broadcast's `EXTRA_STATE`:
     * after coalescing, the only state that matters is the one it settled on.
     *
     * Recovery is a **full rebuild of the radio**, not a re-issued scan. When
     * Bluetooth was off, the L2CAP listener never opened either, so the radio
     * has no PSM — re-issuing only the scan would leave a node that hears
     * everyone and can be dialled by no one, advertising a PSM nothing listens
     * on (the failure fixed in 86ce34d). A rebuild goes through the same path
     * the mesh toggle uses: `bleBridgeClear` empties the core's radio slot,
     * `bleBridgeNew` installs a fresh one, and the core's `RadioIntent` then
     * re-applies listen → advertise → scan against it *in that order* — so the
     * PSM it advertises is by construction the one `listen()` just bound.
     */
    private fun applyAdapterState() {
        if (!running) return
        if (adapterOn()) {
            if (!parked) {
                Log.i(TAG, "bluetooth on and the lane was never parked — nothing to recover")
                return
            }
            rebuilds = 0
            rebuildLane("bluetooth came back")
        } else {
            handler.removeCallbacks(checkRebuild)
            if (parked && radio == null) return // already parked; nothing to do
            parked = true
            Log.w(TAG, "bluetooth off — parking the BLE lane (scanning + advertising now report down)")
            // Tears the radio down, which pushes scanning=false and
            // advertising=false into the core's atomics — so the Dev tab stops
            // claiming a radio that no longer exists is active.
            teardown(keepAlive = true)
            showNotification(TEXT_WAITING)
        }
    }

    /** Rebuild the radio from scratch and check, once, that it actually came up. */
    private fun rebuildLane(why: String) {
        rebuilds++
        Log.i(TAG, "$why — rebuilding the BLE lane (attempt $rebuilds)")
        if (radio != null) teardown(keepAlive = true)
        // startBle re-reads the adapter and sets `parked` (and the notification)
        // itself, so a rebuild that races Bluetooth going away again lands back
        // in the parked state rather than claiming success.
        startBle()
        handler.removeCallbacks(checkRebuild)
        handler.postDelayed(checkRebuild, REBUILD_CHECK_MS)
    }

    /**
     * Did the rebuild produce a dialable radio?
     *
     * A rebuild issued a moment too early gets a scanner but no listener — the
     * adapter is ON while the stack is still settling — and that is exactly the
     * half-recovered state this whole change exists to avoid, so it is checked
     * rather than assumed. [BleRadio.listenPsm] is non-zero only while a real
     * L2CAP server socket is bound.
     */
    private val checkRebuild = Runnable {
        val psm = radio?.listenPsm ?: 0
        when {
            !adapterOn() -> Unit // an OFF overtook us; applyAdapterState owns it
            psm != 0 -> {
                Log.i(TAG, "BLE lane recovered: L2CAP listener bound to PSM $psm (advertised by the core)")
                rebuilds = 0
            }
            rebuilds >= MAX_REBUILDS ->
                Log.e(TAG, "BLE lane still has no L2CAP listener after $rebuilds rebuilds — giving up until the adapter changes again")
            else -> rebuildLane("no L2CAP listener after rebuild $rebuilds")
        }
    }

    private fun adapterOn(): Boolean =
        (getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)?.adapter?.isEnabled == true

    private fun startForegroundCompat() {
        val notif = buildNotification(TEXT_ACTIVE)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIF_ID, notif, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        } else {
            startForeground(NOTIF_ID, notif)
        }
    }

    /** Re-post the ongoing notification so it says what the lane is actually
     *  doing — the service stays foreground either way. */
    private fun showNotification(text: String) {
        if (!running) return
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        runCatching { nm.notify(NOTIF_ID, buildNotification(text)) }
    }

    private fun buildNotification(text: String): Notification {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(CHANNEL_ID, "Myco BLE", NotificationManager.IMPORTANCE_LOW)
            channel.description = "Bluetooth peering"
            nm.createNotificationChannel(channel)
        }
        val stop = Intent(this, BleService::class.java).setAction(ACTION_STOP)
        val stopPi = android.app.PendingIntent.getService(
            this, 0, stop,
            android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Myco")
            .setContentText(text)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setOngoing(true)
            .addAction(0, "Stop", stopPi)
            .build()
    }

    companion object {
        private const val TAG = "MycoBleService"
        private const val NOTIF_ID = 1
        private const val CHANNEL_ID = "myco_ble"
        private const val TEXT_ACTIVE = "BLE peering active"
        private const val TEXT_WAITING = "Waiting for Bluetooth"

        /** How long a settled adapter state is left to stand before it is acted
         *  on. Two jobs at once: it collapses an off/on/off flurry into one
         *  decision, and it gives the Bluetooth stack a moment after `STATE_ON`
         *  — the adapter reports enabled before an L2CAP listener will reliably
         *  bind. [checkRebuild] covers the case where it is still too soon. */
        private const val ADAPTER_SETTLE_MS = 1_500L

        /** How long after a rebuild to confirm the listener actually bound. */
        private const val REBUILD_CHECK_MS = 5_000L

        /** Rebuild attempts per adapter change before the lane is left alone.
         *  Bounded so a chipset that will not open a listener at all cannot
         *  turn recovery into an endless radio-churn loop. */
        private const val MAX_REBUILDS = 3
        const val ACTION_START = "app.myco.ble.START"
        const val ACTION_STOP = "app.myco.ble.STOP"

        fun start(context: Context) {
            val i = Intent(context, BleService::class.java).setAction(ACTION_START)
            context.startForegroundService(i)
        }

        fun stop(context: Context) {
            val i = Intent(context, BleService::class.java).setAction(ACTION_STOP)
            context.startService(i)
        }
    }
}
