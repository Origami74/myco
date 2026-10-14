//! JNI side of the Android `VpnService`'s TUN: the service hands the native
//! pump ([`crate::tun_pump`]) a duplicate of the TUN fd once, and the pump
//! moves every packet without crossing into the JVM again.
//!
//! Android-only; verified by the cargo-ndk build, not host `cargo test`.

use std::os::fd::{FromRawFd, OwnedFd};

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;

/// Kotlin → Rust: pump the TUN `fd` (from `ParcelFileDescriptor.detachFd()` on
/// a `dup()` of the tunnel's descriptor). Ownership passes here; any pump
/// already running is stopped first. False when `fd` is not a descriptor.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_tunStartPump(
    _env: JNIEnv,
    _class: JClass,
    fd: jint,
) -> jboolean {
    if fd < 0 {
        return JNI_FALSE;
    }
    // SAFETY: Kotlin detached this descriptor and hands over sole ownership.
    crate::tun_pump::start(unsafe { OwnedFd::from_raw_fd(fd) });
    JNI_TRUE
}

/// Kotlin → Rust: stop the TUN pump. Returns at once; the pump closes its
/// descriptor within a quarter second.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_tunStopPump(_env: JNIEnv, _class: JClass) {
    crate::tun_pump::stop();
}

/// Kotlin → Rust: turn the SOCKS exit on (`"host:port"` of a SOCKS5 proxy on a
/// mesh exit node) or off (`""`). See [`crate::socks_exit`].
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_setSocksExit(
    mut env: JNIEnv,
    _class: JClass,
    proxy: JString,
) {
    let proxy: Option<String> = env.get_string(&proxy).ok().map(Into::into);
    crate::socks_exit::set_proxy(proxy);
}
