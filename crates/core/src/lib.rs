//! transfer-core：局域网发现 + HTTP 传输的纯 tokio 核心（不依赖 UI）
//!
//! 用法：在独立线程运行（见 [`start`]），UI 通过 smol channel 与之通信——
//! `UiCommand` 进、`CoreEvent` 出。

pub mod client;
pub mod discovery;
pub mod proto;
pub mod server;
pub mod store;
pub mod web;

pub use proto::{
    ChatMessage, CoreEvent, Device, DeviceInfo, FileMeta, MessageKind, UiCommand,
};
pub use discovery::SharedMe;
pub use store::{Config, Store, random_poetic_name};

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

use anyhow::{Context, Result};
use smol::channel::{Receiver, Sender};
use tokio_util::sync::CancellationToken;

use server::ServerState;

pub struct CoreHandle {
    cmd_tx: Sender<UiCommand>,
    shutdown: CancellationToken,
    thread: Option<JoinHandle<()>>,
}

impl CoreHandle {
    pub fn send(&self, cmd: UiCommand) -> Result<()> {
        self.cmd_tx
            .try_send(cmd)
            .map_err(|e| anyhow::anyhow!("核心线程已退出: {e}"))
    }

    /// 优雅关闭：广播 bye → 等核心线程结束
    pub fn shutdown(mut self) {
        self.shutdown_inner();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn shutdown_inner(&mut self) {
        let _ = self.send(UiCommand::Shutdown);
        self.shutdown.cancel();
    }
}

/// 窗口关闭（GPUI 退出，实体析构）时走这里：广播 bye 让对端立刻感知下线，
/// 而不是等心跳超时。收尾路径约 150ms，不会明显拖慢退出。
impl Drop for CoreHandle {
    fn drop(&mut self) {
        self.shutdown_inner();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 启动核心（独立线程 + 自有 tokio runtime）。绑定失败通过 `CoreEvent::Error` 上报。
pub fn start(config: Config, store: Store, event_tx: Sender<CoreEvent>) -> Result<CoreHandle> {
    // 日志写文件（GUI 模式没有控制台；按天滚动），保活 guard 防止丢失尾部日志
    {
        use tracing_appender::non_blocking::WorkerGuard;
        static LOG_GUARD: std::sync::OnceLock<WorkerGuard> = std::sync::OnceLock::new();
        if LOG_GUARD.get().is_none() {
            if let Ok(dir) = Config::app_dir().map(|d| d.join("logs")) {
                let appender = tracing_appender::rolling::daily(dir, "localtransfer.log");
                let (writer, guard) = tracing_appender::non_blocking(appender);
                LOG_GUARD.set(guard).ok();
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(
                        tracing_subscriber::EnvFilter::try_from_default_env()
                            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                    )
                    .with_writer(writer)
                    .try_init();
            }
        }
    }

    let (cmd_tx, cmd_rx) = smol::channel::unbounded::<UiCommand>();
    let shutdown = CancellationToken::new();

    let me = DeviceInfo {
        id: config.device_id.clone(),
        name: config.device_name.clone(),
        plat: platform().to_string(),
        port: config.http_port,
        v: proto::PROTOCOL_VERSION,
    };
    tracing::info!(
        "启动身份: {} (id {}…) HTTP 端口 {}",
        me.name,
        &me.id[..8],
        me.port
    );

    let thread = std::thread::Builder::new()
        .name("transfer-core".into())
        .spawn({
            let shutdown = shutdown.clone();
            move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("创建 tokio runtime 失败");
                rt.block_on(run(me, config, store, event_tx, cmd_rx, shutdown));
            }
        })
        .context("启动核心线程失败")?;

    Ok(CoreHandle {
        cmd_tx,
        shutdown,
        thread: Some(thread),
    })
}

fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    }
}

