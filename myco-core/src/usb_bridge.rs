//! Hands Android Open Accessory links to the fips node's USB transport.
//!
//! When a USB host (a desktop fips node, or another phone) switches this phone
//! into accessory mode, Android starts `UsbAccessoryActivity`, which opens the
//! accessory and passes its descriptor here. The node owns it from then on
//! ([`fips::transport::usb::UsbAttach::accessory`]): the USB transport runs
//! the link hello and the node dials the peer it names.
//!
//! The descriptor can arrive before there is a node to give it to. Plugging
//! in is what *starts* the app when it is not running, and the node comes up
//! after that, from the `UsbService` the activity starts. So a descriptor that
//! finds no node is held and handed over by [`install`] when `start_node`
//! arms the next one. Process-global, like the other bridges: the JNI call
//! must not take the reducer lock.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use fips::transport::usb::UsbAttach;

struct Bridge {
    /// The running node's USB handle, once `start_node` has armed one.
    attach: Option<UsbAttach>,
    /// Accessories opened before any node was armed.
    pending: Vec<OwnedFd>,
}

static BRIDGE: Mutex<Bridge> = Mutex::new(Bridge {
    attach: None,
    pending: Vec::new(),
});

/// Labels for accessory links, unique for the life of the process.
static NEXT_LINK: AtomicU64 = AtomicU64::new(0);

fn lock() -> std::sync::MutexGuard<'static, Bridge> {
    BRIDGE.lock().unwrap_or_else(|e| e.into_inner())
}

fn hand_over(attach: &UsbAttach, fd: OwnedFd) {
    let label = format!("accessory{}", NEXT_LINK.fetch_add(1, Ordering::Relaxed));
    tracing::info!(link = %label, "USB accessory handed to the node");
    attach.accessory(fd, label);
}

/// A freshly started node's USB handle. Accessories that were waiting for a
/// node go to it now; later ones go straight to it. Replaces the handle of a
/// node that has since been stopped.
pub(crate) fn install(attach: UsbAttach) {
    let mut bridge = lock();
    for fd in std::mem::take(&mut bridge.pending) {
        hand_over(&attach, fd);
    }
    bridge.attach = Some(attach);
}

/// An opened accessory, from Kotlin.
pub(crate) fn attach_accessory(fd: OwnedFd) {
    let mut bridge = lock();
    match &bridge.attach {
        Some(attach) => hand_over(attach, fd),
        None => {
            tracing::info!("USB accessory opened before the node; holding it");
            bridge.pending.push(fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// The bridge is process-global, so its tests run as one.
    #[tokio::test]
    async fn an_accessory_waits_for_the_node_and_later_ones_go_straight_to_it() {
        // No node yet: held.
        let (early, _far_early) = UnixStream::pair().unwrap();
        attach_accessory(early.into());
        assert_eq!(lock().pending.len(), 1);

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
