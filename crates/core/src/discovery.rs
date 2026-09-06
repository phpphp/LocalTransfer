//! UDP 多播设备发现
//!
//! - 固定多播组 [`DISCOVERY_GROUP`]:[`DISCOVERY_PORT`]（SO_REUSEADDR 支持同机多实例）
//! - 枚举本机非环回 IPv4 接口逐一 join + 轮流设置 IP_MULTICAST_IF 发送（应对多网卡/VPN）
//! - 收到陌生/变更的 announce 时单播回自己的 announce，实现双向快速发现

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddrV4},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use smol::channel::Sender;
use socket2::{Protocol, SockRef, Socket, Type};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::proto::{
    CoreEvent, Device, DeviceInfo, DiscoveryPacket, DEVICE_TIMEOUT_SECS, DISCOVERY_GROUP,
    DISCOVERY_PORT,
};

#[derive(Default)]
pub struct Registry {
    devices: HashMap<String, Device>,
}

impl Registry {
    /// 处理一条 announce；返回设备表示"新增/信息变更/复活"，需要通知 UI
    pub(crate) fn on_announce(&mut self, info: DeviceInfo, addr: IpAddr, now_ms: i64) -> Option<Device> {
        // 同 IP 上的旧 id 幽灵（重装后的残留）：新设备认领了这个 IP，
        // 旧条目不可能再回来了，直接清掉
        self.devices
            .retain(|_, d| d.info.id == info.id || d.addr != addr || d.online);
        match self.devices.get_mut(&info.id) {
            Some(existing) => {
                let changed = existing.info != info || !existing.online;
                existing.info = info;
                existing.addr = addr;
                existing.online = true;
                existing.last_seen_ms = now_ms;
                if changed {
                    Some(existing.clone())
                } else {
                    None
                }
            }
            None => {
                let dev = Device {
                    info,
                    addr,
                    online: true,
                    last_seen_ms: now_ms,
                };
                self.devices.insert(dev.info.id.clone(), dev.clone());
                Some(dev)
            }
        }
    }

    fn on_bye(&mut self, id: &str) -> Option<(String, String)> {
        if let Some(d) = self.devices.get_mut(id) {
            if d.online {
                d.online = false;
                return Some((id.to_string(), d.info.name.clone()));
            }
        }
        None
    }

    fn prune(&mut self, now_ms: i64) -> Vec<(String, String)> {
        let timeout = (DEVICE_TIMEOUT_SECS * 1000) as i64;
        let mut downs = Vec::new();
        for d in self.devices.values_mut() {
            if d.online && now_ms - d.last_seen_ms > timeout {
                d.online = false;
                downs.push((d.info.id.clone(), d.info.name.clone()));
            }
        }
        // 离线超过 2 分钟的条目整个移除（含重装后的旧 id 幽灵），
        // 防止注册表无限膨胀、被同 IP 流量误复活
        let dead: Vec<String> = self
            .devices
            .iter()
            .filter(|(_, d)| !d.online && now_ms - d.last_seen_ms > 120_000)
            .map(|(id, _)| id.clone())
            .collect();
        for id in dead {
            self.devices.remove(&id);
        }
        downs
    }

    pub fn get(&self, id: &str) -> Option<&Device> {
        self.devices.get(id)
    }

    /// 按源 IP 刷新在线状态（对端的 HTTP 访问证明其存活）。
    /// 只刷新**在线**条目、不复活离线条目：设备重新上线只认它自己的
    /// announce。否则同一 IP 上"旧安装的幽灵设备"会被新设备的 HTTP
    /// 流量反复续命（表现为同一台手机在多个 IP 上同时在线）。
    pub(crate) fn touch_by_ip(&mut self, ip: IpAddr, now_ms: i64) {
        for d in self.devices.values_mut() {
            if d.addr == ip && d.online {
                d.last_seen_ms = now_ms;
            }
        }
    }

