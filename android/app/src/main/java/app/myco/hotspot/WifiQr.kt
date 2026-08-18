package app.myco.hotspot

/**
 * The `WIFI:` QR payload (the ZXing convention every stock camera app
 * understands): scanning one offers to join the network directly, no typing.
 * A local-only hotspot is always WPA2-PSK, so `T:WPA` is fixed.
 *
 * Plain Kotlin (no Android types) so the escaping is host-unit-testable.
 */
object WifiQr {
    /** Backslash-escape the characters the WIFI: format reserves. */
    fun escape(value: String): String = buildString {
        for (c in value) {
            if (c in "\\;,\":") append('\\')
            append(c)
        }
    }

    fun payload(ssid: String, passphrase: String): String =
        "WIFI:T:WPA;S:${escape(ssid)};P:${escape(passphrase)};;"
}
