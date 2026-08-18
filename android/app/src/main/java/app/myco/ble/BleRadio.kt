package app.myco.ble

import android.annotation.SuppressLint
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothServerSocket
import android.bluetooth.BluetoothSocket
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.os.ParcelUuid
import android.util.Log
import app.myco.core.NativeCore
import java.io.IOException
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/**
 * The Android BLE radio. Kotlin owns the radio; the Rust core (fips `AndroidIo`)
 * drives it through the byte-bridge:
 *
 * - The Rust core calls the control methods here (listen/connect/advertise/scan/
 *   close) via JNI — see [app.myco.core.NativeCore.bleBridgeNew].
 * - This class pushes inbound bytes/events to the core and pulls outbound bytes
 *   via the `NativeCore.ble*` exports.
 *
 * L2CAP CoC over Android's [BluetoothSocket] is a byte **stream** with no relation
 * to FIPS packet boundaries. This radio is a transparent byte pipe: it forwards the
 * exact socket bytes to/from the core, in order and losslessly. The core recovers
 * packet boundaries itself from the 4-byte FMP length-prefixed header (the same
 * framer TCP uses) — see fips `BleStreamRead` / `read_fmp_packet`.
 *
 * The contract: the ordered concatenation of every chunk passed to `deliver_recv`
 * must exactly equal the byte stream the peer sent. Chunking is free; losslessness
 * and order are mandatory. A dropped inbound chunk desyncs the core's reframer for
 * the rest of the connection, so it is **fatal** — see [readerLoop].
 */
@SuppressLint("MissingPermission")
class BleRadio(context: Context) {
    private val adapter: BluetoothAdapter? =
        (context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)?.adapter

    private val appContext = context.applicationContext
    private var bridgeHandle: Long = 0
    private val io = Executors.newCachedThreadPool()
    private val channels = ConcurrentHashMap<Long, BluetoothSocket>()
    // Dials parked inside BluetoothSocket.connect(), by device MAC. Android's
    // connect blocks well past the core's probe timeout, so without this the
    // core's re-dials stack up: several sockets per device, each holding an LE
    // connect slot, none of them closed. See [connect].
    private val dialing = ConcurrentHashMap<String, BluetoothSocket>()
    // A parallel GATT per channel, used only to request a low connection interval
    // (L2CAP CoC exposes no priority API). Bumps mesh throughput ~2-4x.
    private val gatts = ConcurrentHashMap<Long, GattPrio>()

    /**
     * Per-channel GATT + connection-priority state. HIGH priority pins the LE
     * connection interval at ~11-15ms, which is great for throughput but costs
     * real battery if held for the life of the link — so it is granted on
     * connect (the noise handshake is many small round-trips) and on bulk
     * traffic, then demoted to BALANCED after [IDLE_DEMOTE_MS] without a
     * bulk-sized packet. Heartbeats/pings ride the BALANCED interval fine.
     */
    private class GattPrio {
        @Volatile var gatt: BluetoothGatt? = null // set right after connectGatt returns
        @Volatile var connected = false
        @Volatile var high = false
        @Volatile var lastBulkMs = 0L
        @Volatile var demotePending = false
    }

    @Volatile
    private var stopped = false

    private var serverSocket: BluetoothServerSocket? = null

    /**
     * The PSM this radio's L2CAP listener is actually bound to, or 0 if it has
     * none. Written only by [listen] and cleared by [shutdown], so it can never
     * name a socket that is gone.
     *
     * Read by [BleService] to tell a recovered lane from one that came back up
     * scanning but undialable — a radio with no listener advertises nothing a
     * peer can connect to, which is invisible from the outside.
     */
    @Volatile
    var listenPsm: Int = 0
        private set

    // Say it once, not once per operation and not once per retry: with the
    // adapter off, `bluetoothLeScanner` / `bluetoothLeAdvertiser` are null and
    // `listenUsingInsecureL2capChannel` throws — three separate ways to fail,
    // all meaning the same single thing. Per-radio, so the replacement radio
    // built when Bluetooth returns starts with a clean slate.
    private val adapterOffLogged = AtomicBoolean(false)
    private var advertiser: BluetoothLeAdvertiser? = null
    private var advertiseCallback: AdvertiseCallback? = null
    private var scanner: BluetoothLeScanner? = null
    private var scanCallback: ScanCallback? = null

    // Advertiser-retry state: when the OS refuses our advert because every BLE
    // advertising slot is taken (TOO_MANY_ADVERTISERS, typically Play Services'
    // Nearby Share/Fast Pair), we keep retrying on a backoff until a slot frees.
    private val retryExec = Executors.newSingleThreadScheduledExecutor()
    @Volatile private var advertisePsm = 0


    /** Last advertised name seen per address, so the log fires on change only. */
    private val lastAdvertName = java.util.concurrent.ConcurrentHashMap<String, String>()

    /** Re-issue the advert so a changed [localName] reaches the scan response,
     *  which is fixed at start time. No-op unless we are actually advertising. */
    private fun reAdvertiseForName() {
        if (!stopped && advertiseCallback != null) startAdvertising(advertisePsm)
    }
    private var advertiseRetries = 0
    // Scanner-retry state: a failed scan (Android throttles ~5 startScan/30s →
    // SCAN_FAILED_SCANNING_TOO_FREQUENTLY) used to be logged and abandoned, which
    // permanently killed peer discovery until the mesh was toggled. We re-arm it.
    private var scanRetries = 0

    // Scan-report accounting, summarised on a timer instead of logged per
    // result. Per-result logging emitted several lines a second on a busy
    // radio and rotated every other MycoBleRadio line out of the buffer within
    // seconds. The timer fires whether or not anything was seen, because a
    // scanner producing *nothing* is as much of a symptom as one producing
    // adverts with no PSM — a device in that state is inbound-only, and under
    // a log-only-when-there-is-something scheme it would look healthy.
    private val scanWithPsm = AtomicInteger()
    private val scanWithoutPsm = AtomicInteger()
    private val scanPsmAddrs = ConcurrentHashMap.newKeySet<String>()
    private val scanNoPsmAddrs = ConcurrentHashMap.newKeySet<String>()
    @Volatile private var scanSummaryTask: ScheduledFuture<*>? = null

