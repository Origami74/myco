//! Hands USB links to the fips node's USB transport.
//!
//! Two ways a cable reaches the node, one per side of it:
//!
//! - **Accessory.** A USB host (a desktop fips node, or another phone) switched
//!   this phone into accessory mode; Android starts `UsbAccessoryActivity`,
//!   which opens the accessory and passes its descriptor here
//!   ([`attach_accessory`]).
//! - **Host.** This phone holds the cable's host role and another phone is
//!   plugged in; Android starts `UsbDeviceActivity`, which opens the device and
//!   passes its descriptor here ([`attach_host_device`]). The node switches it
//!   into accessory mode; it comes back as a new device and is handed over
//!   again, and that time the node links with it.
//!
//! Either way the node owns the descriptor from then on
//! ([`fips::transport::usb::UsbAttach`]): the USB transport runs the link
//! hello and the node dials the peer it names.
//!
//! A descriptor can arrive before there is a node to give it to. Plugging in
//! is what *starts* the app when it is not running, and the node comes up
//! after that, from the `UsbService` the activity starts. So a descriptor that
//! finds no node is held and handed over by [`install`] when `start_node` arms
//! the next one. Process-global, like the other bridges: the JNI call must not
//! take the reducer lock.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use fips::transport::usb::UsbAttach;

/// A descriptor from Kotlin, waiting for a node.
enum Handed {
    Accessory(OwnedFd),
    HostDevice(OwnedFd),
}

struct Bridge {
    /// The running node's USB handle, once `start_node` has armed one.
    attach: Option<UsbAttach>,
    /// Descriptors opened before any node was armed.
    pending: Vec<Handed>,
}

static BRIDGE: Mutex<Bridge> = Mutex::new(Bridge {
    attach: None,
    pending: Vec::new(),
});

/// Labels for links, unique for the life of the process.
static NEXT_LINK: AtomicU64 = AtomicU64::new(0);

fn lock() -> std::sync::MutexGuard<'static, Bridge> {
    BRIDGE.lock().unwrap_or_else(|e| e.into_inner())
}

fn hand_over(attach: &UsbAttach, handed: Handed) {
    let n = NEXT_LINK.fetch_add(1, Ordering::Relaxed);
    match handed {
        Handed::Accessory(fd) => {
            let label = format!("accessory{n}");
            tracing::info!(link = %label, "USB accessory handed to the node");
            attach.accessory(fd, label);
        }
        Handed::HostDevice(fd) => {
            let label = format!("device{n}");
            tracing::info!(link = %label, "USB device handed to the node");
            attach.host_device(fd, label);
        }
    }
}

/// A freshly started node's USB handle. Descriptors that were waiting for a
/// node go to it now; later ones go straight to it. Replaces the handle of a
/// node that has since been stopped.
pub(crate) fn install(attach: UsbAttach) {
    let mut bridge = lock();
    for handed in std::mem::take(&mut bridge.pending) {
        hand_over(&attach, handed);
    }
    bridge.attach = Some(attach);
}

fn handed(handed: Handed) {
    let mut bridge = lock();
    match &bridge.attach {
        Some(attach) => hand_over(attach, handed),
        None => {
            tracing::info!("USB descriptor arrived before the node; holding it");
            bridge.pending.push(handed);
        }
    }
}

/// An opened accessory, from Kotlin.
pub(crate) fn attach_accessory(fd: OwnedFd) {
    handed(Handed::Accessory(fd));
}

/// A USB device this phone is host of, opened by Kotlin.
pub(crate) fn attach_host_device(fd: OwnedFd) {
    handed(Handed::HostDevice(fd));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// The bridge is process-global, so its tests run as one.
    #[tokio::test]
    async fn descriptors_wait_for_the_node_and_later_ones_go_straight_to_it() {
        // No node yet: held, whichever side of the cable they came from.
        let (early, _far_early) = UnixStream::pair().unwrap();
        attach_accessory(early.into());
        let (device, _far_device) = UnixStream::pair().unwrap();
        attach_host_device(device.into());
        assert_eq!(lock().pending.len(), 2);

        // A `UsbAttach` comes only from a node: arm one and give the bridge
        // its handle, as `start_node` does.
        let mut node = fips::Node::new(fips::Config::new()).unwrap();
        install(node.enable_app_owned_usb());
        assert!(lock().pending.is_empty());

        let (late, _far_late) = UnixStream::pair().unwrap();
        attach_accessory(late.into());
        assert!(lock().pending.is_empty());
    }
}
