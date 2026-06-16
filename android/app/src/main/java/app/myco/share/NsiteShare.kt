package app.myco.share

import android.graphics.Bitmap
import android.graphics.Color
import android.util.Base64
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import org.json.JSONObject
import java.security.SecureRandom

/**
 * Build + encode the **share-an-nsite** payload: a single QR that carries the
 * nsite id *and* this device's pairing info, so the scanning device can open the
 * nsite and — if not already paired — pair with the sharer in one scan.
 *
 * Payload: `myco://share/<base64url(json)>` where the JSON is
 * `{ v, nsite, npub, name, secret }`:
 * - `nsite`  — the `<host>` label to open (`npub1…` root, or `<pubkeyB36><dTag>`).
 * - `npub`   — the **sharer's device** npub (the mesh/pairing identity).
 * - `name`   — a human label for the sharer's device.
 * - `secret` — a one-time `pairSecret` the scanner echoes back over the Noise
 *   channel to complete the mandatory mutual handshake (the matching/handshake is
 *   the P3 receive side; this is the generate side).
 */
object NsiteShare {
    const val SCHEME = "myco"
    const val SHARE_PREFIX = "myco://share/"

    fun buildShareUri(
        nsiteHost: String,
        deviceNpub: String,
        deviceName: String,
        pairSecret: String,
    ): String {
        val json = JSONObject()
            .put("v", 1)
            .put("nsite", nsiteHost)
            .put("npub", deviceNpub)
            .put("name", deviceName)
            .put("secret", pairSecret)
        val b64 = Base64.encodeToString(
            json.toString().toByteArray(Charsets.UTF_8),
            Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP,
        )
        return "$SHARE_PREFIX$b64"
    }

    /** The decoded contents of a `myco://share/…` QR. */
    data class ShareInfo(
        val nsiteHost: String,
        val npub: String,
        val name: String,
        val secret: String,
    )

    /** Decode a scanned `myco://share/<base64url(json)>` URI, or null if malformed. */
    fun parseShareUri(uri: String): ShareInfo? {
        val b64 = uri.removePrefix(SHARE_PREFIX)
        if (b64 == uri) return null // missing the share prefix
        return runCatching {
            val json = JSONObject(
                String(Base64.decode(b64, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP), Charsets.UTF_8)
            )
            ShareInfo(
                nsiteHost = json.optString("nsite"),
                npub = json.optString("npub"),
                name = json.optString("name"),
                secret = json.optString("secret"),
            ).takeIf { it.nsiteHost.isNotEmpty() }
        }.getOrNull()
    }

    /** A one-time, long random pairing secret (URL-safe base64 of 24 bytes). */
    fun newPairSecret(): String {
        val bytes = ByteArray(24)
        SecureRandom().nextBytes(bytes)
        return Base64.encodeToString(bytes, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
    }

    /** A short, stable-ish device label derived from the npub (placeholder until
     *  the memorable color+name lands in P3 pairing). */
    fun deviceName(ownNpub: String): String =
        "Myco-" + ownNpub.removePrefix("npub1").take(6)

    /** Encode a string as a square black-on-white QR bitmap. */
    fun qrBitmap(content: String, size: Int = 720): Bitmap {
        val hints = mapOf(EncodeHintType.MARGIN to 1)
        val matrix = QRCodeWriter().encode(content, BarcodeFormat.QR_CODE, size, size, hints)
        val bmp = Bitmap.createBitmap(size, size, Bitmap.Config.RGB_565)
        for (x in 0 until size) {
            for (y in 0 until size) {
                bmp.setPixel(x, y, if (matrix.get(x, y)) Color.BLACK else Color.WHITE)
            }
        }
        return bmp
    }
}
