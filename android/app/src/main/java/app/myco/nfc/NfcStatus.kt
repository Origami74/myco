package app.myco.nfc

import android.content.Context
import android.content.Intent
import android.nfc.NfcAdapter
import android.provider.Settings

/** Whether this device can do NFC tap-to-pair right now. */
enum class NfcState { ENABLED, DISABLED, UNAVAILABLE }

object NfcStatus {
    fun state(context: Context): NfcState {
        val adapter = NfcAdapter.getDefaultAdapter(context) ?: return NfcState.UNAVAILABLE
        return if (adapter.isEnabled) NfcState.ENABLED else NfcState.DISABLED
    }

    /** Open the system NFC settings so the user can turn it on. */
    fun openSettings(context: Context) {
        runCatching { context.startActivity(Intent(Settings.ACTION_NFC_SETTINGS)) }
            .onFailure { runCatching { context.startActivity(Intent(Settings.ACTION_WIRELESS_SETTINGS)) } }
    }
}
