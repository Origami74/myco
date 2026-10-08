# The USB Lane (Android Open Accessory)

> Status: built. fips `transport::usb` (`feat/usb-transport`) carries the
> link; Myco's `app.myco.usb` package hands it the cable. Phone ↔ desktop
> and phone ↔ phone, checked on a Pixel 7 Pro, a Galaxy A52s and macOS.

Two FIPS nodes joined by one USB cable form a link. It is the fastest lane
Myco has: over 100 Mbps through the mesh from a Pixel to a Mac (127 Mbps
measured), against tens of kB/s over Bluetooth. Nothing needs setting up beyond one "Open
Myco?" prompt the first time.

## Why Android Open Accessory

The cable has to work between a phone and a desktop, and between two
phones, without root or developer settings.

- **USB tethering** gives the desktop an IP interface, but macOS has no
  driver for the RNDIS mode most phones tether in, and switching a phone to
  NCM needs adb. It never connects two phones.
- **Android Open Accessory (AOA)** is driven entirely from the USB host
  with vendor control requests. The other phone re-appears as an accessory
  (`18d1:2d00`–`2d05`) with a bulk endpoint each way. Any app can open it,
  and nothing on either side needs a driver.

So the lane is AOA. Which end is host is decided by the cable: a desktop
always is; between two phones, USB-C role negotiation picks one.

## The link

A link is a reliable, ordered byte stream between exactly two nodes, with
no address either side could dial. Plugging in is the discovery.

1. **Hello.** Each side sends `FUSB`, a version byte and its x-only key
   (37 bytes). Noise IK needs the responder's key before msg1, and a cable
   advertises nothing. A stale or foreign stream fails the hello instead of
   being misframed.
2. **Peer.** A completed hello publishes the other node to fips's discovery
   path, which dials it over the link, or adds the link as a path to a peer
   that already has a session over Bluetooth or Wi-Fi.
3. **Frames.** After the hello the stream is plain FMP. Each side's writer
   joins whatever packets queued up behind the first into one transfer of up
   to 16 KiB: a transfer costs a few hundred microseconds whatever its size,
   so batching, not packet size, sets the throughput.
4. **Unplug.** The accessory read returns `EIO` and the host sees the device
   go, both within a millisecond. The transport reports the link closed and
   fips drops the peer, or moves its traffic to another path, on the next
   fast tick rather than after the 30 s link-dead timeout.

## Roles in Myco

**Accessory** (a desktop or another phone switched this phone). Android
starts `UsbAccessoryActivity` for an accessory announcing
`FIPS`/`fips-link` (`res/xml/accessory_filter.xml`). It opens the accessory,
hands the descriptor to the core, and starts `UsbService` to keep the
process — and the mesh — up while the cable is in. This works with Myco not
running and the screen locked: the attach starts the app.

**Host** (this phone holds the host role of a USB-C cable to another phone).
The core gets the device's descriptor and fips asks it to switch into
accessory mode. It comes back as an accessory device and is handed over
again, and that time fips links with it. Two routes deliver the device:

- `UsbDeviceActivity`, through `res/xml/usb_device_filter.xml`.
- `UsbHostWatcher`, a runtime receiver for `USB_DEVICE_ATTACHED`. Stock
  Android (a Pixel) does not offer a device exposing MTP — which every phone
  does — to any app unless one is already its default; it shows its own
  "USB device connected" notification instead. The broadcast still reaches
  runtime receivers, so a running Myco asks for permission and hands the
  device over itself. `UsbHandoff` keeps the two routes from handing over
  one attachment twice.

The phone in the host role therefore needs Myco running. With the mesh on,
it is.

## Desktop

The fips daemon is the host, with the `usb-host` feature and
`transports.usb.host: true`. It watches the bus, asks only devices that look
like a phone (an MTP, PTP or adb interface) to switch, and leaves a device
alone until it is next plugged in if it does not answer. See
[connect-over-usb.md](../../how-to/connect-over-usb.md).

## Limits

- The host phone needs Myco running; the accessory phone does not.
- A desktop host is Linux or macOS. Windows has no device-level control
  transfers in `nusb` and would need a WinUSB driver.
- One peer per cable. Hubs with several phones are untested.
