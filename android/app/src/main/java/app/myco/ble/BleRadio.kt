package app.myco.ble

import android.annotation.SuppressLint
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothManager
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

    private var bridgeHandle: Long = 0
    private val io = Executors.newCachedThreadPool()
    private val channels = ConcurrentHashMap<Long, BluetoothSocket>()

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
        // Primary advert carries the UUID; the PSM service-data rides the scan
        // response (a 128-bit UUID + service data won't both fit one 31-byte PDU).
        val primary = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(FIPS_PARCEL_UUID)
            .build()
        val psmLe = byteArrayOf((psm and 0xFF).toByte(), ((psm shr 8) and 0xFF).toByte())
        val scanResponse = AdvertiseData.Builder()
            .addServiceData(FIPS_PARCEL_UUID, psmLe)
            .build()
        val cb = object : AdvertiseCallback() {
            override fun onStartFailure(errorCode: Int) {
                Log.e(TAG, "advertise failed: $errorCode")
            }
        }
        advertiseCallback = cb
        try {
            adv.startAdvertising(settings, primary, scanResponse, cb)
            Log.i(TAG, "advertising PSM $psm")
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
                val sd = result.scanRecord?.getServiceData(FIPS_PARCEL_UUID)
                val psm = if (sd != null && sd.size >= 2) {
                    (sd[0].toInt() and 0xFF) or ((sd[1].toInt() and 0xFF) shl 8) // 16-bit LE
                } else 0
                NativeCore.bleDeliverScan(bridgeHandle, addr, psm)
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
    }

    /** Tear everything down (called when the service stops). */
    fun shutdown() {
        stopped = true
        stopScanning()
        stopAdvertising()
        runCatching { serverSocket?.close() }
        serverSocket = null
        channels.keys.toList().forEach { closeChannel(it) }
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
        io.execute { readerLoop(chId, sock) }
        io.execute { writerLoop(chId, sock) }
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
        try {
            while (true) {
                val n = NativeCore.bleChannelNextSend(bridgeHandle, chId, buf, SEND_TIMEOUT_MS)
                when {
                    n < 0 -> break // channel closed
                    n == 0 -> continue // timeout, poll again
                    else -> {
                        outs.write((n shr 8) and 0xFF) // 2-byte BE length prefix
                        outs.write(n and 0xFF)
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
        runCatching { NativeCore.bleChannelClosed(bridgeHandle, chId) }
    }

    private fun sendMtu(sock: BluetoothSocket): Int =
        (sock.maxTransmitPacketSize - FRAME_HEADER).coerceAtLeast(20)

    private fun recvMtu(sock: BluetoothSocket): Int =
        sock.maxReceivePacketSize.coerceAtLeast(20)

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

        /** FIPS service UUID — must match fips-core (ble-wire.md). */
        val FIPS_UUID: UUID = UUID.fromString("9c90b790-2cc5-42c0-9f87-c9cc40648f4c")
        val FIPS_PARCEL_UUID = ParcelUuid(FIPS_UUID)
    }
}
