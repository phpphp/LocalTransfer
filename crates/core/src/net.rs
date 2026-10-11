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
    // 对端就是本机（同机多实例互发）：不绑定，让连接走纯回环——
    // 绑定物理源后自连接要绕行物理网卡路径，可能被 WFP/Hyper-V/VPN 过滤
    if ifaces.contains(&p) {
        return None;
    }
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

// ---------------------------------------------------------------- 网关链路诊断

/// ICMP ping 统计（判断速度慢在哪：无线信号差=网关延迟高/丢包）
#[derive(Debug, Clone)]
pub struct PingStats {
    pub sent: u32,
    pub received: u32,
    /// 平均往返延迟 ms
    pub avg_ms: f64,
    /// 相邻延迟差的平均（抖动）ms
    pub jitter_ms: f64,
    /// 丢包率 0.0~1.0
    pub loss: f64,
}

/// 默认网关 IPv4（取路由表 0.0.0.0/0 中 metric 最小的下一跳）。
/// 非 Windows 或取不到返回 None。
#[cfg(target_os = "windows")]
pub fn default_gateway_v4() -> Option<std::net::Ipv4Addr> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetIpForwardTable, MIB_IPFORWARDTABLE,
    };
    unsafe {
        let mut size: u32 = 0;
        let _ = GetIpForwardTable(std::ptr::null_mut(), &mut size, 0);
        if size == 0 || size > 1 << 20 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let table = buf.as_mut_ptr() as *mut MIB_IPFORWARDTABLE;
        if GetIpForwardTable(table, &mut size, 0) != 0 {
            return None;
        }
        let n = (*table).dwNumEntries as usize;
        let rows = (*table).table.as_ptr();
        let mut best: Option<(u32, u32)> = None; // (metric, 网关)
        for i in 0..n {
            let r = &*rows.add(i);
            if r.dwForwardDest == 0 && r.dwForwardMask == 0 {
                let hop = u32::from_be(r.dwForwardNextHop);
                let gw = std::net::Ipv4Addr::from(hop);
                // 0.0.0.0（按接口路由）不算
                if gw.is_unspecified() {
                    continue;
                }
                if best.is_none_or(|(m, _)| r.dwForwardMetric1 < m) {
                    best = Some((r.dwForwardMetric1, hop));
                }
            }
        }
        best.map(|(_, hop)| std::net::Ipv4Addr::from(hop))
    }
}

#[cfg(not(target_os = "windows"))]
pub fn default_gateway_v4() -> Option<std::net::Ipv4Addr> {
    None
}

