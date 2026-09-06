//! LocalTransfer 协议 v1：数据模型与 UI↔Core 事件定义
//!
//! - 设备发现：UDP 多播（见 discovery.rs），载荷为 [`DiscoveryPacket`] 的 JSON
//! - 传输：HTTP REST（见 server.rs / client.rs）

use std::{collections::HashMap, net::IpAddr, path::PathBuf};

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// 网页会话的伪 peer_id（浏览器聊天在桌面端呈现为一个固定会话）
pub const WEB_PEER_ID: &str = "__web__";

/// 多播组（组织本地范围，避开 LocalSend 的 224.0.0.167）
pub const DISCOVERY_GROUP: std::net::Ipv4Addr = std::net::Ipv4Addr::new(239, 192, 71, 82);
/// 发现端口固定，与 HTTP 端口解耦（同机双实例测试时 HTTP 端口不同也能互相发现）。
/// 17878 避开 Windows 动态端口段 49152+（该段常被 Hyper-V/WSL 随机保留导致绑定失败）
pub const DISCOVERY_PORT: u16 = 17878;
pub const DEFAULT_HTTP_PORT: u16 = 17878;

/// 心跳/离线判定（announce 周期：前 6s 每 2s，之后每 10s；超时即判离线）
pub const DEVICE_TIMEOUT_SECS: u64 = 15;

// ---------------------------------------------------------------------------
// 发现协议
// ---------------------------------------------------------------------------

/// UDP 多播数据报（UTF-8 JSON，<1KB）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum DiscoveryPacket {
    Announce {
        #[serde(flatten)]
        info: DeviceInfo,
    },
    Bye {
        id: String,
    },
}

/// 设备身份信息（announce 与各 HTTP 请求体里都携带）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    /// windows / mac / linux
    pub plat: String,
    /// HTTP 服务端口
    pub port: u16,
    #[serde(default)]
    pub v: u32,
}

/// 设备注册表条目（UI 看到的设备）
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub info: DeviceInfo,
    pub addr: IpAddr,
    pub online: bool,
    pub last_seen_ms: i64,
}

impl Device {
    pub fn http_base(&self) -> String {
        format!("http://{}:{}", self.addr, self.info.port)
    }
}

// ---------------------------------------------------------------------------
// HTTP 请求/响应体
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MessageBody {
    pub sender: DeviceInfo,
    pub text: String,
    pub sent_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileMeta {
    pub id: String,
    /// 显示名
    pub name: String,
    /// 相对保存目录的路径（文件夹传输时含子目录），收发双方都必须净化
    pub rel_path: String,
    pub size: u64,
}

impl FileMeta {
    /// rel_path 只允许是安全的相对路径：不允许绝对路径、根斜杠、`..`、盘符
    pub fn is_rel_path_safe(rel_path: &str) -> bool {
        let p = std::path::Path::new(rel_path);
        if p.is_absolute() || rel_path.is_empty() {
            return false;
        }
        // Windows 上 "/x" 不算 is_absolute，但有 RootDir 组件，同样拒绝
        !p.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
            )
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PrepareBody {
    pub sender: DeviceInfo,
    pub files: Vec<FileMeta>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PrepareOk {
    pub token: String,
}

// ---------------------------------------------------------------------------
// 聊天消息（存储 & UI 展示共用）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub id: i64,
    pub peer_id: String,
    /// true = 我发出
    pub outgoing: bool,
    pub kind: MessageKind,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageKind {
    Text(String),
    File {
        name: String,
        size: u64,
        /// 接收完成后才有（接收方保存位置 / 发送方为空）
        saved_path: Option<PathBuf>,
        transfer_id: Option<String>,
        /// 发送方分配的单文件 ID，用于匹配实时进度
        file_id: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Core → UI 事件（唯一出口，经 smol channel）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum CoreEvent {
    DeviceUp(Device),
    DeviceDown {
        id: String,
        name: String,
    },
    /// 文本消息（收到的，或本地发出已入库的）
    Message {
        msg: ChatMessage,
    },
    /// 收到传输请求，等待 UI 确认
    IncomingRequest {
        req_id: String,
        peer: Device,
        files: Vec<FileMeta>,
    },
    /// 传输请求等待确认超时（UI 应关掉对应弹窗）
    RequestExpired {
        req_id: String,
    },
    /// 传输会话建立（接收方接受后 / 发送方拿到 token 后）
    TransferStarted {
        transfer_id: String,
        peer_id: String,
        peer_name: String,
        outgoing: bool,
        files: Vec<FileMeta>,
        /// 发送方本地源路径（与 files 一一对应；接收方为空，
        /// 接收路径由 FileProgress 完成事件带回）
        sources: Vec<PathBuf>,
        save_dir: Option<PathBuf>,
    },
    /// 单文件进度（done=true 表示该文件完成，接收方携带最终落盘路径）
    FileProgress {
        transfer_id: String,
        file_id: String,
        transferred: u64,
        total: u64,
        done: bool,
        path: Option<std::path::PathBuf>,
    },
    /// 整个传输结束（error=None 成功；Some 为取消/失败原因）
    TransferFinished {
        transfer_id: String,
        error: Option<String>,
    },
    Error {
        context: String,
        message: String,
    },
    Info(String),
}

// ---------------------------------------------------------------------------
// UI → Core 命令
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum UiCommand {
    SendText {
        peer_id: String,
        text: String,
    },
    SendFiles {
        peer_id: String,
        paths: Vec<PathBuf>,
    },
    /// 回应接收请求
    RespondRequest {
        req_id: String,
        accept: bool,
        save_dir: Option<PathBuf>,
    },
    CancelTransfer {
        transfer_id: String,
    },
    /// 改设备名（更新对外广播的身份，立即生效）
    Rename {
        name: String,
    },
    /// 把文件发布到网页接收页（浏览器打开 http://本机IP:端口/ 下载）
    WebOffer {
        paths: Vec<PathBuf>,
    },
    /// 发文本到网页会话（选中"网页"聊天时）
    WebText {
        text: String,
    },
    /// 手动连接设备：`ip` 或 `ip:port`（多播/广播都不通时的兜底）
    ConnectPeer {
        host: String,
    },
    Shutdown,
}

/// 进度节流器：避免进度事件淹没 UI（默认 150ms 一次）
pub struct ProgressThrottle {
    last_ms: i64,
    interval_ms: i64,
}

impl ProgressThrottle {
    pub fn new(interval_ms: i64) -> Self {
        Self {
            last_ms: 0,
            interval_ms,
        }
    }

    pub fn allow(&mut self) -> bool {
        let now = now_ms();
        if now - self.last_ms >= self.interval_ms {
            self.last_ms = now;
            true
        } else {
            false
        }
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 把字节数格式化为人类可读（UI 备用，核心不做格式化也行）
pub fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

/// 供测试/UI 使用的占位：HashMap 别名
pub type ProgressMap = HashMap<String, (u64, u64)>;
