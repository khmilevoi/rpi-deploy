use std::net::{IpAddr, UdpSocket};

use pi_domain::contracts::HostNetwork;

pub struct UdpHostNetwork;

impl Default for UdpHostNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl UdpHostNetwork {
    pub fn new() -> UdpHostNetwork {
        UdpHostNetwork
    }
}

impl HostNetwork for UdpHostNetwork {
    /// Best-effort detection for display only. Opens a UDP socket to a public
    /// address and reads the local address the kernel chose — this is the IP of
    /// the default-route interface. On multi-homed hosts this may not be the
    /// LAN interface the operator expects.
    fn primary_ipv4(&self) -> Option<IpAddr> {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("8.8.8.8:80").ok()?;
        match socket.local_addr().ok()?.ip() {
            IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(IpAddr::V4(ip)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UdpHostNetwork;
    use pi_domain::contracts::HostNetwork;

    #[test]
    fn primary_ipv4_does_not_panic() {
        let network = UdpHostNetwork::new();
        let ip = network.primary_ipv4();

        if let Some(ip) = ip {
            assert!(ip.is_ipv4(), "got {ip}");
            assert!(!ip.is_unspecified(), "got {ip}");
            assert!(!ip.is_loopback(), "got {ip}");
        }
    }
}
