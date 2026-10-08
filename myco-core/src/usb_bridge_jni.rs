//! USB bridge — the JNI side. See [`crate::usb_bridge`].

use std::os::fd::{FromRawFd, OwnedFd};

use jni::objects::JClass;
use jni::sys::{jboolean, jint, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;

/// Kotlin opened an Android Open Accessory and detached its descriptor
/// (`ParcelFileDescriptor.detachFd()`): ownership passes here, and Kotlin must
/// not close it. False when `fd` is not a descriptor, in which case nothing
/// was taken.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_usbAttachAccessory(
    _env: JNIEnv,
    _class: JClass,
    fd: jint,
) -> jboolean {
    if fd < 0 {
        return JNI_FALSE;
    }
    // SAFETY: Kotlin detached this descriptor and hands over sole ownership.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    crate::usb_bridge::attach_accessory(fd);
    JNI_TRUE
}

/// Kotlin opened a USB device this phone is host of (`UsbDeviceConnection`) and
/// detached a duplicate of its descriptor: ownership passes here. The node
/// switches it into accessory mode, or links with it if it already is one.
/// False when `fd` is not a descriptor, in which case nothing was taken.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_usbAttachHostDevice(
    _env: JNIEnv,
    _class: JClass,
    fd: jint,
) -> jboolean {
    if fd < 0 {
        return JNI_FALSE;
    }
    // SAFETY: Kotlin detached this descriptor and hands over sole ownership.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    crate::usb_bridge::attach_host_device(fd);
    JNI_TRUE
}
