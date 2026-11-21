//! 取りに行ける先の判定（Spec 78 D4 / `web_fetch_contract` 9）。純関数。
//!
//! **表は上流の公式 fetch サーバー（main）の `_is_public_address` に揃える** — 公開でない住所の表
//! （IANA の特別用途の登録簿のうち公開でないもの）と、その中で公開されている例外の表の 2 つ。
//! Rust の `IpAddr::is_global` は安定化していないので、表を純関数で持つ。
//!
//! 順序も上流と同じ: IPv4 射影・IPv4 変換・NAT64 は中の IPv4 を取り出して IPv4 として判定する →
//! 例外なら通す → 表に当たれば断る → 2000::/3 の外の IPv6 は断る → マルチキャストは断る → 残りは通す。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// 断った住所の分類（モデルへ返す理由の文に使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// 127.0.0.0/8・::1。
    Loopback,
    /// 10/8・172.16/12・192.168/16・100.64/10（共有アドレス）・fc00::/7。
    Private,
    /// 169.254/16（クラウドのメタデータの口の多く）・fe80::/10。
    LinkLocal,
    /// それ以外の公開でない住所（未指定・文書用・ベンチマーク・予約・マルチキャストほか）。
    Other,
}

impl Blocked {
    /// ログの語。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::LinkLocal => "link_local",
            Self::Other => "other",
        }
    }
}

/// 公開でない IPv4 の表（上流の `_NON_PUBLIC_NETWORKS` の IPv4 の行）。
const IPV4_NON_PUBLIC: [([u8; 4], u8, Blocked); 14] = [
    ([0, 0, 0, 0], 8, Blocked::Other),
    ([10, 0, 0, 0], 8, Blocked::Private),
    ([100, 64, 0, 0], 10, Blocked::Private),
    ([127, 0, 0, 0], 8, Blocked::Loopback),
    ([169, 254, 0, 0], 16, Blocked::LinkLocal),
    ([172, 16, 0, 0], 12, Blocked::Private),
    ([192, 0, 0, 0], 24, Blocked::Other),
    ([192, 0, 2, 0], 24, Blocked::Other),
    ([192, 88, 99, 0], 24, Blocked::Other),
    ([192, 168, 0, 0], 16, Blocked::Private),
    ([198, 18, 0, 0], 15, Blocked::Other),
    ([198, 51, 100, 0], 24, Blocked::Other),
    ([203, 0, 113, 0], 24, Blocked::Other),
    ([240, 0, 0, 0], 4, Blocked::Other),
];

/// 公開でない表の中で、公開されている例外（上流の `_PUBLIC_EXCEPTIONS` の IPv4 の行）。
const IPV4_PUBLIC_EXCEPTIONS: [[u8; 4]; 2] = [[192, 0, 0, 9], [192, 0, 0, 10]];

/// 公開でない IPv6 の表のうち 2000::/3 の中にあるもの（外は全部断るので、ここに要るのは中だけ）。
const IPV6_NON_PUBLIC_IN_GLOBAL: [([u16; 8], u8); 4] = [
    ([0x2001, 0, 0, 0, 0, 0, 0, 0], 23),
    ([0x2001, 0x0db8, 0, 0, 0, 0, 0, 0], 32),
    ([0x2002, 0, 0, 0, 0, 0, 0, 0], 16),
    ([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20),
];

/// 2001::/23 の中で公開されている例外（上流の `_PUBLIC_EXCEPTIONS` の IPv6 の行）。
const IPV6_PUBLIC_EXCEPTIONS: [([u16; 8], u8); 6] = [
    ([0x2001, 0x0001, 0, 0, 0, 0, 0, 1], 128),
    ([0x2001, 0x0001, 0, 0, 0, 0, 0, 2], 128),
    ([0x2001, 0x0003, 0, 0, 0, 0, 0, 0], 32),
    ([0x2001, 0x0004, 0x0112, 0, 0, 0, 0, 0], 48),
    ([0x2001, 0x0020, 0, 0, 0, 0, 0, 0], 28),
    ([0x2001, 0x0030, 0, 0, 0, 0, 0, 0], 28),
];

fn v4_in(ip: Ipv4Addr, net: [u8; 4], prefix: u8) -> bool {
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(prefix)) };
    (u32::from(ip) & mask) == (u32::from(Ipv4Addr::from(net)) & mask)
}

