//! **Hosting a FIPS node inside an app.**
//!
//! Everything an app needs to run on the FIPS mesh that is not the app
//! itself, kept free of any one app's types so it can be lifted out into its
//! own library. Today it holds one piece:
//!
//! - [`netstack`] — a userspace TCP/IP stack over the node's app-owned TUN
//!   channels, so the app's own servers and clients reach the mesh with no
//!   `VpnService` and no system TUN.
//!
//! Where it is headed, so new FIPS plumbing lands here rather than in the app:
//! the node's lifecycle, the platform transports (BLE, Wi-Fi Aware, hotspot,
//! USB), finding a FIPS node another app on the device already runs and using
//! it instead of starting one, and the system-wide TUN that lets other apps
//! use the mesh.

pub mod netstack;
