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
import java.util.concurrent.TimeUnit

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
    private var advertiser: BluetoothLeAdvertiser? = null
    private var advertiseCallback: AdvertiseCallback? = null
    private var scanner: BluetoothLeScanner? = null
    private var scanCallback: ScanCallback? = null

    // Advertiser-retry state: when the OS refuses our advert because every BLE
    // advertising slot is taken (TOO_MANY_ADVERTISERS, typically Play Services'
    // Nearby Share/Fast Pair), we keep retrying on a backoff until a slot frees.
    private val retryExec = Executors.newSingleThreadScheduledExecutor()
    @Volatile private var advertisePsm = 0
    private var advertiseRetries = 0
    // Scanner-retry state: a failed scan (Android throttles ~5 startScan/30s →
    // SCAN_FAILED_SCANNING_TOO_FREQUENTLY) used to be logged and abandoned, which
    // permanently killed peer discovery until the mesh was toggled. We re-arm it.
    private var scanRetries = 0

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

    fun bindBridge(handle: Long) {
        bridgeHandle = handle
    }

    // ---- control methods, invoked by the Rust core via JNI ----

    /** Open an insecure L2CAP listener; return the OS-assigned PSM (0 on failure). */
    fun listen(): Int {
        if (stopped) return 0
        val a = adapter ?: return 0
        return try {
            val ss = a.listenUsingInsecureL2capChannel()
            serverSocket = ss
            val psm = ss.psm
            io.execute { acceptLoop(ss) }
            Log.i(TAG, "L2CAP listening on PSM $psm")
            psm
        } catch (e: Exception) {
            Log.e(TAG, "listen failed", e)
            0
        }
    }

    /** Dial a peer; deliver the result (and, on success, start the channel). */
    fun connect(connectId: Long, addr: String, psm: Int) {
        if (stopped) return
        runCatching {
        io.execute {
            val mac = addr.substringAfter('/', addr)
            try {
                val device = adapter?.getRemoteDevice(mac)
                    ?: throw IOException("no adapter / bad addr $addr")
                val sock = device.createInsecureL2capChannel(psm)
                sock.connect()
                val chId = NativeCore.bleDeliverConnectResult(
                    bridgeHandle, connectId, true, addr, sendMtu(sock), recvMtu(sock),
                )
                if (chId > 0) startChannel(chId, sock) else closeQuietly(sock)
            } catch (e: Exception) {
                Log.w(TAG, "connect $addr psm $psm failed: ${e.message}")
                NativeCore.bleDeliverConnectResult(bridgeHandle, connectId, false, addr, 0, 0)
            }
        }
        }
    }

    fun startAdvertising(psm: Int) {
        if (stopped) return
        advertisePsm = psm
        // Stop-before-start hygiene: never orphan a prior advertiser set on a
        // re-advertise (retry / radio restart), which itself burns a slot.
        stopAdvertising()
        val adv = adapter?.bluetoothLeAdvertiser ?: return
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
        val cb = object : AdvertiseCallback() {
            override fun onStartSuccess(settingsInEffect: AdvertiseSettings?) {
                advertiseRetries = 0
                BleHealth.advertiserExhausted = false
                Log.i(TAG, "advertising PSM $psm (in primary advert)")
            }
            override fun onStartFailure(errorCode: Int) {
                Log.e(TAG, "advertise failed: $errorCode (1=DATA_TOO_LARGE, 2=TOO_MANY_ADVERTISERS)")
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
            adv.startAdvertising(settings, advData, cb)
        } catch (e: Exception) {
            Log.e(TAG, "startAdvertising failed", e)
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
    }

    fun startScanning() {
        if (stopped) return
        // Stop-before-start hygiene: a re-arm (retry / radio restart) must not
        // orphan a prior scan callback.
        stopScanning()
        val sc = adapter?.bluetoothLeScanner ?: return
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
            NativeCore.bleDeliverScan(bridgeHandle, addr, psm, result.rssi)
        } else {
            Log.d(TAG, "scan $addr: PSM not in advert yet (awaiting scan response)")
        }
    }

    /** Re-arm scanning after a failure, so a transient throttle/error doesn't
     *  permanently stop peer discovery. Android caps ~5 `startScan` calls per 30s
     *  (SCAN_FAILED_SCANNING_TOO_FREQUENTLY) — restarting inside that window just
     *  re-trips it, so the throttle case gets a 30s floor; other errors use the
     *  same exponential backoff (5→10→20→40s, capped 60s) as the advertiser. The
     *  backoff resets once a scan result comes in. */
    private fun scheduleScanRetry(errorCode: Int) {
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
    }

    fun closeChannel(chId: Long) {
        channels.remove(chId)?.let { closeQuietly(it) }
        dropGatt(chId)
    }

    /** Tear everything down (called when the service stops). */
    fun shutdown() {
        stopped = true
        stopScanning()
        stopAdvertising()
        runCatching { serverSocket?.close() }
        serverSocket = null
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

        /** Background scan: batched delivery window (controller-offloaded). */
        private const val BACKGROUND_BATCH_MS = 5000L

        /** A packet at least this big is "bulk" (site/sync transfer, ~MTU-sized
         *  1357B frames) and re-grants HIGH connection priority. Control chatter —
         *  1B link heartbeats, WS pings and TCP ACKs (~250B on the wire with
         *  FSP+IPv6 overhead) — stays under it and rides BALANCED. */
        private const val BULK_BOOST_BYTES = 512

        /** Demote HIGH → BALANCED after this long without a bulk packet. */
        private const val IDLE_DEMOTE_MS = 5000L

        /** Cap on the MTU reported to the core: one full IPv6 packet per L2CAP SDU.
         *  The TUN sends ≤1280-byte IPv6 packets; with FSP's ~77-byte overhead that
         *  fits in 1357, so 1500 carries a whole packet with no fragmentation and
         *  no head-of-line batching of multiple packets into one giant frame. */
        private const val MESH_MTU_CAP = 1500

        /** FIPS service UUID — must match fips-core. */
        val FIPS_UUID: UUID = UUID.fromString("9c90b790-2cc5-42c0-9f87-c9cc40648f4c")
        val FIPS_PARCEL_UUID = ParcelUuid(FIPS_UUID)

        /** Compact 16-bit service-data UUID carrying the 2-byte LE listener PSM in
         *  the PRIMARY advert (0x9C90 = the FIPS UUID's leading 16 bits, via the
         *  Bluetooth base UUID). A 16-bit key keeps PSM service-data + the full
         *  128-bit FIPS UUID inside one 31-byte legacy advert. */
        val PSM_SD_PARCEL_UUID = ParcelUuid.fromString("00009c90-0000-1000-8000-00805f9b34fb")
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
}