fn v6_in(ip: Ipv6Addr, net: [u16; 8], prefix: u8) -> bool {
    let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - u32::from(prefix)) };
    (u128::from(ip) & mask) == (u128::from(Ipv6Addr::from(net)) & mask)
}

/// IPv4 射影（::ffff:0:0/96）・IPv4 変換（::ffff:0:0:0/96）・NAT64（64:ff9b::/96）なら中の IPv4。
/// どれも、その先で IPv4 の宛先へ届く形（上流と同じ 3 つ）。
fn wrapped_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return Some(v4);
    }
    let seg = ip.segments();
    let low = Ipv4Addr::from(u32::from(seg[6]) << 16 | u32::from(seg[7]));
    let translated = seg[..4] == [0, 0, 0, 0] && seg[4] == 0xffff && seg[5] == 0;
    let nat64 = seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0];
    (translated || nat64).then_some(low)
}

fn classify_v4(ip: Ipv4Addr) -> Option<Blocked> {
    if IPV4_PUBLIC_EXCEPTIONS.contains(&ip.octets()) {
        return None;
    }
    if let Some((_, _, class)) = IPV4_NON_PUBLIC.iter().find(|(net, prefix, _)| v4_in(ip, *net, *prefix)) {
        return Some(*class);
    }
    if ip.is_multicast() {
        return Some(Blocked::Other);
    }
    None
}

fn classify_v6(ip: Ipv6Addr) -> Option<Blocked> {
    if let Some(v4) = wrapped_v4(ip) {
        return classify_v4(v4);
    }
    if IPV6_PUBLIC_EXCEPTIONS.iter().any(|(net, prefix)| v6_in(ip, *net, *prefix)) {
        return None;
    }
    // IPv4 互換（::/96）は中身に関わらず断る（廃止された形で、:: と ::1 を含む）。
    if ip.segments()[..6] == [0, 0, 0, 0, 0, 0] {
        return Some(if ip == Ipv6Addr::LOCALHOST { Blocked::Loopback } else { Blocked::Other });
    }
    // 2000::/3 の外は全部断る（許容の側から引く — 表に漏れた範囲が公開として通らない）。
    if !v6_in(ip, [0x2000, 0, 0, 0, 0, 0, 0, 0], 3) {
        return Some(if v6_in(ip, [0xfc00, 0, 0, 0, 0, 0, 0, 0], 7) {
            Blocked::Private
        } else if v6_in(ip, [0xfe80, 0, 0, 0, 0, 0, 0, 0], 10) {
            Blocked::LinkLocal
        } else {
            Blocked::Other
        });
    }
    if IPV6_NON_PUBLIC_IN_GLOBAL.iter().any(|(net, prefix)| v6_in(ip, *net, *prefix)) {
        return Some(Blocked::Other);
    }
    None
}

