//! Native TUN pump: moves IPv6 packets between the VpnService's TUN fd and the
//! node without crossing into the JVM per packet.
//!
//! The VpnService used to pump the fd from two Kotlin threads, calling into
//! Rust once per packet (`tunSendPacket` / `tunNextPacket`), each call copying
//! the packet byte by byte between Java and Rust arrays. At ~10k packets/s
//! over a USB link that cost ~55 µs per packet delivered and ~75 µs per packet
//! sent — the writer thread alone was at 60% of a core, a ceiling for every
//! lane, not just USB. Now Kotlin hands over a duplicate of the fd once
//! ([`start`]) and two native threads read and write it directly, through the
//! same [`crate::tun_bridge`] entry points.
//!
//! The fd is non-blocking (`establish()`'s default) and both threads wait on it
//! with `poll()` and a timeout, for the reason the Kotlin pump learned the hard
//! way: a thread blocked in `read()` is not woken by closing the fd, so on a
//! tunnel restart the old reader could hang holding the old tunnel. Here every
//! wait is bounded by [`POLL_MS`], so [`stop`] always lets the threads go; each
//! closes its share of the descriptor as it leaves.

use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Longest any wait goes without looking at the stop flag.
const POLL_MS: i32 = 250;

/// Read buffer. The TUN MTU is at most 1500; this leaves room.
const READ_BUF: usize = 4096;

/// How long a write that finds the TUN queue full waits for room before the
/// packet is dropped, as a full TUN queue drops it anyway.
const WRITE_WAIT_MS: i32 = 50;

/// The running pump's stop flag.
static RUNNING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// Start pumping `fd`, replacing any pump already running. Takes ownership:
/// the descriptor is closed when both threads have stopped.
pub(crate) fn start(fd: OwnedFd) {
    stop();
    let running = Arc::new(AtomicBool::new(true));
    let fd = Arc::new(fd);
    spawn("myco-tun-read", {
        let (fd, running) = (Arc::clone(&fd), Arc::clone(&running));
        move || read_loop(&fd, &running)
    });
    spawn("myco-tun-write", {
        let (fd, running) = (Arc::clone(&fd), Arc::clone(&running));
        move || write_loop(&fd, &running)
    });
    *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) = Some(running);
    tracing::info!("native TUN pump started");
}

/// Stop the running pump, if any. Returns at once; the threads notice within
/// [`POLL_MS`] and close the descriptor as they leave.
pub(crate) fn stop() {
    if let Some(running) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).take() {
        running.store(false, Ordering::Relaxed);
        tracing::info!("native TUN pump stopping");
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        tracing::error!(thread = name, error = %e, "could not start TUN pump thread");
    }
}

enum Ready {
    Yes,
    Timeout,
    /// The fd reported an error or hangup: the tunnel is gone.
    Gone,
}

fn wait(fd: RawFd, events: libc::c_short, timeout_ms: i32) -> Ready {
    let mut pfd = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: one valid pollfd, count 1.
    let n = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if n < 0 {
        return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            Ready::Timeout
        } else {
            Ready::Gone
        };
    }
    if n == 0 {
        return Ready::Timeout;
    }
    if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
        return Ready::Gone;
    }
    Ready::Yes
}

/// TUN → mesh: wait for packets, then read every one that is queued before
/// waiting again.
fn read_loop(fd: &OwnedFd, running: &AtomicBool) {
    let raw = fd.as_raw_fd();
    let mut buf = [0u8; READ_BUF];
    while running.load(Ordering::Relaxed) {
        match wait(raw, libc::POLLIN, POLL_MS) {
            Ready::Timeout => continue,
            Ready::Gone => break,
            Ready::Yes => {}
        }
        loop {
            // SAFETY: reads at most `buf.len()` bytes into `buf`.
            let n = unsafe { libc::read(raw, buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                crate::tun_bridge::send_packet(buf[..n as usize].to_vec());
                continue;
            }
            if n == 0 {
                return;
            }
            match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::WouldBlock => break,
                std::io::ErrorKind::Interrupted => continue,
                _ => return,
            }
        }
    }
}

/// Mesh → TUN: write each packet the node delivers.
fn write_loop(fd: &OwnedFd, running: &AtomicBool) {
    let raw = fd.as_raw_fd();
    while running.load(Ordering::Relaxed) {
        if let Some(packet) = crate::tun_bridge::next_packet(Duration::from_millis(POLL_MS as u64))
        {
            if !write_packet(raw, &packet) {
                return;
            }
        }
    }
}

/// Write one packet. False when the tunnel is gone; a packet the TUN queue has
/// no room for after [`WRITE_WAIT_MS`] is dropped, which is all the kernel
/// would do with it.
fn write_packet(fd: RawFd, packet: &[u8]) -> bool {
    let mut waited = false;
    loop {
        // SAFETY: writes `packet.len()` bytes from `packet`.
        let n = unsafe { libc::write(fd, packet.as_ptr().cast(), packet.len()) };
        if n >= 0 {
            return true;
        }
        match std::io::Error::last_os_error().kind() {
            std::io::ErrorKind::Interrupted => continue,
            std::io::ErrorKind::WouldBlock if !waited => {
                waited = true;
                match wait(fd, libc::POLLOUT, WRITE_WAIT_MS) {
                    Ready::Gone => return false,
                    _ => continue,
                }
            }
            std::io::ErrorKind::WouldBlock => return true,
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixDatagram;

    fn mesh_packet(tag: u8) -> Vec<u8> {
        let mut p = vec![0u8; 60];
        p[0] = 0x60; // IPv6
        p[24] = 0xfd; // dst in fd00::/8
        p[59] = tag;
        p
    }

    /// The pump is process-global and so is the bridge it feeds, so one test
    /// drives both directions, a restart, and a stop.
    #[tokio::test]
    async fn packets_cross_both_ways_and_a_restart_releases_the_old_tunnel() {
        let _serial = crate::tun_bridge::TEST_LOCK.lock().await;
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
        let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        crate::tun_bridge::install(out_tx, in_rx, 1143);

        // A datagram socket pair keeps packet boundaries, like a TUN fd.
        let (ours, theirs) = UnixDatagram::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        start(OwnedFd::from(ours));

        // TUN → mesh, several queued at once.
        for tag in 1..=3 {
            theirs.send(&mesh_packet(tag)).unwrap();
        }
        for tag in 1..=3 {
            let got = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(got[59], tag);
        }

        // Mesh → TUN.
        in_tx.send(mesh_packet(9)).unwrap();
        theirs
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut buf = [0u8; 128];
        let n = theirs.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], &mesh_packet(9)[..]);

        // A restart: the new tunnel carries traffic, and the old pump lets its
        // descriptor go (its far end sees the peer gone).
        let (ours2, theirs2) = UnixDatagram::pair().unwrap();
        ours2.set_nonblocking(true).unwrap();
        start(OwnedFd::from(ours2));
        theirs2.send(&mesh_packet(5)).unwrap();
        let got = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got[59], 5);
        std::thread::sleep(Duration::from_millis(3 * POLL_MS as u64));
        assert!(
            theirs.send(&mesh_packet(1)).is_err(),
            "the old pump should have closed its descriptor"
        );

        stop();
    }
}
