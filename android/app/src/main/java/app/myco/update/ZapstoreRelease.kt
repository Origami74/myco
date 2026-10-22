package app.myco.update

import app.myco.core.NativeCore

/** Rust owns relay queries, publisher authentication and announcement validation. */
internal object ZapstoreRelease {
    fun fetch(): String = NativeCore.fetchSoftwareRelease()
}