/// 从 start 开始向后找最多 tries 个端口，返回第一个能绑定 0.0.0.0 的端口。
/// 全部被占用返回 None（调用方可回落到原端口，由服务启动报错）。
pub fn pick_free_port(start: u16, tries: u16) -> Option<u16> {
    for off in 0..tries {
        let port = start.saturating_add(off);
        if port == 0 {
            break;
        }
        match std::net::TcpListener::bind(("0.0.0.0", port)) {
            Ok(_) => return Some(port),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(_) => continue, // 权限等其他错误也继续尝试
        }
    }
    None
}

/// 单实例锁：按 17877、17876… 依次探测回环绑定。
/// 返回 (锁 listener——必须保活到进程结束, 实例序号 1..=10)。
/// 序号 > 1 说明本机已有其他实例在跑，调用方应使用临时设备身份避免
/// 共用同一 device_id 互相把自己过滤掉。
pub fn acquire_instance_lock() -> (Option<std::net::TcpListener>, u32) {
    for i in 0..10u32 {
        let port = 17877u16.saturating_sub(i as u16);
        if let Ok(l) = std::net::TcpListener::bind(("127.0.0.1", port)) {
            return (Some(l), i + 1);
        }
    }
    (None, 99)
}

/// 本机对外的局域网 IPv4（侧栏展示用）：优先私网地址，其次任意非环回。
pub fn local_ip_hint() -> Option<std::net::Ipv4Addr> {
    let addrs = if_addrs::get_if_addrs().ok()?;
    let mut fallback = None;
    for a in addrs {
        if a.is_loopback() {
            continue;
        }
        if let std::net::IpAddr::V4(v4) = a.ip() {
            let o = v4.octets();
            let private = o[0] == 192 && o[1] == 168
                || o[0] == 10
                || o[0] == 172 && (16..=31).contains(&o[1]);
            if private {
                return Some(v4);
            }
            fallback = fallback.or(Some(v4));
        }
    }
    fallback
}

