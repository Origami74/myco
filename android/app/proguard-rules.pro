# R8 rules for the release build.
#
# Rust reaches into Kotlin over JNI by class and method *name*, which R8 can't
# see. Native methods themselves are kept by the default rules; these keep
# what the Rust side calls back into.

# Shrink, don't rename: stack traces in bug reports (logcat, the crash
# buffer) stay readable without a mapping file. Nearly all of the size win
# is from removing unused code, not from shorter names.
-dontobfuscate

# NativeCore: the JNI entry points (`Java_app_myco_core_NativeCore_*`) bind
# by class and method name.
-keep class app.myco.core.NativeCore { *; }

# BleRadio: myco-core/src/ble_bridge_jni.rs holds a reference to it and calls
# listen, connect, startAdvertising, stopAdvertising, startScanning,
# stopScanning and closeChannel by name and signature.
-keep class app.myco.ble.BleRadio { public *; }
