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
import java.io.InputStream
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

/**
 * The Android BLE radio. Kotlin owns the radio; the Rust core (fips `AndroidIo`)
 * drives it through the byte-bridge:
 *
 * - The Rust core calls the control methods here (listen/connect/advertise/scan/
 *   close) via JNI — see [app.myco.core.NativeCore.bleBridgeNew].
 * - This class pushes inbound bytes/events to the core and pulls outbound bytes
 *   via the `NativeCore.ble*` exports.
 *
 * L2CAP CoC over Android's [BluetoothSocket] is a byte **stream**, so each FIPS
 * packet is framed with a 2-byte big-endian length prefix and reassembled on
 * read — the bytes handed to/from the core are clean per-packet boundaries.
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
    private val gatts = ConcurrentHashMap<Long, BluetoothGatt>()

    @Volatile
    private var stopped = false

    private var serverSocket: BluetoothServerSocket? = null
    private var advertiser: BluetoothLeAdvertiser? = null
    private var advertiseCallback: AdvertiseCallback? = null
    private var scanner: BluetoothLeScanner? = null
    private var scanCallback: ScanCallback? = null

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
        // See docs/reference/ble-wire.md.
        val psmLe = byteArrayOf((psm and 0xFF).toByte(), ((psm shr 8) and 0xFF).toByte())
        val advData = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(FIPS_PARCEL_UUID)
            .addServiceData(PSM_SD_PARCEL_UUID, psmLe)
            .build()
        val cb = object : AdvertiseCallback() {
            override fun onStartSuccess(settingsInEffect: AdvertiseSettings?) {
                Log.i(TAG, "advertising PSM $psm (in primary advert)")
            }
            override fun onStartFailure(errorCode: Int) {
                Log.e(TAG, "advertise failed: $errorCode (1=DATA_TOO_LARGE)")
            }
        }
        advertiseCallback = cb
        try {
            adv.startAdvertising(settings, advData, cb)
        } catch (e: Exception) {
            Log.e(TAG, "startAdvertising failed", e)
        }
    }

    fun stopAdvertising() {
        advertiseCallback?.let { runCatching { advertiser?.stopAdvertising(it) } }
        advertiseCallback = null
    }

    fun startScanning() {
        if (stopped) return
        val sc = adapter?.bluetoothLeScanner ?: return
        scanner = sc
        val filters = listOf(ScanFilter.Builder().setServiceUuid(FIPS_PARCEL_UUID).build())
        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
            .build()
        val cb = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, result: ScanResult) {
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

            override fun onScanFailed(errorCode: Int) {
                Log.e(TAG, "scan failed: $errorCode")
            }
        }
        scanCallback = cb
        try {
            sc.startScan(filters, settings, cb)
            Log.i(TAG, "scanning for FIPS peers")
        } catch (e: Exception) {
            Log.e(TAG, "startScanning failed", e)
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
            val gatt = device.connectGatt(appContext, false, object : BluetoothGattCallback() {
                override fun onConnectionStateChange(g: BluetoothGatt, status: Int, newState: Int) {
                    if (newState == BluetoothProfile.STATE_CONNECTED) {
                        val ok = runCatching {
                            g.requestConnectionPriority(BluetoothGatt.CONNECTION_PRIORITY_HIGH)
                        }.getOrDefault(false)
                        // 2M PHY ~doubles the raw rate over 1M.
                        runCatching {
                            g.setPreferredPhy(
                                BluetoothDevice.PHY_LE_2M_MASK,
                                BluetoothDevice.PHY_LE_2M_MASK,
                                BluetoothDevice.PHY_OPTION_NO_PREFERRED,
                            )
                        }
                        Log.i(TAG, "GATT up: requestConnectionPriority(HIGH)=$ok, requested 2M PHY")
                    }
                }

                override fun onPhyUpdate(g: BluetoothGatt, txPhy: Int, rxPhy: Int, status: Int) {
                    Log.i(TAG, "PHY now tx=$txPhy rx=$rxPhy (2=2M) status=$status")
                }
            })
            if (gatt != null) gatts[chId] = gatt
        }.onFailure { Log.w(TAG, "boostPriority failed: ${it.message}") }
    }

    private fun dropGatt(chId: Long) {
        gatts.remove(chId)?.let { runCatching { it.disconnect(); it.close() } }
    }

    private fun readerLoop(chId: Long, sock: BluetoothSocket) {
        val ins = sock.inputStream
        val header = ByteArray(2)
        try {
            while (true) {
                if (!readFully(ins, header, 2)) break
                val len = ((header[0].toInt() and 0xFF) shl 8) or (header[1].toInt() and 0xFF)
                if (len <= 0 || len > MAX_PACKET) break
                val payload = ByteArray(len)
                if (!readFully(ins, payload, len)) break
                if (!NativeCore.bleChannelDeliverRecv(bridgeHandle, chId, payload, len)) break
            }
        } catch (_: IOException) {
        } finally {
            onChannelGone(chId)
        }
    }

    private fun writerLoop(chId: Long, sock: BluetoothSocket) {
        val outs = sock.outputStream
        val buf = ByteArray(MAX_PACKET)
        // Header + payload coalesced into one buffer so each packet is a single
        // write → a single L2CAP SDU. Writing the 2-byte prefix separately emitted
        // extra tiny SDUs and wasted connection-event airtime.
        val frame = ByteArray(MAX_PACKET + FRAME_HEADER)
        try {
            while (true) {
                val n = NativeCore.bleChannelNextSend(bridgeHandle, chId, buf, SEND_TIMEOUT_MS)
                when {
                    n < 0 -> break // channel closed
                    n == 0 -> continue // timeout, poll again
                    else -> {
                        frame[0] = ((n shr 8) and 0xFF).toByte() // 2-byte BE length prefix
                        frame[1] = (n and 0xFF).toByte()
                        System.arraycopy(buf, 0, frame, FRAME_HEADER, n)
                        outs.write(frame, 0, n + FRAME_HEADER)
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
        (sock.maxTransmitPacketSize - FRAME_HEADER).coerceIn(20, MESH_MTU_CAP)

    private fun recvMtu(sock: BluetoothSocket): Int =
        sock.maxReceivePacketSize.coerceIn(20, MESH_MTU_CAP)

    private fun closeQuietly(sock: BluetoothSocket) {
        runCatching { sock.close() }
    }

    /** Read exactly [n] bytes into [buf]; false on EOF/close. */
    private fun readFully(ins: InputStream, buf: ByteArray, n: Int): Boolean {
        var off = 0
        while (off < n) {
            val r = ins.read(buf, off, n - off)
            if (r < 0) return false
            off += r
        }
        return true
    }

    companion object {
        private const val TAG = "MycoBleRadio"
        private const val ADAPTER = "ble0" // matches fips AndroidIo's adapter tag
        private const val FRAME_HEADER = 2
        private const val MAX_PACKET = 8192
        private const val SEND_TIMEOUT_MS = 1000

        /** Cap on the MTU reported to the core: one full IPv6 packet per L2CAP SDU.
         *  The TUN sends ≤1280-byte IPv6 packets; with FSP's ~77-byte overhead that
         *  fits in 1357, so 1500 carries a whole packet with no fragmentation and
         *  no head-of-line batching of multiple packets into one giant frame. */
        private const val MESH_MTU_CAP = 1500

        /** FIPS service UUID — must match fips-core (ble-wire.md). */
        val FIPS_UUID: UUID = UUID.fromString("9c90b790-2cc5-42c0-9f87-c9cc40648f4c")
        val FIPS_PARCEL_UUID = ParcelUuid(FIPS_UUID)

        /** Compact 16-bit service-data UUID carrying the 2-byte LE listener PSM in
         *  the PRIMARY advert (0x9C90 = the FIPS UUID's leading 16 bits, via the
         *  Bluetooth base UUID). A 16-bit key keeps PSM service-data + the full
         *  128-bit FIPS UUID inside one 31-byte legacy advert. See ble-wire.md. */
        val PSM_SD_PARCEL_UUID = ParcelUuid.fromString("00009c90-0000-1000-8000-00805f9b34fb")
    }
}
