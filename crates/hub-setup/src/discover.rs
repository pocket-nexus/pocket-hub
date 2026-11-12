//! Finding a console that runs ftpd: a bounded scan of the local IPv4
//! subnets for an open TCP port 5000 that answers with an FTP `220` banner.
//!
//! The scan covers at most the /24 around each interface address (a larger
//! subnet is cut to the /24 its address is in), runs the probes
//! concurrently and ends at a deadline (2 s by default) with what answered
//! by then. A probe that connects reads one line, sends `QUIT` and closes:
//! ftpd on the 3DS serves only a few sockets at a time, so no probe stays.
//! A manual address is always possible; this only offers candidates.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout, timeout_at};

use crate::ftp::FTPD_PORT;

#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    pub port: u16,
    /// The addresses to probe; None scans the local subnets
    /// (`scan_targets`).
    pub targets: Option<Vec<Ipv4Addr>>,
    /// The whole scan ends here.
    pub deadline: Duration,
    pub connect_timeout: Duration,
    /// Probes in flight at once. Each holds a socket; macOS gives a process
    /// 256 descriptors by default.
    pub concurrency: usize,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self { port: FTPD_PORT, targets: None, deadline: Duration::from_secs(2), connect_timeout: Duration::from_millis(700), concurrency: 128 }
    }
}

/// A host that answered like an FTP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub address: Ipv4Addr,
    pub port: u16,
    /// The banner line without its code, e.g. `Hello!`.
    pub banner: String,
    /// The banner is ftpd's (`220 Hello!`).
    pub ftpd: bool,
}

/// The hosts of the subnet of `address`/`prefix`, at most the /24 that holds
/// `address`, without the network and broadcast addresses and `address`
/// itself. A /31 or /32 (a point-to-point link) has none to scan.
pub fn subnet_hosts(address: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    if prefix >= 31 || address.is_loopback() || address.is_link_local() {
        return Vec::new();
    }
    let prefix = prefix.max(24) as u32;
    let mask = u32::MAX << (32 - prefix);
    let network = u32::from(address) & mask;
    let broadcast = network | !mask;
    ((network + 1)..broadcast).map(Ipv4Addr::from).filter(|host| *host != address).collect()
}

/// Every host of the local subnets (`subnet_hosts` of each IPv4 interface),
/// the default route's interface first.
pub fn scan_targets() -> Vec<Ipv4Addr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else { return Vec::new() };
    let order = crate::card::interface_addresses();
    let mut subnets: Vec<(Ipv4Addr, u8)> = interfaces
        .into_iter()
        .filter_map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(v4) => Some((v4.ip, v4.prefixlen)),
            _ => None,
        })
        .collect();
    subnets.sort_by_key(|(ip, _)| order.iter().position(|address| address == ip).unwrap_or(usize::MAX));
    let mut seen = BTreeSet::new();
    let mut hosts = Vec::new();
    for (ip, prefix) in subnets {
        for host in subnet_hosts(ip, prefix) {
            if seen.insert(host) {
                hosts.push(host);
            }
        }
    }
    hosts
}

async fn probe(address: Ipv4Addr, port: u16, connect_timeout: Duration, until: Instant) -> Option<Candidate> {
    let stream = timeout(connect_timeout, TcpStream::connect(SocketAddr::from((address, port)))).await.ok()?.ok()?;
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    timeout_at(until, (&mut reader).take(512).read_until(b'\n', &mut line)).await.ok()?.ok()?;
    let line = String::from_utf8_lossy(&line);
    let line = line.trim_end_matches(['\r', '\n']);
    let banner = line.strip_prefix("220")?;
    if !(banner.is_empty() || banner.starts_with([' ', '-'])) {
        return None;
    }
    let _ = timeout(Duration::from_millis(100), write.write_all(b"QUIT\r\n")).await;
    let banner = banner.trim_start_matches([' ', '-']).to_owned();
    Some(Candidate { address, port, ftpd: banner == "Hello!", banner })
}

/// Probes the targets and returns the hosts that answered with a `220`, in
/// address order.
pub async fn discover(options: &DiscoverOptions) -> Vec<Candidate> {
    let targets = options.targets.clone().unwrap_or_else(scan_targets);
    let until = Instant::now() + options.deadline;
    let permits = Arc::new(Semaphore::new(options.concurrency.max(1)));
    let mut probes = JoinSet::new();
    for address in targets {
        let permits = permits.clone();
        let (port, connect_timeout) = (options.port, options.connect_timeout);
        probes.spawn(async move {
            let _permit = permits.acquire_owned().await.ok()?;
            probe(address, port, connect_timeout, until).await
        });
    }
    let mut found = Vec::new();
    while let Ok(Some(done)) = timeout_at(until, probes.join_next()).await {
        if let Ok(Some(candidate)) = done {
            found.push(candidate);
        }
    }
    probes.abort_all();
    found.sort_by_key(|candidate| candidate.address);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnets_are_bounded_to_a_24() {
        let hosts = subnet_hosts(Ipv4Addr::new(192, 168, 1, 20), 24);
        assert_eq!(hosts.len(), 253);
        assert_eq!(hosts.first(), Some(&Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(hosts.last(), Some(&Ipv4Addr::new(192, 168, 1, 254)));
        assert!(!hosts.contains(&Ipv4Addr::new(192, 168, 1, 20)));
        let wide = subnet_hosts(Ipv4Addr::new(10, 1, 2, 3), 8);
        assert_eq!(wide.len(), 253);
        assert!(wide.iter().all(|host| host.octets()[..3] == [10, 1, 2]));
        assert_eq!(subnet_hosts(Ipv4Addr::new(10, 0, 0, 5), 30), vec![Ipv4Addr::new(10, 0, 0, 6)]);
        assert!(subnet_hosts(Ipv4Addr::new(10, 0, 0, 5), 32).is_empty());
        assert!(subnet_hosts(Ipv4Addr::LOCALHOST, 8).is_empty());
    }
}
