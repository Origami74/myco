package app.myco.ui

import android.content.Context
import app.myco.ble.BleRadio
import app.myco.core.AppCoreClient
import app.myco.core.NativeActions
import app.myco.share.DeviceName

/**
 * Change this device's name, everywhere it is published.
 *
 * A name lives in three places and all three have to move together:
 *
 *  1. the local preference, which every screen reads back;
 *  2. the core, which stamps it into outgoing pair events;
 *  3. the BLE radio, which broadcasts it in the scan response so devices that
 *     have never paired with us can still show it in their Nearby list.
 *
 * This exists because the third one kept being forgotten. The rename sites each
 * did the first two and the radio only picked the change up on the next
 * `onResume` — so renaming and staying in the app left the old name going out
 * over the air, which is precisely the surface a rename is usually aimed at.
 *
 * Passing a blank name clears the override, and the name falls back to the
 * phone's own or the generated one; the resolved value is what gets published
 * and is returned.
 */
fun applyDeviceName(
    context: Context,
    client: AppCoreClient,
    ownNpub: String,
    name: String,
): String {
    DeviceName.set(context, name)
    val resolved = DeviceName.current(context, ownNpub)
    client.dispatch(NativeActions.setDeviceName(resolved))
    BleRadio.localName = resolved
    // Harmless to re-assert, and it is what the advertised name is keyed on —
    // a radio that came up before the identity was ready has none yet.
    BleRadio.localNodeAddrHex = client.state().nodeAddrHex
    return resolved
}

/**
 * Publish the generated name to the core and the BLE radio, without storing
 * it, until the user has answered the setup popup's name step.
 *
 * The radios come up during the popup's mesh steps, before its last step
 * asks for the name. [applyDeviceName] with no override set would publish the
 * phone's own name (often the owner's, e.g. "A52 of Alex") to everyone in
 * range before they chose to share it. The pseudonym goes out instead; the
 * name step still offers the phone's name, and nothing is stored here, so
 * what the user picks is what [applyDeviceName] publishes next.
 */
fun publishProvisionalName(client: AppCoreClient, ownNpub: String): String {
    val name = DeviceName.generated(ownNpub)
    client.dispatch(NativeActions.setDeviceName(name))
    BleRadio.localName = name
    BleRadio.localNodeAddrHex = client.state().nodeAddrHex
    return name
}