/// ICMP ping（IcmpSendEcho，无需管理员）。返回 None = 平台不支持/无法创建句柄。
#[cfg(target_os = "windows")]
pub fn icmp_ping(host: std::net::Ipv4Addr, count: u32) -> Option<PingStats> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY,
    };
    const TIMEOUT_MS: u32 = 1500;
    let handle: HANDLE = unsafe { IcmpCreateFile() };
    if handle.is_null() {
        return None;
    }
    // IcmpSendEcho 的 destinationaddress 要求"网络字节序的 u32"——
    // 即内存布局为 [a,b,c,d]；小端机上需 u32::from(ip).to_be()（实测直传丢包 100%）
    let dest = u32::from(host).to_be();
    let payload = [0x61u8; 16]; // "a" x16
    let mut rtts: Vec<f64> = Vec::with_capacity(count as usize);
    let mut received: u32 = 0;
    for _ in 0..count {
        let mut reply: ICMP_ECHO_REPLY = unsafe { std::mem::zeroed() };
        let reply_size = std::mem::size_of::<ICMP_ECHO_REPLY>() as u32 + payload.len() as u32 + 8;
        let mut reply_buf = vec![0u8; reply_size as usize];
        let t0 = std::time::Instant::now();
        let n = unsafe {
            IcmpSendEcho(
                handle,
                dest,
                payload.as_ptr() as *const _,
                payload.len() as u16,
                std::ptr::null(),
                reply_buf.as_mut_ptr() as *mut _,
                reply_size,
                TIMEOUT_MS,
            )
        };
        let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
        if n >= 1 {
            reply = unsafe { *(reply_buf.as_ptr() as *const ICMP_ECHO_REPLY) };
            if reply.Status == 0 {
                received += 1;
                rtts.push(elapsed);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(handle);
    }
    let (avg, jitter) = if rtts.is_empty() {
        (f64::NAN, f64::NAN)
    } else {
        let avg = rtts.iter().sum::<f64>() / rtts.len() as f64;
        let jit = if rtts.len() >= 2 {
            rtts.windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .sum::<f64>()
                / (rtts.len() - 1) as f64
        } else {
            0.0
        };
        (avg, jit)
    };
    Some(PingStats {
        sent: count,
        received,
        avg_ms: avg,
        jitter_ms: jitter,
        loss: 1.0 - received as f64 / count as f64,
    })
}

#[cfg(not(target_os = "windows"))]
pub fn icmp_ping(_host: std::net::Ipv4Addr, _count: u32) -> Option<PingStats> {
    None
}

/// 本机网络链路信息（诊断传输天花板：WiFi 协商速率 × 信号质量）
#[derive(Debug, Clone)]
pub struct WifiLink {
    /// 网卡描述（WiFi SSID 或 "有线"）
    pub name: String,
    pub is_wifi: bool,
    /// 协商速率 Mbps（WiFi: RX/TX；有线：链路速率）
    pub rx_mbps: f64,
    pub tx_mbps: f64,
    /// 信号质量 0-100（仅 WiFi）
    pub signal_pct: u32,
}

/// 当前活动网卡的链路信息。
/// WiFi 走 WlanQueryInterface（协商速率=iperf 类吞吐的天花板）；
/// 非 WiFi（有线）从 if_addrs 网卡名粗判，速率未知返回 0。
#[cfg(target_os = "windows")]
pub fn wifi_link_info() -> Option<WifiLink> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::NetworkManagement::WiFi::{
        WlanEnumInterfaces, WlanOpenHandle, WlanQueryInterface,
        wlan_intf_opcode_current_connection, WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO,
        WLAN_INTERFACE_INFO_LIST,
    };
    unsafe {
        let mut handle: HANDLE = std::ptr::null_mut();
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let wlan_ok = WlanOpenHandle(2, std::ptr::null(), &mut 0, &mut handle) == 0
            && WlanEnumInterfaces(handle, std::ptr::null(), &mut list) == 0;
        let mut out: Option<WifiLink> = None;
        if wlan_ok {
        let n = (*list).dwNumberOfItems;
        let items = (*list).InterfaceInfo.as_ptr();
        for i in 0..n as usize {
            let it: WLAN_INTERFACE_INFO = *items.add(i);
            if it.isState != 1 { // wlan_interface_state_connected
                continue;
            }
            // 接口描述（UTF-16 定长 256）
            let desc = String::from_utf16_lossy(
                &it.strInterfaceDescription[..]
                    .iter()
                    .take_while(|c| **c != 0)
                    .copied()
                    .collect::<Vec<_>>(),
            );
            let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut size: u32 = 0;
            if WlanQueryInterface(
                handle,
                &it.InterfaceGuid,
                wlan_intf_opcode_current_connection,
                std::ptr::null(),
                &mut size,
                &mut data,
                std::ptr::null_mut(),
            ) == 0 && !data.is_null() && size as usize >= std::mem::size_of::<WLAN_CONNECTION_ATTRIBUTES>()
            {
                let conn = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                let assoc = &conn.wlanAssociationAttributes;
                let ssid = String::from_utf16_lossy(
                    &assoc.dot11Ssid.ucSSID[..assoc.dot11Ssid.uSSIDLength as usize]
                        .iter().map(|b| *b as u16).collect::<Vec<_>>(),
                );
                // ulRxRate/ulTxRate：bps（实测 866.7M 档回报 866700000）
                let rx = assoc.ulRxRate as f64 / 1e6;
                let tx = assoc.ulTxRate as f64 / 1e6;
                let link = WifiLink {
                    name: if ssid.is_empty() { desc.clone() } else { ssid },
                    is_wifi: true,
                    rx_mbps: rx,
                    tx_mbps: tx,
                    signal_pct: assoc.wlanSignalQuality,
                };
                // 取第一个已连接的 WiFi 口
                if out.is_none() || link.signal_pct > out.as_ref().unwrap().signal_pct {
                    out = Some(link);
                }
                windows_sys::Win32::NetworkManagement::WiFi::WlanFreeMemory(data);
            }
        }
        }
        if wlan_ok {
            windows_sys::Win32::NetworkManagement::WiFi::WlanFreeMemory(list as _);
            windows_sys::Win32::Foundation::CloseHandle(handle);
            if out.is_some() {
                return out;
            }
        }
        // 无 WiFi 口：有线（速率未知——if_addrs 拿不到链路速率，标 0）
        let phys = lan_ipv4_interfaces();
        if !phys.is_empty() {
            return Some(WifiLink {
                name: "有线".into(),
                is_wifi: false,
                rx_mbps: 0.0,
                tx_mbps: 0.0,
                signal_pct: 100,
            });
        }
        None
    }
}

#[cfg(not(target_os = "windows"))]
pub fn wifi_link_info() -> Option<WifiLink> {
    None
}
