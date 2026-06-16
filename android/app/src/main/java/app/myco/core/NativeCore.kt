package app.myco.core

import android.content.Context

/**
 * JNI bindings to `libmyco_core.so`. The contract is a Redux-style reducer:
 * `dispatchJson(actionJson) -> stateJson`, with a monotonic `rev` in the state.
 * See docs/reference/ffi-surface.md.
 */
internal object NativeCore {
    init {
        System.loadLibrary("myco_core")
    }

    external fun initializeAndroidContext(context: Context)
    external fun appNew(dataDir: String, appVersion: String): Long
    external fun appFree(handle: Long)
    external fun stateJson(handle: Long): String
    external fun refreshJson(handle: Long): String
    external fun dispatchJson(handle: Long, actionJson: String): String
}
