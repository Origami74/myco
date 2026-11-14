package app.myco.nfc

import androidx.compose.runtime.mutableStateOf

/**
 * Process-global state for NFC "tap to connect". While **presenting**, this device
 * emulates an NDEF tag ([PairHostApduService] serves [payload]); the counterpart's
 * OS reads it. Presenting is scoped to the open invite or share sheet.
 *
 * **NFC exposure note.** While presenting, the emulated tag is readable by *any*
 * NFC reader held to the phone — not just another Myco device. It is deliberately
 * limited to the moments an invite or share sheet is open, never the whole time
 * the app is open. The one exception is
 * the file-share hotspot's page URL ([beginUrl]): it presents for as long as the
 * hotspot runs, which is fine — the URL is only reachable by devices the user
 * just let onto that hotspot, and every transfer is gated on the owner anyway.
 *
 * [onChanged] lets the Activity re-claim/release foreground HCE when presenting
 * flips. [presenting] is backed by Compose snapshot state so UI gates (auto-accept
 * vs. prompt) recompose the instant it changes.
 */
object PairPresent {
    /** The `myco://…` URI currently being emulated, or null when not presenting. */
    val payload = mutableStateOf<String?>(null)

    /** While the file-share hotspot runs, it owns the NFC surface outright:
     *  the tag serves the page's `http://…` address (the reader's OS opens it
     *  in the browser; no Myco needed on that side) — and until that address
     *  is known it serves *nothing*. */
    private val hotspotActiveState = mutableStateOf(false)
    private val urlOverride = mutableStateOf<String?>(null)

    private val pairPresenting = mutableStateOf(false)

    /** True while the hotspot owns the NFC surface. */
    val hotspotActive: Boolean get() = hotspotActiveState.value

    /** True while we're emulating a card. Read it from composition to observe flips. */
    val presenting: Boolean get() = pairPresenting.value || hotspotActiveState.value

    /** What the emulated tag actually serves right now ([PairHostApduService]). */
    val effective: String?
        get() = if (hotspotActiveState.value) urlOverride.value else payload.value

    /** Set by the Activity to react when presenting starts/stops (toggle HCE). */
    @Volatile
    var onChanged: (() -> Unit)? = null

    /** Begin presenting a prebuilt payload verbatim (e.g. a `myco://share/…` URI
     *  carrying an nsite, or a `myco://link/…` invitation). */
    fun beginRaw(uri: String) {
        payload.value = uri
        pairPresenting.value = true
        onChanged?.invoke()
    }

    /** The hotspot is starting: take the NFC surface now, before the page URL
     *  is even known — a bump in that window must hand over nothing. */
    fun beginHotspot() {
        hotspotActiveState.value = true
        onChanged?.invoke()
    }

    /** Present `uri` to every bump until [stopUrl] — outlives any sheet. */
    fun beginUrl(uri: String) {
        hotspotActiveState.value = true
        urlOverride.value = uri
        onChanged?.invoke()
    }

    /** The hotspot stopped: hand the NFC surface back. */
    fun stopUrl() {
        hotspotActiveState.value = false
        urlOverride.value = null
        onChanged?.invoke()
    }

    /** Stop presenting (the sheet closed). A hotspot URL keeps presenting —
     *  its lifetime belongs to the hotspot, not the sheet. */
    fun stop() {
        pairPresenting.value = false
        payload.value = null
        onChanged?.invoke()
    }
}