    // Background mode (set from the service via ProcessLifecycleOwner): while the
    // app is not visible, discovery drops from LOW_LATENCY (~100% RX duty) to
    // LOW_POWER (~10% duty, 512ms/5120ms) with batched delivery — the single
    // biggest background battery saving. Established connections are unaffected;
    // only how fast we *find* new peers degrades (seconds, not minutes).
    @Volatile
    private var backgroundMode = false

    /** Flip fore/background discovery intensity; restarts the scan if one is live. */
    fun setBackgroundMode(bg: Boolean) {
        if (backgroundMode == bg) return
        backgroundMode = bg
        Log.i(TAG, "background mode: $bg")
        // Re-arm the scan with the new duty cycle. Mode flips are user-driven
        // (screen off / app switch), far below the ~5 startScan/30s throttle;
        // if we do trip it, scheduleScanRetry re-arms with the 30s floor.
        if (scanCallback != null) startScanning()
    }

    init {
        // There is only ever one radio per process (BleService guards it), and
        // the app needs a handle on it to push the display name in without
        // threading a reference through the service's start intents.
        instance = this
    }

    fun bindBridge(handle: Long) {
        bridgeHandle = handle
    }

    // ---- control methods, invoked by the Rust core via JNI ----

    /** Open an insecure L2CAP listener; return the OS-assigned PSM (0 on failure). */
    fun listen(): Int {
        if (stopped) return 0
        val a = adapter ?: run {
            Log.e(TAG, "listen: no Bluetooth adapter on this device")
            return 0
        }
        if (!a.isEnabled) {
            reportAdapterOff("listen")
            return 0
        }
        return try {
            val ss = a.listenUsingInsecureL2capChannel()
            serverSocket = ss
            val psm = ss.psm
            listenPsm = psm
            io.execute { acceptLoop(ss) }
            Log.i(TAG, "L2CAP listening on PSM $psm")
            psm
        } catch (e: Exception) {
            Log.e(TAG, "listen failed", e)
            0
        }
    }

    /**
     * Report — once per radio — that Bluetooth itself is off, so the lane
     * cannot open.
     *
     * This is the line whose absence hid the off/on bug: `startScanning` used
     * to return on a null `bluetoothLeScanner` without a word, so a process
     * that started with Bluetooth disabled looked identical to a healthy one
     * (`bluetooth_on=1`, transport `up`) while having never scanned once.
     * Recovery is [BleService]'s job — it watches the adapter — so this only
     * has to be audible, not actionable.
     */
    private fun reportAdapterOff(op: String) {
        if (adapterOffLogged.compareAndSet(false, true)) {
            Log.w(
                TAG,
                "$op: Bluetooth is off — this radio has no listener, no advert and " +
                    "no scan; the lane stays down until the adapter comes back " +
                    "(BleService rebuilds it then)",
            )
        }
    }

    /**
     * Dial a peer; deliver the result (and, on success, start the channel).
     *
     * Every exit closes the socket unless the core adopted it. A
     * [BluetoothSocket] whose `connect()` threw is *not* self-closing, and an
     * abandoned one keeps its slot in the BT stack's LE connect table: leak
     * enough of them and every later dial hangs for its full timeout and then
     * fails, with nothing in the app's own state to explain it. Only killing
     * the process gets them back.
     *
     * The dial is also bounded twice over, because `connect()` blocks for far
     * longer than the core is willing to wait — the core gives up at its
     * probe timeout and re-dials while this thread is still parked inside
     * Android:
     *
     * - one in-flight dial per device ([dialing]): a fresh dial to the same
     *   MAC closes the previous socket, which unblocks it promptly;
     * - [DIAL_WATCHDOG_MS] as a backstop, for the peer the core discovers
     *   once and never probes again.
     *
     * So the socket is closed on every exit that is not an adoption — a
     * `connect()` that threw, a success the core declined to take, the
     * watchdog, and [shutdown] — and the core is answered on every exit
     * including the two that never reach a socket at all (already stopped,
     * and a thread pool that refuses the work).
     */
    fun connect(connectId: Long, addr: String, psm: Int) {
        // Every return from here answers the core, one way or another. A dial
        // that is silently dropped costs the probe loop its full 10s timeout
        // waiting for an attempt that was never made.
        if (stopped) {
            failDial(connectId, addr)
            return
        }
        val submitted = runCatching { io.execute { dial(connectId, addr, psm) } }
        if (submitted.isFailure) {
            // The pool was shut down underneath this call, so the dial will
            // never run.
            failDial(connectId, addr)
        }
    }

    /** Tell the core a dial did not produce a channel. Safe at any point,
     *  including after [shutdown]: the bridge handle outlives the radio (see
     *  [BleService.stopBle]), and an answer for a dial the core already gave up
     *  on is discarded there rather than allocating anything. */
    private fun failDial(connectId: Long, addr: String) {
        runCatching {
            NativeCore.bleDeliverConnectResult(bridgeHandle, connectId, false, addr, 0, 0)
        }
    }

