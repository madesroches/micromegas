use micromegas_auth::ip_allowlist::IpAllowlist;

fn ip(s: &str) -> std::net::IpAddr {
    s.parse().expect("valid ip")
}

fn entries(strs: &[&str]) -> Vec<String> {
    strs.iter().map(|s| s.to_string()).collect()
}

#[test]
fn empty_list_allows_any_ip_including_none() {
    let allowlist = IpAllowlist::parse(&[]).expect("empty list parses");
    assert!(allowlist.allows(Some(ip("203.0.113.7"))));
    assert!(allowlist.allows(Some(ip("::1"))));
    assert!(allowlist.allows(None));
}

#[test]
fn default_is_the_empty_unrestricted_allowlist() {
    let allowlist = IpAllowlist::default();
    assert!(allowlist.allows(None));
    assert!(allowlist.allows(Some(ip("203.0.113.7"))));
}

#[test]
fn single_bare_ip_entry_matches_only_that_exact_address_v4() {
    let allowlist = IpAllowlist::parse(&entries(&["203.0.113.7"])).expect("parses");
    assert!(allowlist.allows(Some(ip("203.0.113.7"))));
    assert!(!allowlist.allows(Some(ip("203.0.113.8"))));
    assert!(!allowlist.allows(None));
}

#[test]
fn single_bare_ip_entry_matches_only_that_exact_address_v6() {
    let allowlist = IpAllowlist::parse(&entries(&["2001:db8::1"])).expect("parses");
    assert!(allowlist.allows(Some(ip("2001:db8::1"))));
    assert!(!allowlist.allows(Some(ip("2001:db8::2"))));
    assert!(!allowlist.allows(None));
}

#[test]
fn slash_24_entry_matches_every_address_in_range_and_rejects_one_outside() {
    let allowlist = IpAllowlist::parse(&entries(&["203.0.113.0/24"])).expect("parses");
    assert!(allowlist.allows(Some(ip("203.0.113.0"))));
    assert!(allowlist.allows(Some(ip("203.0.113.255"))));
    assert!(allowlist.allows(Some(ip("203.0.113.42"))));
    assert!(!allowlist.allows(Some(ip("203.0.114.1"))));
}

#[test]
fn slash_64_entry_matches_every_address_in_range_and_rejects_one_outside() {
    let allowlist = IpAllowlist::parse(&entries(&["2001:db8::/64"])).expect("parses");
    assert!(allowlist.allows(Some(ip("2001:db8::1"))));
    assert!(allowlist.allows(Some(ip("2001:db8::ffff:ffff:ffff:ffff"))));
    assert!(!allowlist.allows(Some(ip("2001:db8:1::1"))));
}

#[test]
fn non_empty_list_rejects_none_unresolved_client_ip() {
    let allowlist = IpAllowlist::parse(&entries(&["203.0.113.0/24"])).expect("parses");
    assert!(!allowlist.allows(None));
}

#[test]
fn malformed_bad_cidr_syntax_returns_err() {
    assert!(IpAllowlist::parse(&entries(&["not-an-ip"])).is_err());
}

#[test]
fn malformed_out_of_range_prefix_length_returns_err() {
    assert!(IpAllowlist::parse(&entries(&["203.0.113.0/99"])).is_err());
}

#[test]
fn malformed_empty_string_returns_err() {
    assert!(IpAllowlist::parse(&entries(&[""])).is_err());
}

#[test]
fn multiple_entries_are_all_checked() {
    let allowlist = IpAllowlist::parse(&entries(&["10.0.0.0/8", "203.0.113.7"])).expect("parses");
    assert!(allowlist.allows(Some(ip("10.1.2.3"))));
    assert!(allowlist.allows(Some(ip("203.0.113.7"))));
    assert!(!allowlist.allows(Some(ip("192.168.1.1"))));
}

#[test]
fn one_malformed_entry_fails_the_whole_parse_fail_fast() {
    let result = IpAllowlist::parse(&entries(&["10.0.0.0/8", "garbage", "203.0.113.7"]));
    assert!(result.is_err());
}

#[test]
fn cidr_with_host_bits_set_returns_err_v4() {
    assert!(IpAllowlist::parse(&entries(&["10.0.0.5/8"])).is_err());
}

#[test]
fn cidr_with_host_bits_set_returns_err_v6() {
    assert!(IpAllowlist::parse(&entries(&["2001:db8::1/64"])).is_err());
}

#[test]
fn ipv4_mapped_v6_bare_entry_matches_the_canonical_v4_client_ip() {
    let allowlist = IpAllowlist::parse(&entries(&["::ffff:203.0.113.7"])).expect("parses");
    assert!(allowlist.allows(Some(ip("203.0.113.7"))));
    assert!(!allowlist.allows(Some(ip("203.0.113.8"))));
}

#[test]
fn ipv4_mapped_v6_slash_128_entry_matches_the_canonical_v4_client_ip() {
    let allowlist = IpAllowlist::parse(&entries(&["::ffff:203.0.113.7/128"])).expect("parses");
    assert!(allowlist.allows(Some(ip("203.0.113.7"))));
    assert!(!allowlist.allows(Some(ip("203.0.113.8"))));
}

#[test]
fn ipv4_mapped_v6_slash_120_prefix_entry_matches_the_canonical_v4_range() {
    let allowlist = IpAllowlist::parse(&entries(&["::ffff:203.0.113.0/120"])).expect("parses");
    assert!(allowlist.allows(Some(ip("203.0.113.0"))));
    assert!(allowlist.allows(Some(ip("203.0.113.255"))));
    assert!(!allowlist.allows(Some(ip("203.0.114.1"))));
}
