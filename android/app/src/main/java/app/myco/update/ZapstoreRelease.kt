package app.myco.update

import app.myco.core.NativeCore

/**
 * Rust owns relay queries, publisher authentication and announcement validation.
 * With a core [handle] it also asks the Circle; [online] allows Zapstore.
 */
internal object ZapstoreRelease {
    fun fetch(handle: Long, online: Boolean): String = NativeCore.fetchSoftwareRelease(handle, online)
}