async fn run(
    me: DeviceInfo,
    config: Config,
    store: Store,
    event_tx: Sender<CoreEvent>,
    cmd_rx: Receiver<UiCommand>,
    shutdown: CancellationToken,
) {
    let store = Arc::new(Mutex::new(store));
    // 共享身份：改名后 announce / HTTP sender / info 端点全部立即生效
    let me: SharedMe = Arc::new(Mutex::new(me));

    // 发现服务
    let disc = match discovery::start(me.clone(), event_tx.clone()) {
        Ok(d) => d,
        Err(e) => {
            let _ = event_tx
                .try_send(CoreEvent::Error {
                    context: "设备发现".into(),
                    message: format!("启动失败: {e:#}"),
                });
            return;
        }
    };
    let registry = disc.registry.clone();

    // HTTP 服务
    let state = Arc::new(ServerState {
        me: me.clone(),
        store: store.clone(),
        registry: registry.clone(),
        event_tx: event_tx.clone(),
        default_download_dir: config.download_dir.clone(),
        pending: Mutex::new(HashMap::new()),
        sessions: Mutex::new(HashMap::new()),
        web_offers: Mutex::new(Vec::new()),
    });
    {
        let state = state.clone();
        let event_tx = event_tx.clone();
        let port = config.http_port;
        tokio::spawn(async move {
            if let Err(e) = server::serve(state, port).await {
                let _ = event_tx.try_send(CoreEvent::Error {
                    context: "HTTP 服务".into(),
                    message: format!("监听 0.0.0.0:{port} 失败：{e:#}（端口被占用？）"),
                });
            }
        });
    }

    // 发送中传输登记
    let sendings: client::Sendings = Arc::new(Mutex::new(HashMap::new()));

    // 命令循环
    loop {
        let cmd = tokio::select! {
            c = cmd_rx.recv() => match c {
                Ok(c) => c,
                Err(_) => break, // UI 侧 drop 了 handle
            },
            _ = shutdown.cancelled() => break,
        };

        match cmd {
            UiCommand::SendText { peer_id, text } => {
                let me = me.lock().unwrap().clone();
                handle_send_text(&me, &registry, &store, &event_tx, &peer_id, text);
            }
            UiCommand::SendFiles { peer_id, paths } => {
                let me = me.lock().unwrap().clone();
                handle_send_files(
                    &me,
                    &registry,
                    &event_tx,
                    sendings.clone(),
                    &peer_id,
                    paths,
                );
            }
            UiCommand::RespondRequest {
                req_id,
                accept,
                save_dir,
            } => {
                server::respond(&state, &req_id, accept, save_dir);
            }
            UiCommand::CancelTransfer { transfer_id } => {
                // 优先查发送登记；否则按接收会话取消
                let reg = sendings.lock().unwrap().remove(&transfer_id);
                if let Some(reg) = reg {
                    reg.cancel.cancel();
                    if let Some(token) = reg.token {
                        let base = reg.base.clone();
                        tokio::spawn(async move {
                            let client = reqwest::Client::new();
                            client::notify_cancel(&client, &base, &token).await;
                        });
                    }
                } else {
                    server::cancel_local(&state, &transfer_id);
                }
            }
            UiCommand::ConnectPeer { host } => {
                handle_connect_peer(&registry, &event_tx, host);
            }
            UiCommand::Rename { name } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    continue;
                }
                let renamed = {
                    let mut m = me.lock().unwrap();
                    if m.name != name {
                        m.name = name.clone();
                        true
                    } else {
                        false
                    }
                };
                if renamed {
                    tracing::info!("设备已改名: {name}（下一条 announce 生效）");
                }
            }
            UiCommand::WebOffer { paths } => {
                let state = state.clone();
                let event_tx = event_tx.clone();
                let port = config.http_port;
                tokio::spawn(async move {
                    let files =
                        match tokio::task::spawn_blocking(move || client::collect_files(&paths))
                            .await
                        {
                            Ok(Ok(f)) => f,
                            Ok(Err(e)) => {
                                let _ = event_tx.try_send(CoreEvent::Error {
                                    context: "发送到网页".into(),
                                    message: format!("{e:#}"),
                                });
                                return;
                            }
                            Err(e) => {
                                let _ = event_tx.try_send(CoreEvent::Error {
                                    context: "发送到网页".into(),
                                    message: format!("任务失败: {e}"),
                                });
                                return;
                            }
                        };
                    let _ = web::web_offer(&state, &files);
                    let _ = port;
                });
            }
            UiCommand::WebText { text } => {
                let text = text.trim().to_string();
                if !text.is_empty() {
                    web::web_text(&state, &text);
                }
            }
            UiCommand::Shutdown => break,
        }
    }

    // 收尾：广播 bye、取消进行中的传输
    disc.shutdown.cancel();
    {
        let mut sendings = sendings.lock().unwrap();
        for (_, reg) in sendings.drain() {
            reg.cancel.cancel();
        }
    }
    server::cancel_all(&state);
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
}

fn lookup_peer(registry: &Arc<Mutex<discovery::Registry>>, peer_id: &str) -> Option<Device> {
    let reg = registry.lock().unwrap();
    reg.get(peer_id).filter(|d| d.online).cloned()
}

/// 手动连接：解析 `ip` / `ip:port`，探测 /api/info，成功则登记设备
fn handle_connect_peer(
    registry: &Arc<Mutex<discovery::Registry>>,
    event_tx: &Sender<CoreEvent>,
    host: String,
) {
    let host = host.trim().trim_start_matches("http://").to_string();
    let parsed: std::net::SocketAddr = if host.contains(':') {
        match host.parse() {
            Ok(a) => a,
            Err(_) => {
                let _ = event_tx.try_send(CoreEvent::Error {
                    context: "添加设备".into(),
                    message: "格式不对，应为 IP 或 IP:端口，如 192.168.1.5".into(),
                });
                return;
            }
        }
    } else {
        match format!("{host}:{}", proto::DEFAULT_HTTP_PORT).parse() {
            Ok(a) => a,
            Err(_) => {
                let _ = event_tx.try_send(CoreEvent::Error {
                    context: "添加设备".into(),
                    message: "无效的 IP 地址".into(),
                });
                return;
            }
        }
    };

    let base = format!("http://{parsed}");
    let registry = registry.clone();
    let event_tx = event_tx.clone();
    tokio::spawn(async move {
        match client::probe_info(&base).await {
            Ok(info) => {
                let now = proto::now_ms();
                let up = {
                    let mut reg = registry.lock().unwrap();
                    reg.on_announce(info, parsed.ip(), now)
                };
                // on_announce 未变化时返回 None（设备已在列表），仍给用户成功反馈
                let _ = event_tx.try_send(CoreEvent::Info(format!("已连接 {}", parsed.ip())));
                if let Some(dev) = up {
                    let _ = event_tx.try_send(CoreEvent::DeviceUp(dev));
                }
            }
            Err(e) => {
                let _ = event_tx.try_send(CoreEvent::Error {
                    context: "添加设备".into(),
                    message: format!("{parsed}: {e:#}"),
                });
            }
        }
    });
}