/// `ip` が公開でない住所なら、その分類。公開なら `None`。
pub fn classify(ip: IpAddr) -> Option<Blocked> {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Option<Blocked> {
        classify(s.parse().expect("住所として読める"))
    }

    #[test]
    fn public_addresses_pass() {
        for ip in ["8.8.8.8", "1.1.1.1", "93.184.215.14", "2606:4700:4700::1111", "2001:4860:4860::8888"] {
            assert_eq!(c(ip), None, "{ip} は公開");
        }
    }

    /// 表の境界の両側（1 つ内側は断り、1 つ外側は通す）。
    #[test]
    fn ipv4_table_edges() {
        let cases = [
            ("9.255.255.255", None),
            ("10.0.0.0", Some(Blocked::Private)),
            ("10.255.255.255", Some(Blocked::Private)),
            ("11.0.0.0", None),
            ("100.63.255.255", None),
            ("100.64.0.0", Some(Blocked::Private)),
            ("100.127.255.255", Some(Blocked::Private)),
            ("100.128.0.0", None),
            ("127.0.0.1", Some(Blocked::Loopback)),
            ("126.255.255.255", None),
            ("169.254.169.254", Some(Blocked::LinkLocal)),
            ("172.15.255.255", None),
            ("172.16.0.0", Some(Blocked::Private)),
            ("172.31.255.255", Some(Blocked::Private)),
            ("172.32.0.0", None),
            ("192.0.0.8", Some(Blocked::Other)),
            ("192.0.0.9", None),
            ("192.0.0.10", None),
            ("192.0.0.11", Some(Blocked::Other)),
            ("192.0.2.1", Some(Blocked::Other)),
            ("192.88.99.1", Some(Blocked::Other)),
            ("192.168.1.1", Some(Blocked::Private)),
            ("198.17.255.255", None),
            ("198.18.0.0", Some(Blocked::Other)),
            ("198.19.255.255", Some(Blocked::Other)),
            ("198.20.0.0", None),
            ("198.51.100.1", Some(Blocked::Other)),
            ("203.0.113.1", Some(Blocked::Other)),
            ("0.0.0.0", Some(Blocked::Other)),
            ("224.0.0.1", Some(Blocked::Other)),
            ("239.255.255.255", Some(Blocked::Other)),
            ("240.0.0.1", Some(Blocked::Other)),
            ("255.255.255.255", Some(Blocked::Other)),
        ];
        for (ip, want) in cases {
            assert_eq!(c(ip), want, "{ip}");
        }
    }

    /// IPv4 を包んだ IPv6 は中の IPv4 で判定する（上流と同じ 3 つ）。
    #[test]
    fn wrapped_ipv4_is_judged_by_the_inner_address() {
        assert_eq!(c("::ffff:127.0.0.1"), Some(Blocked::Loopback), "IPv4 射影");
        assert_eq!(c("::ffff:7f00:1"), Some(Blocked::Loopback), "IPv4 射影（16 進）");
        assert_eq!(c("::ffff:0:a00:1"), Some(Blocked::Private), "IPv4 変換 10.0.0.1");
        assert_eq!(c("64:ff9b::a9fe:a9fe"), Some(Blocked::LinkLocal), "NAT64 169.254.169.254");
        assert_eq!(c("::ffff:8.8.8.8"), None, "射影でも公開なら通す");
        assert_eq!(c("64:ff9b::808:808"), None, "NAT64 でも公開なら通す");
    }

    #[test]
    fn ipv6_table() {
        let cases = [
            ("::1", Some(Blocked::Loopback)),
            ("::", Some(Blocked::Other)),
            ("::808:808", Some(Blocked::Other)), // IPv4 互換は中身に関わらず断る
            ("fe80::1", Some(Blocked::LinkLocal)),
            ("fc00::1", Some(Blocked::Private)),
            ("fd00:ec2::254", Some(Blocked::Private)),
            ("fec0::1", Some(Blocked::Other)),
            ("ff02::1", Some(Blocked::Other)),
            ("64:ff9b:1::1", Some(Blocked::Other)),
            ("100::1", Some(Blocked::Other)),
            ("2001::1", Some(Blocked::Other)),
            ("2001:db8::1", Some(Blocked::Other)),
            ("2002::1", Some(Blocked::Other)),
            ("3fff::1", Some(Blocked::Other)),
            ("2001:1::1", None),
            ("2001:1::2", None),
            ("2001:1::3", Some(Blocked::Other)),
            ("2001:3::1", None),
            ("2001:4:112::1", None),
            ("2001:20::1", None),
            ("2001:30::1", None),
            ("2001:200::1", None), // 2001::/23 の外（2001:0200::/23 から上は公開）
        ];
        for (ip, want) in cases {
            assert_eq!(c(ip), want, "{ip}");
        }
    }
}
