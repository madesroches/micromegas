//! A typed, allowlist-only IP allowlist: a set of CIDR ranges (or bare IPs, treated as a `/32` or
//! `/128`) a credential may be presented from. An empty allowlist means "no restriction" -- the
//! backward-compatible default for every key minted before this feature existed.

use anyhow::{Context, Result};
use ipnet::IpNet;
use std::net::IpAddr;

/// A set of CIDR ranges (or bare IPs) a credential may be used from.
///
/// Empty (the `Default`/`parse(&[])` result) means "no restriction": [`Self::allows`] always
/// returns `true`. Once non-empty, an unresolved client IP (`None`) never satisfies the
/// restriction -- "unknown" must never be treated as "allowed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpAllowlist(Vec<IpNet>);

impl IpAllowlist {
    /// Parses `entries` (CIDR notation, e.g. `"10.0.0.0/8"`, or a bare IP, e.g.
    /// `"203.0.113.7"`) into an [`IpAllowlist`]. An empty slice parses to an empty (unrestricted)
    /// allowlist. Fails on the first entry that is neither a valid CIDR nor a valid bare IP
    /// address.
    ///
    /// `IpNet::from_str` doesn't accept a bare IP without a prefix, so this falls back to
    /// parsing a bare `IpAddr` and widening it to a `/32`/`/128` via `IpNet::from`.
    ///
    /// Also rejects a CIDR whose address has host bits set (e.g. `10.0.0.5/8`): `ipnet` parses
    /// it successfully but silently widens the range to the whole `/8`, which is the opposite of
    /// what an operator narrowing an allowlist intends.
    pub fn parse(entries: &[String]) -> Result<Self> {
        let mut nets = Vec::with_capacity(entries.len());
        for entry in entries {
            let net = entry
                .parse::<IpNet>()
                .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
                .with_context(|| format!("invalid CIDR or IP address: {entry:?}"))?;
            let net = canonicalize(net);
            if net.addr() != net.network() {
                anyhow::bail!(
                    "CIDR entry {entry:?} has host bits set; did you mean {:?}?",
                    net.trunc().to_string()
                );
            }
            nets.push(net);
        }
        Ok(Self(nets))
    }

    /// `true` when `ip` is permitted by this allowlist: always `true` when the allowlist is
    /// empty (no restriction); otherwise `true` only when `ip` is `Some` and falls within at
    /// least one stored network. An unresolved client IP (`None`) never satisfies a non-empty
    /// allowlist.
    pub fn allows(&self, ip: Option<IpAddr>) -> bool {
        if self.0.is_empty() {
            return true;
        }
        match ip {
            Some(ip) => self.0.iter().any(|net| net.contains(&ip)),
            None => false,
        }
    }
}

/// Downgrades an IPv4-mapped IPv6 entry (e.g. `::ffff:203.0.113.7/128`, `::ffff:203.0.113.0/120`,
/// or a bare `::ffff:203.0.113.7`) to its IPv4 form. `resolve_client_ip` always canonicalizes the
/// client IP it checks against (`to_canonical()`), so an unnormalized V6-mapped entry here
/// would never match: `ipnet`'s `contains` treats a V6 net and a V4 address as different
/// families regardless of value. The mapped prefix `::ffff:0:0/96` is exactly the fixed 96 bits
/// ahead of the embedded v4 address, so any prefix `>= 96` has a well-defined v4 equivalent at
/// `prefix_len - 96`.
fn canonicalize(net: IpNet) -> IpNet {
    if net.prefix_len() >= 96 {
        let canonical = net.addr().to_canonical();
        if canonical.is_ipv4() {
            return IpNet::new(canonical, net.prefix_len() - 96)
                .expect("a v4 address always accepts a prefix in 0..=32");
        }
    }
    net
}

impl Default for IpAllowlist {
    /// The empty, unrestricted allowlist.
    fn default() -> Self {
        Self(Vec::new())
    }
}
