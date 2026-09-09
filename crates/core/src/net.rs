//! VPN 环境下的局域网出站加固。
//!
//! 飞连 / EasyConnect 等企业 VPN 会向路由表推宽路由（如 192.168.0.0/16 甚至
//! 0.0.0.0/1），默认路由选择会把本应走物理网卡的 LAN 流量吸进隧道——
//! 表现为"设备看得到，消息/文件却发不出去"。
//! 对策（Windows 强主机模型：socket 绑定源 IP 即锁定出接口）：
//! - 枚举网卡时过滤 VPN/虚拟网卡与 APIPA 自配地址，只保留物理口
//! - 与对端通信时把 socket 绑到同网段的本地源 IP，绕开隧道路由

use std::net::{IpAddr, Ipv4Addr};

/// 名字命中这些关键字的网卡按虚拟/VPN 处理（不参与发现多播与出站绑定）。
/// 小写子串匹配，覆盖常见中英文产品名。
const VIRTUAL_IFACE_KEYWORDS: &[&str] = &[
    // 通用 TUN/TAP 家族
    "tap", "tun", "wintun", "wireguard", "openvpn", "tun2socks",
    // 企业 VPN
    "easyconnect", "sangfor", // 深信服 EasyConnect
    "feilian", "飞连", "volcengine", // 火山飞连
    "cisco", "anyconnect", "globalprotect", "forticlient", "pulsesecure", "vpn",
    // 虚拟组网
    "tailscale", "zerotier", "hamachi", "leaflet", "tinc",
    // 代理 TUN
    "clash", "mihomo", "sing-box", "singbox", "netch", "sstap",
    // 虚拟机/容器
    "virtualbox", "vmnet", "hyper-v", "docker", "wsl", "veth",
];

fn is_virtual_iface(name: &str) -> bool {
    let lower = name.to_lowercase();
    VIRTUAL_IFACE_KEYWORDS.iter().any(|k| lower.contains(k))
}

/// 物理网卡的 IPv4 地址列表。
/// 过滤环回、APIPA（169.254.*，DHCP 失败自配）与虚拟/VPN 网卡；
/// 若全被过滤（识别列表没覆盖到的新 VPN 等），回退为全部非环回地址——
/// 宁可多走几个口也不能没有口。
pub fn lan_ipv4_interfaces() -> Vec<Ipv4Addr> {
    let list = match if_addrs::get_if_addrs() {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("枚举网卡失败: {e}");
            return Vec::new();
        }
    };
    let mut all: Vec<(String, Ipv4Addr)> = Vec::new();
    for i in list {
        if i.is_loopback() {
            continue;
        }
        if let IpAddr::V4(v4) = i.ip() {
            if v4.is_link_local() {
                continue; // 169.254.* APIPA
            }
            all.push((i.name, v4));
        }
    }
    let physical: Vec<Ipv4Addr> = all
        .iter()
        .filter(|(name, _)| !is_virtual_iface(name))
        .map(|(_, ip)| *ip)
        .collect();
    if physical.is_empty() {
        tracing::warn!("未识别到物理网卡（全部网卡被 VPN 过滤规则命中？），回退全部接口");
        all.into_iter().map(|(_, ip)| ip).collect()
    } else {
        if physical.len() < all.len() {
            let skipped: Vec<&str> = all
                .iter()
                .filter(|(n, _)| is_virtual_iface(n))
                .map(|(n, _)| n.as_str())
                .collect();
            tracing::info!("跳过虚拟/VPN 网卡: {}", skipped.join(", "));
        }
        physical
    }
}

/// 两个 IPv4 的公共前缀位数
fn common_prefix_bits(a: u32, b: u32) -> u32 {
    (a ^ b).leading_zeros()
}

/// 与对端同网段的本地源 IP（最长公共前缀 ≥ /16 才认）。
/// 用于出站 socket 绑定：绑到物理口源 IP 后路由查找被限制在该口的
/// 在链路由上，VPN 的宽路由（哪怕更精确）不再参与，流量必走物理网卡。
/// 找不到同网段口（对端不在本机任一网卡网段内）返回 None，调用方不绑定。
pub fn local_addr_for(peer: IpAddr) -> Option<Ipv4Addr> {
    let IpAddr::V4(p) = peer else {
        return None;
    };
    let ifaces = lan_ipv4_interfaces();
    ifaces
        .iter()
        .map(|ip| (common_prefix_bits(u32::from(*ip), u32::from(p)), ip))
        .filter(|(bits, _)| *bits >= 16)
        .max_by_key(|(bits, _)| *bits)
        .map(|(_, ip)| *ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_bits() {
        assert_eq!(common_prefix_bits(0x0A_00_00_01, 0x0A_00_00_02), 30);
        assert_eq!(common_prefix_bits(0xC0_A8_08_01, 0xC0_A8_08_C8), 24);
        assert_eq!(common_prefix_bits(0xC0_A8_08_01, 0x0A_00_00_01), 0);
    }

    #[test]
    fn virtual_names() {
        assert!(is_virtual_iface("EasyConnect Adapter"));
        assert!(is_virtual_iface("飞连 VPN Adapter"));
        assert!(is_virtual_iface("TAP-Windows Adapter V9"));
        assert!(is_virtual_iface("Clash TUN"));
        assert!(!is_virtual_iface("WLAN"));
        assert!(!is_virtual_iface("以太网"));
        assert!(!is_virtual_iface("Realtek PCIe GbE"));
    }
}
