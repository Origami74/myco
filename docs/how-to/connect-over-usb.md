# Connect over a USB cable

A USB cable links a phone to a desktop running fips, or two phones to each
other. It is the fastest way to move a large app between phones, and it
works with no network at all. How it works:
[usb-transport.md](../design/fips/usb-transport.md).

---

## Phone to phone

You need a **USB-C to USB-C cable** (or a USB-C OTG adapter). A
USB-A-to-C charging cable cannot join two phones.

1. Open Myco on both phones, with the mesh on.
2. Connect the cable.
3. On the phone that becomes the host, Android asks **"Allow Myco to access
   …?"**. Tap **OK**.
4. Android may then ask **"Open Myco to handle FIPS mesh link?"** on either
   phone. Tick **Always** and tap **OK**. It does not ask again.

Within a few seconds the other phone appears as a peer, and the mesh status
sheet shows a **USB** block marked "linked".

**Nothing happens?** The cable decides which phone is host. If neither
asks anything, pull down the USB notification on one phone and set **USB
controlled by: This device**, or swap the cable's ends.

## Phone to desktop

The desktop runs the fips daemon with USB host support, on Linux or macOS.

1. Build fips with the `usb-host` feature:

   ```sh
   cargo build --release --features usb-host
   ```

2. Turn on the USB host role in `fips.yaml`:

   ```yaml
   transports:
     usb:
       host: true
   ```

3. Restart the daemon and plug the phone in. Its screen can be locked and
   Myco need not be running: the cable starts it.
4. The first time, tap **Always** and **OK** on "Open Myco to handle FIPS
   mesh link?".

`fipsctl show transports` lists a `usb` transport with the link, and
`fipsctl show peers` shows the phone over `usb`.

**Linux:** the daemon needs access to the phone's USB device before and
after it switches. Running as root works; otherwise add a udev rule granting
access to your phone's vendor id and to `18d1:2d00`–`2d05`.

## Unplugging

Pulling the cable drops the link on both ends at once. A peer that is also
reachable over Bluetooth or Wi-Fi stays connected over that; otherwise it
leaves the peer list within seconds.
