package app.myco.nfc

import androidx.compose.runtime.mutableStateOf

/**
 * Process-global flag for foreground NFC **reading**. While [active], the Activity
 * claims NFC reader mode and reads a tapped peer's emulated tag directly.
 *
 * Needed because the share/add surfaces are [androidx.compose.material3.ModalBottomSheet]s
 * shown in their own focused window: there the OS's passive `NDEF_DISCOVERED` intent
 * dispatch (what the Circle full-screen relies on) no longer reaches the Activity,
 * so a tap goes unnoticed. Reader mode is foreground-scoped and works regardless of
 * which window has focus.
 *
 * Scoped to the add-app sheet (the only place we read while a sheet covers us);
 * [onChanged] lets the Activity enable/disable reader mode as it flips.
 */
object NfcReader {
    private val activeState = mutableStateOf(false)

    /** True while we want foreground reader mode. */
    val active: Boolean get() = activeState.value

    /** Set by the Activity to react when reading starts/stops. */
    @Volatile
    var onChanged: (() -> Unit)? = null

    fun begin() {
        activeState.value = true
        onChanged?.invoke()
    }

    fun stop() {
        activeState.value = false
        onChanged?.invoke()
    }
}