    pub fn list(&self) -> Vec<Device> {
        let mut v: Vec<Device> = self.devices.values().cloned().collect();
        v.sort_by(|a, b| b.last_seen_ms.cmp(&a.last_seen_ms));
        v
    }
}

/// 共享身份：改名（UiCommand::Rename）后 announce 与单播回复立刻携带新名
pub type SharedMe = Arc<Mutex<DeviceInfo>>;

pub struct DiscoveryHandle {
    pub registry: Arc<Mutex<Registry>>,
    pub me: SharedMe,
    pub shutdown: CancellationToken,
}

/// 启动发现子系统。返回前完成 socket 绑定（端口占用等错误尽早暴露）。
pub fn start(me: SharedMe, event_tx: Sender<CoreEvent>) -> Result<DiscoveryHandle> {
    let socket = bind_discovery_socket().context("绑定发现端口失败（UDP 53817）")?;

    let interfaces = local_ipv4_interfaces();
    if interfaces.is_empty() {
        tracing::warn!("未找到可用的非环回 IPv4 网卡，多播可能无法收发");
    }
    for ip in &interfaces {
        if let Err(e) = socket.join_multicast_v4(&DISCOVERY_GROUP, ip) {
            tracing::warn!("join 多播组失败 iface={ip}: {e}");
        }
    }
    // 兜底：再按默认接口（INADDR_ANY）加入一次，覆盖枚举不到/虚拟网卡场景
    let _ = socket.join_multicast_v4(&DISCOVERY_GROUP, &std::net::Ipv4Addr::UNSPECIFIED);
    // 允许同机实例收到自己的多播（按 id 过滤掉即可）
    let _ = socket.set_multicast_loop_v4(true);
    let _ = socket.set_multicast_ttl_v4(4);
    // tokio 需要非阻塞 socket
    socket.set_nonblocking(true)?;

    let socket = UdpSocket::from_std(socket.into())?;

    let registry = Arc::new(Mutex::new(Registry::default()));
    let shutdown = CancellationToken::new();

    // 接收循环
    let me_recv = me.clone();
    tokio::spawn({
        let registry = registry.clone();
        let event_tx = event_tx.clone();
        let shutdown = shutdown.clone();
        async move {
            recv_loop(socket, registry, event_tx, me_recv, shutdown).await;
        }
    });

    // announce 循环
    let me_announce = me.clone();
    tokio::spawn({
        let event_tx = event_tx.clone();
        let shutdown = shutdown.clone();
        async move {
            announce_loop(me_announce, interfaces, event_tx, shutdown).await;
        }
    });

    // 离线清理（独立节奏，5s 一次）
    tokio::spawn({
        let registry = registry.clone();
        let event_tx = event_tx.clone();
        let shutdown = shutdown.clone();
        async move {
            prune_loop(registry, event_tx, shutdown).await;
        }
    });

    Ok(DiscoveryHandle {
        registry,
        me,
        shutdown,
    })
}

