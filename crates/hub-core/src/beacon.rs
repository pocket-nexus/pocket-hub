//! Hub's beacons on the TCP carrier (docs/POCKET-LINK.md §2.2): one 64-byte
//! datagram a second to UDP 8627.
//!
//! Targets: `255.255.255.255`, the broadcast address of each LAN interface
//! (the list read again every 10 s), and unicast to the last address of every
//! paired device. macOS sends a `255.255.255.255` datagram out of the default
//! route alone, so the per-interface and unicast copies reach a device on a
//! second network.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use pocket_link::beacon::{self, Beacon};
use pocket_link::consts::{BEACON_INTERVAL_MS, BEACON_PORT, BROADCAST_REFRESH_MS};
use tokio::net::UdpSocket;
use tokio::time::{Instant, interval};

use pocket_hub_store::Store;

/// The IPv4 addresses of this computer's LAN interfaces, the interface of the
/// default route first as far as the system orders them.
pub fn interface_addresses() -> Vec<Ipv4Addr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else { return Vec::new() };
    interfaces
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .filter_map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(v4) if !v4.ip.is_link_local() => Some(v4.ip),
            _ => None,
        })
        .collect()
}

fn broadcast_addresses() -> Vec<Ipv4Addr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else { return Vec::new() };
    interfaces
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .filter_map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(v4) => v4.broadcast,
            _ => None,
        })
        .collect()
}

/// Sends beacons until dropped. `port` is the link port they name; `to` is
/// the UDP port devices listen on (8627 outside tests).
pub async fn run(store: Arc<Store>, port: u16, to: Option<u16>) {
    let to = to.unwrap_or(BEACON_PORT);
    let socket = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
        Ok(socket) => socket,
        Err(error) => {
            tracing::warn!("beacons are off: {error}");
            return;
        }
    };
    if let Err(error) = socket.set_broadcast(true) {
        tracing::warn!("beacons reach paired addresses alone: {error}");
    }
    let mut every = interval(Duration::from_millis(BEACON_INTERVAL_MS));
    let mut broadcasts = Vec::new();
    let mut listed: Option<Instant> = None;
    loop {
        every.tick().await;
        if listed.is_none_or(|at| at.elapsed() >= Duration::from_millis(BROADCAST_REFRESH_MS)) {
            broadcasts = broadcast_addresses();
            listed = Some(Instant::now());
        }
        let Some(datagram) = beacon::encode(&Beacon { port, companion: store.hub_id(), name: store.hub_name() }) else { continue };
        let mut targets: BTreeSet<Ipv4Addr> = broadcasts.iter().copied().collect();
        targets.insert(Ipv4Addr::BROADCAST);
        for record in store.devices() {
            if let Some(IpAddr::V4(address)) = record.last_address.as_deref().and_then(|text| text.parse().ok()) {
                targets.insert(address);
            }
        }
        for target in targets {
            // An interface that went away fails its send; the next list drops it.
            let _ = socket.send_to(&datagram, SocketAddr::new(IpAddr::V4(target), to)).await;
        }
    }
}