    /** The dial itself, on an [io] thread. See [connect] for why it is bounded. */
    private fun dial(connectId: Long, addr: String, psm: Int) {
        val mac = addr.substringAfter('/', addr)
        var sock: BluetoothSocket? = null
        var adopted = false
        var watchdog: ScheduledFuture<*>? = null
        try {
            val device = adapter?.getRemoteDevice(mac)
                ?: throw IOException("no adapter / bad addr $addr")
            val s = device.createInsecureL2capChannel(psm)
            sock = s
            // Supersede any dial to this device still parked in connect():
            // the core only re-dials once it has given up on the last one,
            // so the old socket is dead weight holding an LE connect slot.
            dialing.put(mac, s)?.let { closeQuietly(it) }
            // Re-checked here, and in this order. [shutdown] sets `stopped`
            // and only then drains [dialing], so registering before checking
            // is what makes the race unloseable: either the drain sees our
            // socket and closes it, or we see `stopped` and close it
            // ourselves. Checking first and registering after would let a
            // dial slip between the two and outlive the radio, parked in the
            // BT stack where `shutdownNow` cannot reach it.
            if (stopped) throw IOException("radio stopped")
            watchdog = armDialWatchdog(mac, s)
            s.connect()
            val chId = NativeCore.bleDeliverConnectResult(
                bridgeHandle, connectId, true, addr, sendMtu(s), recvMtu(s),
            )
            // Adopted *before* the channel is started, not after: from this
            // point the socket belongs to [channels]. A throw out of
            // startChannel must not close it underneath the core, nor report
            // this dial failed after it has already been reported succeeded.
            if (chId > 0) {
                adopted = true
                startChannel(chId, s)
            }
        } catch (e: Exception) {
            Log.w(TAG, "connect $addr psm $psm failed: ${e.message}")
            if (!adopted) failDial(connectId, addr)
        } finally {
            // Nothing left to watch: dropping it keeps a resolved dial's socket
            // out of the shared scheduler's queue for the next 15s.
            watchdog?.cancel(false)
            // Only clear the entry if it is still ours — a superseding dial
            // owns it now, and closing its socket would kill a live attempt.
            sock?.let { dialing.remove(mac, it) }
            if (!adopted) sock?.let { closeQuietly(it) }
        }
    }

    /** Close a dial still unresolved after [DIAL_WATCHDOG_MS], so a connect
     *  Android never answers cannot hold an LE connect slot indefinitely.
     *  Closing makes the parked `connect()` throw, and the dial's own `finally`
     *  does the rest. */
    private fun armDialWatchdog(mac: String, sock: BluetoothSocket): ScheduledFuture<*>? =
        runCatching {
            retryExec.schedule({
                if (dialing.remove(mac, sock)) {
                    Log.w(TAG, "dial $mac still unresolved after ${DIAL_WATCHDOG_MS}ms — closing")
                    closeQuietly(sock)
                }
            }, DIAL_WATCHDOG_MS, TimeUnit.MILLISECONDS)
        }.getOrNull()

    /** The first [bytes] bytes of a hex string, as bytes. Null if it is too
     *  short or not hex — never a partially-decoded prefix. */
    private fun hexPrefix(hex: String, bytes: Int): ByteArray? {
        if (hex.length < bytes * 2) return null
        return runCatching {
            ByteArray(bytes) { i -> hex.substring(i * 2, i * 2 + 2).toInt(16).toByte() }
        }.getOrNull()
    }

    /** `s` as UTF-8, cut to at most [max] bytes on a character boundary. */
    private fun truncateUtf8(s: String, max: Int): ByteArray {
        var text = s
        while (text.toByteArray(Charsets.UTF_8).size > max) {
            text = text.dropLast(1)
        }
        return text.toByteArray(Charsets.UTF_8)
    }