fn bind_discovery_socket() -> Result<std::net::UdpSocket> {
    let sock = Socket::new(socket2::Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // 同机多实例都要绑 53817 收多播
    sock.set_reuse_address(true)?;
    let bind_addr: std::net::SocketAddr = (std::net::Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT).into();
    sock.bind(&bind_addr.into())?;
    Ok(sock.into())
}

fn local_ipv4_interfaces() -> Vec<Ipv4Addr> {
    match if_addrs::get_if_addrs() {
        Ok(list) => list
            .into_iter()
            .filter(|i| !i.is_loopback())
            .filter_map(|i| match i.ip() {
                IpAddr::V4(v4) => Some(v4),
                _ => None,
            })
            .collect(),
        Err(e) => {
            tracing::warn!("枚举网卡失败: {e}");
            Vec::new()
        }
    }
}

async fn recv_loop(
    socket: UdpSocket,
    registry: Arc<Mutex<Registry>>,
    event_tx: Sender<CoreEvent>,
    me: SharedMe,
    shutdown: CancellationToken,
) {
    let mut buf = vec![0u8; 2048];
    // 每对设备的单播回复节流（id → 上次回复时间 ms），防互回风暴
    let last_replies: Mutex<HashMap<String, i64>> = Mutex::new(HashMap::new());
    loop {
        let (n, src) = tokio::select! {
            r = socket.recv_from(&mut buf) => match r {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("recv_from 失败: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            },
            _ = shutdown.cancelled() => break,
        };

        let pkt = match serde_json::from_slice::<DiscoveryPacket>(&buf[..n]) {
            Ok(p) => p,
            Err(_) => continue, // 非 LocalTransfer 包，忽略
        };
        let IpAddr::V4(src_v4) = src.ip() else {
            continue;
        };
        let me = me.lock().unwrap().clone();

        match pkt {
            DiscoveryPacket::Announce { info } => {
                if info.id == me.id || info.v != crate::proto::PROTOCOL_VERSION {
                    continue;
                }
                let info_display = format!("{} @ {src_v4}:{}", info.name, info.port);
                let now = crate::proto::now_ms();
                let info_id = info.id.clone();
                let up = {
                    let mut reg = registry.lock().unwrap();
                    reg.on_announce(info, IpAddr::V4(src_v4), now)
                };
                if up.is_some() {
                    tracing::info!("发现设备: {}", info_display);
                    let _ = event_tx.try_send(CoreEvent::DeviceUp(up.unwrap()));
                }
                // 对每个有效 announce 都单播回自己的身份（每对设备 2s 节流）。
                // 之前只在"新增/变更"时回一次：那一个 UDP 包丢了（WiFi 常见），
                // 且对端（尤其 Android）收不到周期多播——MulticastLock 未持有时
                // WiFi 芯片直接丢弃多播/广播帧——就会出现"对端看不到我"的单向发现。
                // 节流同时防止 A/B 互回形成乒乓风暴。
                let should_reply = {
                    let mut last = last_replies.lock().unwrap();
                    let due = last
                        .get(&info_id)
                        .map(|t| now.saturating_sub(*t) >= 2000)
                        .unwrap_or(true);
                    if due {
                        last.insert(info_id, now);
                    }
                    due
                };
                if should_reply {
                    let reply =
                        serde_json::to_vec(&DiscoveryPacket::Announce { info: me.clone() })
                            .unwrap_or_default();
                    let _ = socket
                        .send_to(&reply, SocketAddrV4::new(src_v4, DISCOVERY_PORT))
                        .await;
                }
            }
            DiscoveryPacket::Bye { id } => {
                if id == me.id {
                    continue;
                }
                let down = {
                    let mut reg = registry.lock().unwrap();
                    reg.on_bye(&id)
                };
                if let Some((id, name)) = down {
                    tracing::info!("设备离线: {name}");
                    let _ = event_tx.try_send(CoreEvent::DeviceDown { id, name });
                }
            }
        }
    }
}

async fn announce_loop(
    me: SharedMe,
    interfaces: Vec<Ipv4Addr>,
    event_tx: Sender<CoreEvent>,
    shutdown: CancellationToken,
) {
    let socket = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => {
            let _ = event_tx.try_send(CoreEvent::Error {
                context: "发现服务".into(),
                message: format!("announce socket 创建失败: {e}"),
            });
            return;
        }
    };
    // 广播兜底需要 SO_BROADCAST
    let _ = socket.set_broadcast(true);

    let mut tick: u64 = 0;
    loop {
        // 每轮重取身份：改名（Rename）后下一条 announce 即携带新名
        let payload =
            serde_json::to_vec(&DiscoveryPacket::Announce { info: me.lock().unwrap().clone() })
                .expect("序列化 announce 不可能失败");
        // 多播出口按接口轮转：设置 IP_MULTICAST_IF 后发送
        let ifaces = if interfaces.is_empty() {
            vec![Ipv4Addr::UNSPECIFIED]
        } else {
            interfaces.clone()
        };
        for ip in &ifaces {
            if !ip.is_unspecified() {
                let sr = SockRef::from(&socket);
                if let Err(e) = sr.set_multicast_if_v4(ip) {
                    tracing::debug!("set_multicast_if_v4({ip}) 失败: {e}");
                    continue;
                }
            }
            let _ = socket
                .send_to(&payload, SocketAddrV4::new(DISCOVERY_GROUP, DISCOVERY_PORT))
                .await;
        }
        // 广播兜底：部分网络（AP 隔离多播/交换机不转发多播）下广播更可靠，飞秋也用广播
        let _ = socket
            .send_to(
                &payload,
                SocketAddrV4::new(std::net::Ipv4Addr::BROADCAST, DISCOVERY_PORT),
            )
            .await;

        // 心跳节奏：前 6s 每 2s（快速互相发现），之后每 10s
        let interval = if tick < 3 { 2 } else { 10 };
        tick += 1;

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
            _ = shutdown.cancelled() => {
                let bye = serde_json::to_vec(&DiscoveryPacket::Bye {
                    id: me.lock().unwrap().id.clone(),
                })
                .unwrap_or_default();
                for ip in &ifaces {
                    if !ip.is_unspecified() {
                        let sr = SockRef::from(&socket);
                        let _ = sr.set_multicast_if_v4(ip);
                    }
                    let _ = socket
                        .send_to(&bye, SocketAddrV4::new(DISCOVERY_GROUP, DISCOVERY_PORT))
                        .await;
                }
                break;
            }
        }
    }
}

async fn prune_loop(
    registry: Arc<Mutex<Registry>>,
    event_tx: Sender<CoreEvent>,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            // 2s 一查：15s 超时下最坏 17s 判离线（配合节流单播回复，误判率低）
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = shutdown.cancelled() => break,
        }
        let downs = {
            let mut reg = registry.lock().unwrap();
            reg.prune(crate::proto::now_ms())
        };
        for (id, name) in downs {
            tracing::info!("设备离线（心跳超时）: {name}");
            let _ = event_tx.try_send(CoreEvent::DeviceDown { id, name });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, name: &str) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            name: name.into(),
            plat: "windows".into(),
            port: 53817,
            v: 1,
        }
    }

    #[test]
    fn announce_new_and_change() {
        let mut r = Registry::default();
        let ip: IpAddr = "192.168.1.10".parse().unwrap();
        assert!(r.on_announce(info("a", "A"), ip, 0).is_some());
        // 同信息重复 announce 不再通知
        assert!(r.on_announce(info("a", "A"), ip, 1).is_none());
        // 名字变化要通知
        assert!(r.on_announce(info("a", "A2"), ip, 2).is_some());
    }

    #[test]
    fn bye_prune_revive() {
        let mut r = Registry::default();
        let ip: IpAddr = "192.168.1.10".parse().unwrap();
        r.on_announce(info("a", "A"), ip, 0);
        assert_eq!(r.on_bye("a"), Some(("a".into(), "A".into())));
        // 重复 bye 不通知
        assert_eq!(r.on_bye("a"), None);
        // 离线后重新 announce 要通知（复活）
        assert!(r.on_announce(info("a", "A"), ip, 5).is_some());
        // 超时清理
        assert_eq!(
            r.prune(5 + (DEVICE_TIMEOUT_SECS * 1000) as i64 + 1).len(),
            1
        );
    }

    #[test]
    fn rel_path_safety() {
        use crate::proto::FileMeta;
        assert!(FileMeta::is_rel_path_safe("a.txt"));
        assert!(FileMeta::is_rel_path_safe("dir/sub/a.txt"));
        assert!(!FileMeta::is_rel_path_safe("../a.txt"));
        assert!(!FileMeta::is_rel_path_safe("dir/../a.txt"));
        assert!(!FileMeta::is_rel_path_safe("/etc/passwd"));
        assert!(!FileMeta::is_rel_path_safe("C:/a.txt"));
        assert!(!FileMeta::is_rel_path_safe(""));
    }
}
