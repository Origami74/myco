package app.myco.share

import android.graphics.Bitmap
import android.graphics.Color
import android.util.Base64
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel
import org.json.JSONObject
import java.security.SecureRandom

/**
 * Build + encode the **share-an-nsite** payload: a single QR that carries the
 * nsite id, so the scanning device can open it — from its neighborhoods, then
 * the internet. The sharer's npub still rides along for older builds, but
 * nobody contacts it.
 *
 * Payload: `myco://share/<base64url(json)>` where the JSON is
 * `{ v, nsite, npub, name, secret }`:
 * - `nsite`  — the `<host>` label to open (`npub1…` root, or `<pubkeyB36><dTag>`).
 * - `npub`   — the sharer's device npub. Not used to contact anyone.
 * - `name`   — a human label for the sharer's device.
 * - `secret` — a one-time secret. Pairing is gone, so nothing checks it now;
 *   kept so older builds still read the payload.
 */
object NsiteShare {
    const val SCHEME = "myco"
    const val SHARE_PREFIX = "myco://share/"
    const val PAIR_PREFIX = "myco://pair/"

    /** The decoded contents of a `myco://pair/…` QR from an older Myco: read
     *  only to say it is no longer used. */
    data class PairInfo(val npub: String, val name: String, val secret: String)

    /** Decode a scanned `myco://pair/<base64url(json)>` URI, or null if malformed. */
    fun parsePairUri(uri: String): PairInfo? {
        val b64 = uri.removePrefix(PAIR_PREFIX)
        if (b64 == uri) return null
        return runCatching {
            val json = JSONObject(
                String(Base64.decode(b64, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP), Charsets.UTF_8)
            )
            PairInfo(
                npub = json.optString("npub"),
                name = json.optString("name"),
                secret = json.optString("secret"),
            ).takeIf { it.npub.isNotEmpty() }
        }.getOrNull()
    }

    fun buildShareUri(
        nsiteHost: String,
        deviceNpub: String,
        deviceName: String,
        pairSecret: String,
    ): String = buildShareUri(nsiteHost, null, deviceNpub, deviceName, pairSecret)

    /**
     * A share carrying a **napplet** instead of an nsite.
     *
     * [nappletPointer] is the `naddr` it was added by, so the receiving phone
     * gets the author's relay hints with it — the same reason the Library keeps
     * the pointer. Handing over a reconstructed `<npub>:<dtag>` would make the
     * recipient search relays that may well not hold it.
     */
    fun buildNappletShareUri(
        nappletPointer: String,
        deviceNpub: String,
        deviceName: String,
        pairSecret: String,
    ): String = buildShareUri(null, nappletPointer, deviceNpub, deviceName, pairSecret)

    private fun buildShareUri(
        nsiteHost: String?,
        nappletPointer: String?,
        deviceNpub: String,
        deviceName: String,
        pairSecret: String,
    ): String {
        val json = JSONObject()
            .put("v", 1)
            .put("npub", deviceNpub)
            .put("name", deviceName)
            .put("secret", pairSecret)
        // One of the two, never both — a share is one app.
        if (nsiteHost != null) json.put("nsite", nsiteHost)
        if (nappletPointer != null) json.put("napplet", nappletPointer)
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
        /** Set when the share carries a napplet rather than an nsite. */
        val nappletPointer: String = "",
    ) {
        /** Whether this share is a napplet. */
        val isNapplet: Boolean get() = nappletPointer.isNotEmpty()
    }

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
                nappletPointer = json.optString("napplet"),
            ).takeIf { it.nsiteHost.isNotEmpty() || it.nappletPointer.isNotEmpty() }
        }.getOrNull()
    }

    /** A one-time, long random secret for the share payload (URL-safe base64 of 24 bytes). */
    fun newPairSecret(): String {
        val bytes = ByteArray(24)
        SecureRandom().nextBytes(bytes)
        return Base64.encodeToString(bytes, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
    }

    /** A short, stable-ish device label derived from the npub. */
    fun deviceName(ownNpub: String): String =
        "Myco-" + ownNpub.removePrefix("npub1").take(6)

    /**
     * Encode a string as a square black-on-white QR bitmap.
     *
     * [ecc] defaults to ZXing's own default (L, ~7% redundancy) so the long
     * `myco://share/…` and `myco://pair/…` payloads stay at the lowest version
     * that fits — denser codes scan worse on a phone screen held at arm's
     * length. Callers that draw a badge over the middle of the code must pass
     * [ErrorCorrectionLevel.H] (~30%), which is what makes the obscured modules
     * recoverable.
     */
    fun qrBitmap(
        content: String,
        size: Int = 720,
        ecc: ErrorCorrectionLevel = ErrorCorrectionLevel.L,
    ): Bitmap {
        val hints = mapOf(
            EncodeHintType.MARGIN to 1,
            EncodeHintType.ERROR_CORRECTION to ecc,
        )
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
