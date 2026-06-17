use std::net::{IpAddr, UdpSocket};
use std::sync::Arc;

use pi_domain::contracts::HostNetwork;

pub struct UdpHostNetwork;

impl UdpHostNetwork {
    pub fn new() -> Arc<UdpHostNetwork> {
        Arc::new(UdpHostNetwork)
    }
}

impl HostNetwork for UdpHostNetwork {
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