    fun startAdvertising(psm: Int) {
        if (stopped) return
        advertisePsm = psm
        // Stop-before-start hygiene: never orphan a prior advertiser set on a
        // re-advertise (retry / radio restart), which itself burns a slot.
        stopAdvertising()
        // Null whenever the adapter is not ON. Returning silently here is how a
        // radio ends up believing it advertises when it does not.
        val adv = adapter?.bluetoothLeAdvertiser ?: run {
            reportAdapterOff("startAdvertising")
            return
        }
        advertiser = adv
        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_BALANCED)
            .setConnectable(true)
            .setTimeout(0)
            .build()
        // The PSM rides in the PRIMARY advert (passively received every interval),
        // not the scan response — a scan response only arrives after a successful
        // active-scan SCAN_REQ/RSP round-trip, which drops asymmetrically across
        // chipsets and left peers undiscoverable. To fit one 31-byte legacy PDU we
        // key the 2-byte PSM service-data on a 16-bit UUID (PSM_SD_UUID16) so it
        // sits alongside the full 128-bit FIPS service UUID (used for the scan
        // filter): ~3 (flags) + 18 (128-bit UUID) + 6 (16-bit service-data) = 27B.
        val psmLe = byteArrayOf((psm and 0xFF).toByte(), ((psm shr 8) and 0xFF).toByte())
        val advData = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(FIPS_PARCEL_UUID)
            .addServiceData(PSM_SD_PARCEL_UUID, psmLe)
            .build()
        // The chosen name goes in the scan response's own 31 bytes, so the
        // primary advert above is untouched and the PSM's delivery guarantee is
        // unchanged. Null when no name has been pushed yet — advertise nothing
        // rather than an empty string, so a scanner can tell "chose no name"
        // from "advertised a blank one".
        // <6-byte node_addr prefix><UTF-8 name>. Both or neither: a name with
        // nobody attached to it is unattributable, and an address with no name
        // says nothing the peer row did not already know.
        val nodePrefix = localNodeAddrHex?.let { hexPrefix(it, NODE_PREFIX_BYTES) }
        val nameBytes = localName?.let { truncateUtf8(it, MAX_NAME_BYTES) }
        val scanResponse = if (nodePrefix != null && nameBytes != null) {
            AdvertiseData.Builder()
                .setIncludeDeviceName(false)
                .addServiceData(NAME_SD_PARCEL_UUID, nodePrefix + nameBytes)
                .build()
        } else {
            null
        }
        val cb = object : AdvertiseCallback() {
            override fun onStartSuccess(settingsInEffect: AdvertiseSettings?) {
                advertiseRetries = 0
                BleHealth.advertiserExhausted = false
                // Reports what actually went out, not what was configured: the
                // scan response is only built when BOTH the name and the node
                // address are known, and a log that claimed otherwise sent me
                // hunting on the receiving device for a name never sent.
                Log.i(TAG, "advertising PSM $psm (in primary advert)" +
                    if (scanResponse != null) {
                        ", name '$localName' as ${nodePrefix?.size ?: 0}B+name in scan response"
                    } else {
                        ", no scan response (name=$localName nodeAddr=$localNodeAddrHex)"
                    })
                if (bridgeHandle != 0L) NativeCore.bleDeliverAdvertisingState(bridgeHandle, true)
            }
            override fun onStartFailure(errorCode: Int) {
                Log.e(TAG, "advertise failed: $errorCode (1=DATA_TOO_LARGE, 2=TOO_MANY_ADVERTISERS)")
                if (bridgeHandle != 0L) NativeCore.bleDeliverAdvertisingState(bridgeHandle, false)
                if (errorCode == ADVERTISE_FAILED_TOO_MANY_ADVERTISERS) {
                    // Every advertising slot is taken (usually Play Services'
                    // Nearby Share / Fast Pair). Flag it for the UI and retry.
                    BleHealth.advertiserExhausted = true
                    scheduleAdvertiseRetry()
                }
            }
        }
        advertiseCallback = cb
        try {
            if (scanResponse != null) {
                adv.startAdvertising(settings, advData, scanResponse, cb)
            } else {
                adv.startAdvertising(settings, advData, cb)
            }
        } catch (e: Exception) {
            Log.e(TAG, "startAdvertising failed", e)
            if (bridgeHandle != 0L) NativeCore.bleDeliverAdvertisingState(bridgeHandle, false)
            scheduleAdvertiseRetry()
        }
    }

    /** Re-attempt advertising on an exponential backoff (5→10→20→40s, capped 60s)
     *  until a BLE advertising slot frees up. Cleared on the next success. */
    private fun scheduleAdvertiseRetry() {
        if (stopped) return
        val delay = minOf(60L, 5L shl minOf(advertiseRetries, 3))
        advertiseRetries++
        Log.i(TAG, "advertise retry in ${delay}s (slot exhausted)")
        runCatching {
            retryExec.schedule(
                { if (!stopped) startAdvertising(advertisePsm) },
                delay,
                TimeUnit.SECONDS,
            )
        }
    }

    fun stopAdvertising() {
        advertiseCallback?.let { runCatching { advertiser?.stopAdvertising(it) } }
        advertiseCallback = null
        if (bridgeHandle != 0L) NativeCore.bleDeliverAdvertisingState(bridgeHandle, false)
    }

    fun startScanning() {
        if (stopped) return
        // Stop-before-start hygiene: a re-arm (retry / radio restart) must not
        // orphan a prior scan callback.
        stopScanning()
        // Null whenever the adapter is not ON — the silent return that made a
        // Bluetooth off/on cycle leave this process permanently deaf.
        val sc = adapter?.bluetoothLeScanner ?: run {
            reportAdapterOff("startScanning")
            return
        }
        scanner = sc
        val filters = listOf(ScanFilter.Builder().setServiceUuid(FIPS_PARCEL_UUID).build())
        val settings = ScanSettings.Builder()
            .apply {
                if (backgroundMode) {
                    setScanMode(ScanSettings.SCAN_MODE_LOW_POWER)
                    // Batch results in the controller and deliver every few seconds
                    // so the host CPU stays asleep between batches. Only when the
                    // chipset can actually offload — otherwise a report delay just
                    // adds latency without saving a wakeup.
                    if (adapter?.isOffloadedScanBatchingSupported == true) {
                        setReportDelay(BACKGROUND_BATCH_MS)
                    }
                } else {
                    setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
                }
            }
            .build()
        val cb = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, result: ScanResult) {
                handleScanResult(result)
            }

            // Batched delivery path (background mode with setReportDelay > 0).
            override fun onBatchScanResults(results: List<ScanResult>) {
                results.forEach { handleScanResult(it) }
            }

            override fun onScanFailed(errorCode: Int) {
                Log.e(TAG, "scan failed: $errorCode (2=APP_REGISTRATION_FAILED, 6=TOO_FREQUENT)")
                scheduleScanRetry(errorCode)
            }
        }
        scanCallback = cb
        try {
            sc.startScan(filters, settings, cb)
            Log.i(TAG, "scanning for FIPS peers (${if (backgroundMode) "low-power" else "low-latency"})")
            if (bridgeHandle != 0L) NativeCore.bleDeliverScanningState(bridgeHandle, true)
            // A scanner that has only just started has not been silent yet, so
            // the streak restarts here — this is what keeps the warning off
            // screen in the first moments after the mesh is switched on. The
            // *verdict* ([BleHealth.scannerConfirmedSilent]) deliberately does
            // not restart with it: see that field.
            BleHealth.emptyScanWindows = 0
            scanSummaryTask = runCatching {
                retryExec.scheduleWithFixedDelay(
                    { if (!stopped) runCatching { logScanSummary() } },
                    SCAN_SUMMARY_SECS,
                    SCAN_SUMMARY_SECS,
                    TimeUnit.SECONDS,
                )
            }.getOrNull()
        } catch (e: Exception) {
            Log.e(TAG, "startScanning failed", e)
            scheduleScanRetry(-1)
        }
    }

    private fun handleScanResult(result: ScanResult) {
        scanRetries = 0 // a result proves scanning is live; reset backoff
        val addr = "$ADAPTER/${result.device.address}"
        // PSM rides in the primary advert under the compact 16-bit UUID;
        // fall back to the legacy 128-bit-keyed service-data for any peer
        // still on the old scan-response layout.
        val sd = result.scanRecord?.getServiceData(PSM_SD_PARCEL_UUID)
            ?: result.scanRecord?.getServiceData(FIPS_PARCEL_UUID)
        val psm = if (sd != null && sd.size >= 2) {
            (sd[0].toInt() and 0xFF) or ((sd[1].toInt() and 0xFF) shl 8) // 16-bit LE
        } else 0
        // Only report a peer once its real PSM is known (it rides the
        // scan-response service data). A psm=0 sighting is the primary
        // advert without the scan response yet — reporting it makes the
        // core fall back to the legacy default PSM (0x0085) and dial the
        // wrong L2CAP port, which the peer rejects every time.
        if (psm > 0) {
            scanWithPsm.incrementAndGet()
            scanPsmAddrs.add(addr)
            NativeCore.bleDeliverScan(bridgeHandle, addr, psm, result.rssi)
            // The peer's chosen name, when its scan response reached us. Pushed
            // separately from the PSM: it is a Myco-layer label with no bearing
            // on routing, so it never enters the fips bridge. Absent is normal
            // — a scan response can simply not arrive — and pushing nothing is
            // how the display layer keeps showing the npub-derived name.
            result.scanRecord?.getServiceData(NAME_SD_PARCEL_UUID)
                ?.takeIf { it.size > NODE_PREFIX_BYTES }
                ?.let { blob ->
                    val nodePrefix = blob.take(NODE_PREFIX_BYTES)
                        .joinToString("") { "%02x".format(it) }
                    val advertised = String(
                        blob, NODE_PREFIX_BYTES, blob.size - NODE_PREFIX_BYTES, Charsets.UTF_8,
                    )
                    NativeCore.bleDeliverAdvertName(nodePrefix, advertised)
                    // Logged on change only. The push itself fires several
                    // times a second per peer, but a name that just appeared or
                    // just changed is the one thing worth seeing in a log.
                    if (lastAdvertName.put(nodePrefix, advertised) != advertised) {
                        Log.i(TAG, "$addr ($nodePrefix…) advertises name '$advertised'")
                    }
                }
        } else {
            // Counted, not logged: this fires several times a second per peer
            // while a scan response is outstanding. [logScanSummary] reports
            // the addresses that never produced one, which is the real signal.
            scanWithoutPsm.incrementAndGet()
            scanNoPsmAddrs.add(addr)
        }
    }

    /** Summarise the last window of scan reports; scheduled from [startScanning]. */
    private fun logScanSummary() {
        val withPsm = scanWithPsm.getAndSet(0)
        val withoutPsm = scanWithoutPsm.getAndSet(0)
        val silent = scanNoPsmAddrs.minus(scanPsmAddrs)
        scanPsmAddrs.clear()
        scanNoPsmAddrs.clear()
        val secs = SCAN_SUMMARY_SECS
        if (withPsm == 0 && withoutPsm == 0) {
            // Loud on purpose. A radio that advertises and listens but scans
            // nothing is inbound-only: it can never learn a peer's PSM, so
            // every dial it makes falls back to the configured default and
            // fails. That is invisible unless it is said out loud.
            val empty = BleHealth.emptyScanWindows + 1
            BleHealth.emptyScanWindows = empty
            if (empty >= SILENT_WINDOWS_BEFORE_ALARM) BleHealth.scannerConfirmedSilent = true
            Log.w(TAG, "scan summary: 0 adverts in ${secs}s — scanner produced nothing (window $empty)")
            return
        }
        // One advert of any kind proves the scanner is delivering, so both the
        // streak and the verdict are over. This is the *only* place the
        // verdict is cleared: nothing short of a real advert counts as the
        // radio having recovered.
        BleHealth.emptyScanWindows = 0
        BleHealth.scannerConfirmedSilent = false
        val tail = if (silent.isEmpty()) {
            ""
        } else {
            ", no PSM from ${silent.size}: ${silent.joinToString(" ")}"
        }
        Log.i(
            TAG,
            "scan summary: ${withPsm + withoutPsm} adverts in ${secs}s " +
                "($withPsm with psm, $withoutPsm awaiting scan response$tail)",
        )
    }

    /** Re-arm scanning after a failure, so a transient throttle/error doesn't
     *  permanently stop peer discovery. Android caps ~5 `startScan` calls per 30s
     *  (SCAN_FAILED_SCANNING_TOO_FREQUENTLY) — restarting inside that window just
     *  re-trips it, so the throttle case gets a 30s floor; other errors use the
     *  same exponential backoff (5→10→20→40s, capped 60s) as the advertiser. The
     *  backoff resets once a scan result comes in. */
    private fun scheduleScanRetry(errorCode: Int) {
        if (bridgeHandle != 0L) NativeCore.bleDeliverScanningState(bridgeHandle, false)
        if (stopped) return
        val backoff = minOf(60L, 5L shl minOf(scanRetries, 3))
        val delay =
            if (errorCode == ScanCallback.SCAN_FAILED_SCANNING_TOO_FREQUENTLY) {
                maxOf(30L, backoff)
            } else {
                backoff
            }
        scanRetries++
        Log.i(TAG, "scan retry in ${delay}s (error $errorCode)")
        runCatching {
            retryExec.schedule(
                { if (!stopped) startScanning() },
                delay,
                TimeUnit.SECONDS,
            )
        }
    }

    fun stopScanning() {
        scanCallback?.let { runCatching { scanner?.stopScan(it) } }
        scanCallback = null
        // No scan, nothing to summarise — otherwise the timer would report
        // "0 adverts" against a scanner nobody asked to be running.
        scanSummaryTask?.cancel(false)
        scanSummaryTask = null
        // Silence only means something while we are listening. A stopped
        // scanner has heard nothing by definition, and leaving the streak
        // standing would accuse the phone of a fault while the mesh is off.
        BleHealth.emptyScanWindows = 0
        if (bridgeHandle != 0L) NativeCore.bleDeliverScanningState(bridgeHandle, false)
    }

    fun closeChannel(chId: Long) {
        channels.remove(chId)?.let { closeQuietly(it) }
        dropGatt(chId)
    }

    /** Tear everything down (called when the service stops). */
    fun shutdown() {
        stopped = true
        if (instance === this) instance = null
        // The radio is being destroyed — on a mesh toggle a fresh one is built
        // in its place, and it must start with no verdict against it. This is
        // the one thing other than a real advert that clears the latch, and it
        // is not a shortcut: there is no scanner left to be deaf.
        BleHealth.scannerConfirmedSilent = false
        stopScanning()
        stopAdvertising()
        runCatching { serverSocket?.close() }
        serverSocket = null
        // The socket is gone, so the PSM must not outlive it: a stale non-zero
        // value here would tell [BleService] a dead radio was healthy.
        listenPsm = 0
        // Dials parked in connect() outlive the radio otherwise: the thread
        // pool is shut down below, but shutdownNow cannot interrupt a thread
        // blocked in the BT stack. Closing the socket is what releases it.
        dialing.values.toList().forEach { closeQuietly(it) }
        dialing.clear()
        channels.keys.toList().forEach { closeChannel(it) }
        gatts.keys.toList().forEach { dropGatt(it) }
        io.shutdownNow()
        retryExec.shutdownNow()
    }

    // ---- internals ----

    private fun acceptLoop(ss: BluetoothServerSocket) {
        while (true) {
            val sock = try {
                ss.accept()
            } catch (e: IOException) {
                break // listener closed
            }
            val chId = NativeCore.bleDeliverInbound(
                bridgeHandle, "$ADAPTER/${sock.remoteDevice.address}", sendMtu(sock), recvMtu(sock),
            )
            if (chId > 0) startChannel(chId, sock) else closeQuietly(sock)
        }
    }

    private fun startChannel(chId: Long, sock: BluetoothSocket) {
        channels[chId] = sock
        boostPriority(chId, sock.remoteDevice)
        io.execute { readerLoop(chId, sock) }
        io.execute { writerLoop(chId, sock) }
    }

    /**
     * Request a high-priority (low-interval) LE connection for throughput. L2CAP
     * CoC has no connection-parameter API, so we open a parallel GATT to the same
     * device purely to call [BluetoothGatt.requestConnectionPriority] — it shares
     * the physical ACL link, so the faster interval applies to the CoC channel too.
     */
    private fun boostPriority(chId: Long, device: BluetoothDevice) {
        runCatching {
            Log.i(TAG, "boostPriority: GATT to ${device.address} (low interval + 2M PHY)")
            val prio = GattPrio()
            // TRANSPORT_LE is mandatory here: the 3-arg connectGatt defaults to
            // TRANSPORT_AUTO, which on a dual-mode peer can bring up a classic
            // BR/EDR link. BR/EDR between two phones makes Android auto-negotiate
            // MAP/PBAP and pop the "<device> wants to access your messages" system
            // dialog. The mesh is LE-only (L2CAP CoC), so pin GATT to LE too.
            val gatt = device.connectGatt(appContext, false, object : BluetoothGattCallback() {
                override fun onConnectionStateChange(g: BluetoothGatt, status: Int, newState: Int) {
                    if (newState == BluetoothProfile.STATE_CONNECTED) {
                        prio.connected = true
                        // HIGH from the start: the noise handshake right after
                        // connect is many small round-trips and wants the low
                        // interval. Demoted to BALANCED once the link idles.
                        boostNow(prio, g)
                        // 2M PHY ~doubles the raw rate over 1M (and halves radio
                        // on-time per byte — keep it regardless of priority).
                        runCatching {
                            g.setPreferredPhy(
                                BluetoothDevice.PHY_LE_2M_MASK,
                                BluetoothDevice.PHY_LE_2M_MASK,
                                BluetoothDevice.PHY_OPTION_NO_PREFERRED,
                            )
                        }
                        Log.i(TAG, "GATT up: requested HIGH priority + 2M PHY")
                    }
                }

                override fun onPhyUpdate(g: BluetoothGatt, txPhy: Int, rxPhy: Int, status: Int) {
                    Log.i(TAG, "PHY now tx=$txPhy rx=$rxPhy (2=2M) status=$status")
                }
            }, BluetoothDevice.TRANSPORT_LE)
            if (gatt != null) {
                prio.gatt = gatt
                gatts[chId] = prio
            }
        }.onFailure { Log.w(TAG, "boostPriority failed: ${it.message}") }
    }

    /** Grant HIGH priority and arm the idle-demotion check. */
    private fun boostNow(prio: GattPrio, gatt: BluetoothGatt) {
        prio.lastBulkMs = android.os.SystemClock.elapsedRealtime()
        synchronized(prio) {
            if (!prio.high) {
                prio.high = true
                runCatching { gatt.requestConnectionPriority(BluetoothGatt.CONNECTION_PRIORITY_HIGH) }
            }
            if (!prio.demotePending) {
                prio.demotePending = true
                scheduleDemoteCheck(prio, gatt, IDLE_DEMOTE_MS)
            }
        }
    }

    /** After [delayMs], demote to BALANCED if no bulk packet moved in the window;
     *  otherwise re-check when the current window would expire. */
    private fun scheduleDemoteCheck(prio: GattPrio, gatt: BluetoothGatt, delayMs: Long) {
        runCatching {
            retryExec.schedule({
                val idle = android.os.SystemClock.elapsedRealtime() - prio.lastBulkMs
                synchronized(prio) {
                    when {
                        !prio.connected || stopped -> prio.demotePending = false
                        idle >= IDLE_DEMOTE_MS -> {
                            prio.demotePending = false
                            prio.high = false
                            runCatching {
                                gatt.requestConnectionPriority(BluetoothGatt.CONNECTION_PRIORITY_BALANCED)
                            }
                            Log.i(TAG, "GATT idle ${idle}ms: demoted to BALANCED priority")
                        }
                        else -> scheduleDemoteCheck(prio, gatt, IDLE_DEMOTE_MS - idle)
                    }
                }
            }, delayMs, TimeUnit.MILLISECONDS)
        }
    }

    /**
     * Called by the reader/writer loops with each packet's size. Bulk-sized
     * packets (sync/site transfers, ~MTU-sized) re-grant HIGH priority and keep
     * it while the burst lasts; control chatter (link heartbeats, WS pings, TCP
     * ACKs — all well under [BULK_BOOST_BYTES]) never re-boosts, so an
     * otherwise-idle link stays on the cheap BALANCED interval.
     */
    private fun noteBulk(chId: Long, n: Int) {
        if (n < BULK_BOOST_BYTES) return
        val prio = gatts[chId] ?: return
        prio.lastBulkMs = android.os.SystemClock.elapsedRealtime()
        if (prio.connected && !prio.high) {
            prio.gatt?.let { boostNow(prio, it) }
        }
    }

    private fun dropGatt(chId: Long) {
        gatts.remove(chId)?.let { prio ->
            prio.connected = false
            prio.gatt?.let { runCatching { it.disconnect(); it.close() } }
        }
    }

    private fun readerLoop(chId: Long, sock: BluetoothSocket) {
        val ins = sock.inputStream
        // A single reusable scratch buffer is safe: bleChannelDeliverRecv copies
        // buf[0 until n] into the core synchronously before it returns, so the next
        // read() never races a chunk still in flight to JNI.
        val buf = ByteArray(MAX_PACKET)
        try {
            while (true) {
                // Forward exactly the bytes read — never assume read() filled buf,
                // and never pass stale bytes past n. The core reframes the stream.
                val n = ins.read(buf)
                if (n < 0) break // EOF → channel closed (handled in finally)
                if (n == 0) continue
                noteBulk(chId, n) // bulk inbound re-grants the low connection interval
                if (!NativeCore.bleChannelDeliverRecv(bridgeHandle, chId, buf, n)) {
                    // deliver_recv == false: the core's bounded inbound queue is
                    // saturated, or the channel is gone. Under FMP reframing a single
                    // dropped chunk desyncs the reframer for the rest of the
                    // connection (it reads a bogus length, then errors). So this is
                    // fatal — reset the channel rather than read on into a corrupted
                    // stream. Falling out of the loop tears down the socket + channel
                    // via onChannelGone; we never silently swallow the drop.
                    Log.w(TAG, "ch $chId: deliver_recv refused (inbound queue full) — resetting channel")
                    break
                }
            }
        } catch (_: IOException) {
        } finally {
            onChannelGone(chId)
        }
    }

    private fun writerLoop(chId: Long, sock: BluetoothSocket) {
        val outs = sock.outputStream
        // Each next_send returns one full FMP-framed FIPS packet (self-delimiting via
        // its 4-byte header) — the core owns framing now, so we write the bytes
        // verbatim. One write = one L2CAP SDU. A single writer thread per channel
        // (started in startChannel) preserves order across packets.
        val buf = ByteArray(MAX_PACKET)
        try {
            while (true) {
                val n = NativeCore.bleChannelNextSend(bridgeHandle, chId, buf, SEND_TIMEOUT_MS)
                when {
                    n < 0 -> break // channel closed
                    n == 0 -> continue // timeout, poll again
                    else -> {
                        noteBulk(chId, n) // bulk outbound re-grants the low interval
                        // OutputStream.write on a blocking socket writes all n bytes
                        // (looping internally) or throws — no manual partial-write loop.
                        outs.write(buf, 0, n)
                        outs.flush()
                    }
                }
            }
        } catch (_: IOException) {
        } finally {
            onChannelGone(chId)
        }
    }

    private fun onChannelGone(chId: Long) {
        channels.remove(chId)?.let { closeQuietly(it) }
        dropGatt(chId)
        runCatching { NativeCore.bleChannelClosed(bridgeHandle, chId) }
    }

    // Cap the MTU we report to the core: L2CAP CoC negotiates up to 64 KB, but BLE
    // moves ~251-byte link-layer packets, so a 64 KB frame fragments into hundreds
    // of LE packets and takes seconds (huge RTT + head-of-line blocking). A few-KB
    // frame keeps latency low while still amortizing framing overhead.
    private fun sendMtu(sock: BluetoothSocket): Int =
        sock.maxTransmitPacketSize.coerceIn(20, MESH_MTU_CAP)

    // Report the real negotiated L2CAP receive MTU (capped): the core's FMP framer
    // uses it as the max allowed packet size, so under-reporting would reject valid
    // large packets. Never 0 here — that would force the core's 2048 default.
    private fun recvMtu(sock: BluetoothSocket): Int =
        sock.maxReceivePacketSize.coerceIn(20, MESH_MTU_CAP)

    private fun closeQuietly(sock: BluetoothSocket) {
        runCatching { sock.close() }
    }

    companion object {
        private const val TAG = "MycoBleRadio"
        private const val ADAPTER = "ble0" // matches fips AndroidIo's adapter tag
        // Scratch-buffer size for the per-channel reader/writer. Comfortably larger
        // than any single L2CAP SDU (recv/send MTU is capped at MESH_MTU_CAP), so a
        // read() fits one SDU and next_send never truncates an outbound packet.
        private const val MAX_PACKET = 8192
        private const val SEND_TIMEOUT_MS = 1000

        /** Backstop for a dial Android never resolves. Comfortably above the
         *  core's probe timeout (10s) so a dial that is merely slow is left
         *  alone, and well below the ~30s+ this has been seen to block for. */
        private const val DIAL_WATCHDOG_MS = 15_000L

        /** How often the scanner reports what it has seen. Long enough that the
         *  summary costs one line a window instead of several a second, short
         *  enough that a scanner going silent shows up while you are watching. */
        private const val SCAN_SUMMARY_SECS = 30L

        /** Consecutive empty [SCAN_SUMMARY_SECS] windows before the radio is
         *  called deaf ([BleHealth.scannerConfirmedSilent]). Two, not one: a
         *  full minute of listening, so a scanner merely slow to deliver its
         *  first result is never accused. */
        private const val SILENT_WINDOWS_BEFORE_ALARM = 2

        /** Background scan: batched delivery window (controller-offloaded). */
        private const val BACKGROUND_BATCH_MS = 5000L

        /** A packet at least this big is "bulk" (site/sync transfer, ~MTU-sized
         *  1357B frames) and re-grants HIGH connection priority. Control chatter —
         *  1B link heartbeats, WS pings and TCP ACKs (~250B on the wire with
         *  FSP+IPv6 overhead) — stays under it and rides BALANCED. */
        private const val BULK_BOOST_BYTES = 512

        /** Demote HIGH → BALANCED after this long without a bulk packet. 30s, not
         *  lower: relay sync traffic arrives in bursts ~10-20s apart, and a 5s
         *  window made every burst renegotiate connection parameters (boost ↔
         *  demote flip-flop every ~15s in the field logs) — churn that risks link
         *  stability on some chipsets for marginal battery gain. */
        private const val IDLE_DEMOTE_MS = 30_000L

        /** Cap on the MTU reported to the core: one full IPv6 packet per L2CAP SDU.
         *  The TUN sends ≤1280-byte IPv6 packets; with FSP's ~77-byte overhead that
         *  fits in 1357, so 1500 carries a whole packet with no fragmentation and
         *  no head-of-line batching of multiple packets into one giant frame. */
        private const val MESH_MTU_CAP = 1500

        /** The live radio, or null when none is running. Set on construction and
         *  cleared on [shutdown]. */
        @Volatile
        private var instance: BleRadio? = null

        @Volatile
        private var nameField: String? = null

        /**
         * The display name this device broadcasts for itself, or null to
         * broadcast none.
         *
         * Process-global rather than per-radio because the app asserts the name
         * on every resume, which can happen before [BleService] has built a
         * radio at all — held here, it is simply read by the next advert. Set
         * while already advertising, it re-issues the advert, since the scan
         * response is fixed at start time and a rename that waited for the next
         * radio restart would look broken to whoever just made it.
         */
        var localName: String?
            get() = nameField
            set(value) {
                val next = value?.trim()?.ifBlank { null }
                if (next == nameField) return
                nameField = next
                instance?.reAdvertiseForName()
            }

        @Volatile
        private var nodeAddrField: String? = null

        /** Our own `node_addr`, hex. Advertised beside [localName] so a scanner
         *  can attribute the name to the peer row that address keys, whatever
         *  transport that peer is ultimately carried on. */
        var localNodeAddrHex: String?
            get() = nodeAddrField
            set(value) {
                val next = value?.trim()?.ifBlank { null }
                if (next == nodeAddrField) return
                nodeAddrField = next
                instance?.reAdvertiseForName()
            }

        /** FIPS service UUID — must match fips-core. */
        val FIPS_UUID: UUID = UUID.fromString("9c90b790-2cc5-42c0-9f87-c9cc40648f4c")
        val FIPS_PARCEL_UUID = ParcelUuid(FIPS_UUID)

        /** Compact 16-bit service-data UUID carrying the 2-byte LE listener PSM in
         *  the PRIMARY advert (0x9C90 = the FIPS UUID's leading 16 bits, via the
         *  Bluetooth base UUID). A 16-bit key keeps PSM service-data + the full
         *  128-bit FIPS UUID inside one 31-byte legacy advert. */
        val PSM_SD_PARCEL_UUID = ParcelUuid.fromString("00009c90-0000-1000-8000-00805f9b34fb")

        /** Compact 16-bit service-data UUID carrying this device's identity-plus-
         *  name blob in the SCAN RESPONSE (0x9C91 — the PSM key's neighbour). It
         *  rides the scan response precisely because the PSM must not: a scan
         *  response needs an active-scan round-trip that drops asymmetrically,
         *  which is fatal for the PSM but merely cosmetic for a name. A name that
         *  never arrives just leaves the peer showing its npub-derived one. */
        val NAME_SD_PARCEL_UUID = ParcelUuid.fromString("00009c91-0000-1000-8000-00805f9b34fb")

        /** Leading bytes of our own `node_addr` prefixed to the advertised name.
         *
         *  The name has to say *whose* it is. Keying it on the BLE MAC instead
         *  only works for a peer currently carried over BLE — but a device is
         *  routinely discovered by advert and then connected over the LAN lane,
         *  and its row is keyed by node address, not by MAC. Six bytes is 48 bits
         *  of node address: ample against accidental collision in a room, and
         *  cheap enough to leave the name most of the payload. */
        const val NODE_PREFIX_BYTES = 6

        /** Longest name that fits the 31-byte scan response beside its 4-byte
         *  service-data header and the node-address prefix. Cut on a UTF-8
         *  boundary, never mid-character. */
        private const val MAX_NAME_BYTES = 31 - 4 - NODE_PREFIX_BYTES
    }
}