fn handle_send_text(
    me: &DeviceInfo,
    registry: &Arc<Mutex<discovery::Registry>>,
    store: &Arc<Mutex<Store>>,
    event_tx: &Sender<CoreEvent>,
    peer_id: &str,
    text: String,
) {
    let Some(dev) = lookup_peer(registry, peer_id) else {
        let _ = event_tx.try_send(CoreEvent::Error {
            context: "发送消息".into(),
            message: "对方不在线或未知".into(),
        });
        return;
    };

    // 本地先入库 + 回显，再异步投递
    let now = proto::now_ms();
    let msg_id = {
        let s = store.lock().unwrap();
        match s.insert_text(peer_id, true, &text, now) {
            Ok(id) => id,
            Err(e) => {
                let _ = event_tx.try_send(CoreEvent::Error {
                    context: "发送消息".into(),
                    message: e.to_string(),
                });
                return;
            }
        }
    };
    let _ = event_tx.try_send(CoreEvent::Message {
        msg: ChatMessage {
            id: msg_id,
            peer_id: peer_id.to_string(),
            outgoing: true,
            kind: MessageKind::Text(text.clone()),
            created_at: now,
        },
    });

    let base = dev.http_base();
    let me = me.clone();
    let event_tx = event_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = client::send_text(&base, &me, &text, now).await {
            let _ = event_tx.try_send(CoreEvent::Error {
                context: "发送消息".into(),
                message: format!("发送到 {} 失败：{e:#}", base),
            });
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn handle_send_files(
    me: &DeviceInfo,
    registry: &Arc<Mutex<discovery::Registry>>,
    event_tx: &Sender<CoreEvent>,
    sendings: client::Sendings,
    peer_id: &str,
    paths: Vec<std::path::PathBuf>,
) {
    let Some(dev) = lookup_peer(registry, peer_id) else {
        let _ = event_tx.try_send(CoreEvent::Error {
            context: "发送文件".into(),
            message: "对方不在线或未知".into(),
        });
        return;
    };

    let transfer_id = uuid::Uuid::new_v4().to_string();
    let cancel = CancellationToken::new();
    sendings.lock().unwrap().insert(
        transfer_id.clone(),
        client::SendReg {
            cancel: cancel.clone(),
            base: dev.http_base(),
            token: None,
        },
    );

    let base = dev.http_base();
    let peer_id = peer_id.to_string();
    let peer_name = dev.info.name.clone();
    let me = me.clone();
    let event_tx = event_tx.clone();
    let tid = transfer_id.clone();
    tokio::spawn(async move {
        // 文件展开是同步 IO，放阻塞线程池
        let files = match tokio::task::spawn_blocking(move || client::collect_files(&paths)).await
        {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => {
                let _ = event_tx.try_send(CoreEvent::Error {
                    context: "发送文件".into(),
                    message: format!("{e:#}"),
                });
                let _ = event_tx.try_send(CoreEvent::TransferFinished {
                    transfer_id: tid,
                    error: Some(e.to_string()),
                });
                return;
            }
            Err(e) => {
                let _ = event_tx.try_send(CoreEvent::Error {
                    context: "发送文件".into(),
                    message: format!("任务失败: {e}"),
                });
                return;
            }
        };
        client::send_files(base, me, files, tid, peer_id, peer_name, event_tx, cancel, sendings)
            .await;
    });
}
