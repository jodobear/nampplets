use super::*;

#[test]
fn public_ip_policy_blocks_local_special_and_documentation_ranges() {
    for address in [
        "127.0.0.1",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.0.1",
        "169.254.169.254",
        "100.64.0.1",
        "192.0.2.1",
        "198.51.100.1",
        "203.0.113.1",
        "::1",
        "fc00::1",
        "fe80::1",
        "2001:db8::1",
        "::ffff:127.0.0.1",
    ] {
        assert!(
            !is_public_ip(address.parse().unwrap()),
            "{address} must be blocked"
        );
    }
    assert!(is_public_ip("1.1.1.1".parse().unwrap()));
    assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
}

#[test]
fn percent_decode_is_exact_and_refuses_malformed_input() {
    assert_eq!(percent_decode("a%20b").unwrap(), b"a b");
    assert!(percent_decode("%").is_err());
    assert!(percent_decode("%xx").is_err());
}

#[test]
fn mime_sniff_never_trusts_labels_or_delivers_raw_svg() {
    assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\nx"), Some(SniffedMime::Png));
    assert_eq!(
        sniff_mime(b"<?xml version=\"1.0\"?><svg></svg>"),
        Some(SniffedMime::Svg)
    );
    assert_eq!(sniff_mime(b"<html>not an image</html>"), None);
}
