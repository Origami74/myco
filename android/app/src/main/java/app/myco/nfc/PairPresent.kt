package app.myco.nfc

import android.content.Context
import androidx.compose.runtime.mutableStateOf
import app.myco.share.NsiteShare
import app.myco.share.PairSecrets

/**
 * Process-global state for NFC "tap to connect". Exactly one of two roles is live
 * at a time (HCE can't read and emulate at once):
 *
 *  - **presenting** — the QR / "show yours" screen is open: this device emulates a
 *    tag ([PairHostApduService] serves [payload]) and the NFC *reader* is off.
 *  - **readerWanted** — the pairing home is open: the device reads a presenter's
 *    tag and routes it into the normal pair flow.
 *
 * [onChanged] lets the Activity re-evaluate reader-mode whenever these flip.
 */
object PairPresent {
    /** The `myco://pair/...` URI currently being emulated, or null when not presenting. */
    val payload = mutableStateOf<String?>(null)

    @Volatile
    var presenting = false
        private set

    /** Set by the Activity to react when presenting starts/stops (toggle HCE). */
    @Volatile
    var onChanged: (() -> Unit)? = null

    /** Begin presenting: mint a fresh single-use secret and build the payload. */
    fun begin(context: Context, ownNpub: String, deviceName: String) {
        rotate(context, ownNpub, deviceName)
        presenting = true
        onChanged?.invoke()
    }

    /** Mint a new single-use secret and rebuild the presented payload (called after
     *  a tap is consumed, so the same code can't pair twice). */
    fun rotate(context: Context, ownNpub: String, deviceName: String) {
        payload.value = NsiteShare.buildPairUri(ownNpub, deviceName, PairSecrets.issue(context))
    }

    fun stop() {
        presenting = false
        payload.value = null
        onChanged?.invoke()
    }
}
