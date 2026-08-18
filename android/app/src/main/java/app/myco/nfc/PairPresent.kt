package app.myco.nfc

import android.content.Context
import androidx.compose.runtime.mutableStateOf
import app.myco.share.NsiteShare
import app.myco.share.PairSecrets

/**
 * Process-global state for NFC "tap to connect". While **presenting**, this device
 * emulates an NDEF tag ([PairHostApduService] serves [payload]); the counterpart's
 * OS reads it. Presenting is scoped to the Circle tab.
 *
 * **NFC exposure note.** While presenting, the emulated tag carries
 * `{npub, name, pairSecret}` and is readable by *any* NFC reader held to the phone
 * — not just another Myco device. That's acceptable: the npub and chosen name are
 * low-sensitivity (the npub is already public), and the `pairSecret` is single-use
 * (consumed on first accept, then rotated — see [PairSecrets]), so a captured tag
 * can't be replayed to pair. It is deliberately limited to the moments the user is
 * on the Circle tab, never the whole time the app is open. The one exception is
 * the file-share hotspot's page URL ([beginUrl]): it presents for as long as the
 * hotspot runs, which is fine — the URL is only reachable by devices the user
 * just let onto that hotspot, and every transfer is gated on the owner anyway.
 *
 * [onChanged] lets the Activity re-claim/release foreground HCE when presenting
 * flips. [presenting] is backed by Compose snapshot state so UI gates (auto-accept
 * vs. prompt) recompose the instant it changes.
 */
object PairPresent {
    /** The `myco://pair/...` URI currently being emulated, or null when not presenting. */
    val payload = mutableStateOf<String?>(null)

    /** While set, this URI is presented *instead of* the pairing payload — the
     *  file-share hotspot parks its page's `http://…` address here so a bump
     *  hands any phone the URL (the reader's OS opens it in the browser; no
     *  Myco needed on that side). Pairing-by-bump resumes when it clears. */
    private val urlOverride = mutableStateOf<String?>(null)

    private val pairPresenting = mutableStateOf(false)

    /** True while we're emulating a card. Read it from composition to observe flips. */
    val presenting: Boolean get() = pairPresenting.value || urlOverride.value != null

    /** What the emulated tag actually serves right now ([PairHostApduService]). */
    val effective: String? get() = urlOverride.value ?: payload.value

    /** Set by the Activity to react when presenting starts/stops (toggle HCE). */
    @Volatile
    var onChanged: (() -> Unit)? = null

    /** Begin presenting: mint a fresh single-use secret and build the payload. */
    fun begin(context: Context, ownNpub: String, deviceName: String) {
        rotate(context, ownNpub, deviceName)
        pairPresenting.value = true
        onChanged?.invoke()
    }

    /** Begin presenting a prebuilt payload verbatim (e.g. a `myco://share/…` URI
     *  carrying an nsite). Unlike [begin], the caller owns the secret — used by the
     *  share-an-app surface, which mints its own one-time secret in the share URI. */
    fun beginRaw(uri: String) {
        payload.value = uri
        pairPresenting.value = true
        onChanged?.invoke()
    }

    /** Mint a new single-use secret and rebuild the presented payload (called after
     *  a tap is consumed, so the same code can't pair twice). */
    fun rotate(context: Context, ownNpub: String, deviceName: String) {
        payload.value = NsiteShare.buildPairUri(ownNpub, deviceName, PairSecrets.issue(context))
    }

    /** Present `uri` to every bump until [stopUrl] — outlives the Circle tab. */
    fun beginUrl(uri: String) {
        urlOverride.value = uri
        onChanged?.invoke()
    }

    fun stopUrl() {
        urlOverride.value = null
        onChanged?.invoke()
    }

    /** Stop pair-presentment (leaving the Circle tab). A hotspot URL keeps
     *  presenting — its lifetime belongs to the hotspot, not the tab. */
    fun stop() {
        pairPresenting.value = false
        payload.value = null
        onChanged?.invoke()
    }
}
