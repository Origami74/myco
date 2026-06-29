package app.myco.nfc

import android.nfc.cardemulation.HostApduService
import android.os.Bundle
import java.nio.charset.StandardCharsets

/**
 * Read-only **NDEF Type 4 tag** emulation (modeled on numo's working HCE): while
 * the QR/present screen is open we emulate a standard NDEF tag whose single Text
 * record is the `myco://pair/...` URI. Using the standard NDEF AID
 * (`D2760000850101`) — not a custom AID — means any NFC reader, and Android's own
 * `Ndef` class, can read it (which is why the custom-AID approach didn't work).
 */
class PairHostApduService : HostApduService() {
    // The file the reader last SELECTed (CC or NDEF), returned by READ BINARY.
    private var selectedFile: ByteArray? = null

    override fun processCommandApdu(commandApdu: ByteArray?, extras: Bundle?): ByteArray {
        val cmd = commandApdu ?: return SW_ERROR

        // 1) SELECT by AID (the NDEF application).
        if (startsWith(cmd, SELECT_AID_HEADER) && cmd.size >= 5 + NDEF_AID.size &&
            cmd.copyOfRange(5, 5 + NDEF_AID.size).contentEquals(NDEF_AID)
        ) {
            selectedFile = null
            android.util.Log.i("MycoNfc", "HCE select AID")
            return SW_OK
        }

        // 2) SELECT a file by id (CC = E103, NDEF = E104).
        if (startsWith(cmd, SELECT_FILE_HEADER) && cmd.size >= 7) {
            val fileId = cmd.copyOfRange(5, 7)
            return when {
                fileId.contentEquals(CC_FILE_ID) -> {
                    selectedFile = CC_FILE
                    android.util.Log.i("MycoNfc", "HCE select CC")
                    SW_OK
                }
                fileId.contentEquals(NDEF_FILE_ID) -> {
                    val uri = PairPresent.payload.value
                    if (uri.isNullOrEmpty()) {
                        android.util.Log.i("MycoNfc", "HCE select NDEF but no payload")
                        SW_ERROR
                    } else {
                        selectedFile = buildNdefFile(uri)
                        android.util.Log.i("MycoNfc", "HCE select NDEF (${uri.length} chars)")
                        SW_OK
                    }
                }
                else -> SW_ERROR
            }
        }

        // 3) READ BINARY: return a slice of the selected file + status.
        if (cmd.size >= 5 && cmd[0] == 0x00.toByte() && cmd[1] == 0xB0.toByte()) {
            val file = selectedFile ?: return SW_ERROR
            val offset = ((cmd[2].toInt() and 0xFF) shl 8) or (cmd[3].toInt() and 0xFF)
            var len = cmd[4].toInt() and 0xFF
            if (len == 0) len = 256
            if (offset >= file.size) return SW_ERROR
            val end = minOf(offset + len, file.size)
            return file.copyOfRange(offset, end) + SW_OK
        }

        return SW_ERROR
    }

    override fun onDeactivated(reason: Int) {
        selectedFile = null
    }

    private companion object {
        val SW_OK = byteArrayOf(0x90.toByte(), 0x00)
        val SW_ERROR = byteArrayOf(0x6A.toByte(), 0x82.toByte())

        // Standard NDEF Type 4 application AID.
        val NDEF_AID = byteArrayOf(0xD2.toByte(), 0x76, 0x00, 0x00, 0x85.toByte(), 0x01, 0x01)
        val SELECT_AID_HEADER = byteArrayOf(0x00, 0xA4.toByte(), 0x04, 0x00)
        val SELECT_FILE_HEADER = byteArrayOf(0x00, 0xA4.toByte(), 0x00, 0x0C)
        val CC_FILE_ID = byteArrayOf(0xE1.toByte(), 0x03)
        val NDEF_FILE_ID = byteArrayOf(0xE1.toByte(), 0x04)

        // Capability Container: NDEF file is E104, max size 0x70FF, read-only-ish.
        val CC_FILE = byteArrayOf(
            0x00, 0x0F, // CCLEN
            0x20, // mapping version 2.0
            0x00, 0x3B, // MLe (max read)
            0x00, 0x34, // MLc (max write)
            0x04, 0x06, // NDEF File Control TLV: T=04, L=06
            0xE1.toByte(), 0x04, // NDEF file id
            0x70, 0xFF.toByte(), // NDEF max size
            0x00, // read access (open)
            0xFF.toByte(), // write access (none — read only)
        )

        fun startsWith(cmd: ByteArray, header: ByteArray): Boolean {
            if (cmd.size < header.size) return false
            for (i in header.indices) if (cmd[i] != header[i]) return false
            return true
        }

        /** Build the NDEF file: [2-byte NLEN][NDEF **URI** record carrying `uri`].
         *  A URI record (not Text) so the reader phone's OS tag dispatch turns it
         *  into a VIEW/NDEF_DISCOVERED intent matched by our `myco://` scheme. */
        fun buildNdefFile(uri: String): ByteArray {
            val uriBytes = uri.toByteArray(StandardCharsets.UTF_8)
            val payload = ByteArray(1 + uriBytes.size)
            payload[0] = 0x00 // URI identifier code: no abbreviation
            System.arraycopy(uriBytes, 0, payload, 1, uriBytes.size)

            val type = byteArrayOf(0x55) // 'U'
            val shortRecord = payload.size <= 255
            val header = if (shortRecord) byteArrayOf(0xD1.toByte()) else byteArrayOf(0xC1.toByte())
            val lenField = if (shortRecord) {
                byteArrayOf(payload.size.toByte())
            } else {
                val p = payload.size
                byteArrayOf((p ushr 24).toByte(), (p ushr 16).toByte(), (p ushr 8).toByte(), p.toByte())
            }
            val record = header + byteArrayOf(type.size.toByte()) + lenField + type + payload
            val nlen = record.size
            return byteArrayOf((nlen ushr 8).toByte(), nlen.toByte()) + record
        }
    }
}
