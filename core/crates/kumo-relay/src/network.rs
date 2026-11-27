use std::net::{IpAddr, Ipv4Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairAddress {
    pub label: &'static str,
    pub url: String,
}

pub fn pair_addresses(port: u16) -> Vec<PairAddress> {
    let mut out: Vec<PairAddress> = labelled_ips()
        .map(|(label, ip)| PairAddress {
            label,
            url: format!("http://{ip}:{port}"),
        })
        .collect();
    out.sort_by_key(|a| if a.label == "Tailscale" { 0 } else { 1 });
    out.dedup();
    out
}

pub fn pair_ips() -> Vec<IpAddr> {
    labelled_ips().map(|(_, ip)| IpAddr::V4(ip)).collect()
}

fn labelled_ips() -> impl Iterator<Item = (&'static str, Ipv4Addr)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(ip) => classify(ip).map(|label| (label, ip)),
            IpAddr::V6(_) => None,
        })
}

fn classify(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, ..] = ip.octets();
    if a == 100 && (64..128).contains(&b) {
        Some("Tailscale")
    } else if ip.is_private() {
        Some("Same Wi-Fi")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_addresses() {
        assert_eq!(classify(Ipv4Addr::new(100, 113, 222, 8)), Some("Tailscale"));
        assert_eq!(classify(Ipv4Addr::new(192, 168, 1, 20)), Some("Same Wi-Fi"));
        assert_eq!(classify(Ipv4Addr::new(127, 0, 0, 1)), None);
        assert_eq!(classify(Ipv4Addr::new(8, 8, 8, 8)), None);
    }
}