/** Process-global BLE health flags read directly by the UI (no AppState round-trip).
 *  Single instance — there is only ever one [BleRadio] per process. */
object BleHealth {
    /** True when the OS refused our advertiser with TOO_MANY_ADVERTISERS: other
     *  apps (typically Google Play Services' Nearby Share / Quick Share / Fast
     *  Pair) hold every BLE advertising slot, so peers can't discover this device.
     *  Cleared automatically once advertising succeeds on a retry. */
    @Volatile
    var advertiserExhausted: Boolean = false

    /**
     * How many consecutive scan-summary windows have produced **no advert at
     * all** — the count behind `scan summary: 0 adverts in 30s`. Restarts at
     * zero every time the scanner starts, which is what keeps a freshly
     * enabled mesh from being accused before it has had time to hear anything.
     */
    @Volatile
    var emptyScanWindows: Int = 0

    /**
     * **The verdict**: this radio has been listening and heard nothing for
     * long enough that it is not merely quiet, it is deaf.
     *
     * This is the observable half of the vendor-stack bug that cost a day of
     * diagnosis: a device whose scan callback never fires is inbound-only — it
     * advertises, accepts connections, and can never learn a peer's PSM, so it
     * never dials anyone. `startScan` reports success throughout. Only the
     * absence of results says so.
     *
     * Kept separate from [emptyScanWindows] because the streak is far too
     * fragile to drive a warning off directly. The scanner re-arms on every
     * fore/background flip, and tapping the warning to open location settings
     * is itself such a flip — so the streak-only version hid the warning the
     * moment the user acted on it, then stayed hidden for another minute with
     * nothing fixed. A conclusion once reached is held until something
     * disproves it, and the only thing that disproves it is a real advert (see
     * [BleRadio.logScanSummary]) or the radio ceasing to exist (see
     * [BleRadio.shutdown]).
     */
    @Volatile
    var scannerConfirmedSilent: Boolean = false
}
