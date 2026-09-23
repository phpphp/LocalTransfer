//! 根视图：持有全部 UI 状态，泵送核心事件，分发布局

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::component::{h_flex, v_flex, Icon, IconName, Sizable as _, TitleBar};
use gpui_kit::*;
use transfer_core::{
    ChatMessage, Config, CoreEvent, CoreHandle, Device, FileMeta, MessageKind, Store, UiCommand,
};

/// 一个进行中/刚结束的传输（UI 侧状态）
pub struct TransferState {
    pub id: String,
    /// 对端 id（收发两个方向都用它来判断属于哪个会话）
    pub peer_id: String,
    pub files: Vec<TransferFile>,
    pub error: Option<String>,
    /// 速度采样
    last_sample: Option<(Instant, u64)>,
    pub speed_bps: f64,
}

pub struct TransferFile {
    pub meta: FileMeta,
    pub transferred: u64,
    pub done: bool,
    pub saved_path: Option<PathBuf>,
    /// 发送方的本地源路径（接收方为 None，落盘路径见 saved_path）
    pub source: Option<PathBuf>,
}

impl TransferState {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.meta.size).sum()
    }

    pub fn transferred_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.transferred).sum()
    }

    pub fn all_done(&self) -> bool {
        self.files.iter().all(|f| f.done)
    }
}

/// 已完成传输的"打开目标"（reveal_path 用：资源管理器打开上一级并选中；
/// 路径数学在 transfer-core，已有单元测试）：接收方用落盘路径，发送方用源路径。
pub(crate) fn transfer_open_target(
    transfers: &[TransferState],
    transfer_id: Option<&str>,
    file_id: Option<&str>,
) -> Option<PathBuf> {
    let t = transfers.iter().find(|t| Some(t.id.as_str()) == transfer_id)?;
    let f = match file_id {
        Some(fid) => t.files.iter().find(|f| f.meta.id == fid)?,
        None => t
            .files
            .iter()
            .find(|f| f.saved_path.is_some() || f.source.is_some())?,
    };
    transfer_core::client::open_target(
        &f.meta.rel_path,
        f.saved_path.as_deref(),
        f.source.as_deref(),
    )
}

/// 一次传输的显示名（文件夹名 / "N 个文件"，规则与测试在 transfer-core）
pub(crate) fn transfer_display_name(files: &[TransferFile]) -> String {
    let rels: Vec<&str> = files.iter().map(|f| f.meta.rel_path.as_str()).collect();
    transfer_core::client::batch_display_name(&rels)
}

/// 自绘首字母头像（取设备名第一个字）。不用组件库 Avatar 的原因：
/// 它的 Size::Size(px) 分支只把文字容器缩到一半大小，不设字号/行高，
/// 默认行高会把字沉到下沿。
pub fn initial_avatar(name: &str, size: f32, cx: &App) -> Div {
    use gpui_kit::component::{ActiveTheme as _, Colorize as _};
    let short: String = name.chars().take(1).collect();
    // 与组件库同款配色：按名字哈希取 24 档色相
    let hue = (hash(&short) % 24) as f32 * 15.0 / 360.0;
    let color = cx.theme().blue.hue(hue);
    let fs = size * 0.42;
    div()
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(color.opacity(0.2))
        .text_color(color)
        .text_size(px(fs))
        .line_height(px(fs))
        .font_weight(FontWeight::SEMIBOLD)
        .child(short)
}

// ---------------------------------------------------------------- 开机启动
// HKCU\...\Run 注册当前 exe 路径（per-user，无需管理员）

const AUTOSTART_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_NAME: &str = "LocalTransfer";

#[cfg(target_os = "windows")]
pub fn autostart_enabled() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(AUTOSTART_KEY)
        .ok()
        .and_then(|k| k.get_value::<String, _>(AUTOSTART_NAME).ok())
        .is_some()
}

#[cfg(target_os = "windows")]
pub fn set_autostart(on: bool) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;
    let run = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(AUTOSTART_KEY, KEY_READ | KEY_WRITE)
        .context("打开注册表 Run 键失败")?;
    if on {
        let exe = std::env::current_exe().context("取程序路径失败")?;
        run.set_value(AUTOSTART_NAME, &exe.to_string_lossy().to_string())
            .context("写入开机启动失败")?;
    } else {
        match run.delete_value(AUTOSTART_NAME) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("移除开机启动失败"),
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn autostart_enabled() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn set_autostart(_on: bool) -> anyhow::Result<()> {
    anyhow::bail!("此平台不支持开机启动")
}

// ---------------------------------------------------------------- 系统通知
// 仿 QQ/微信的提醒：窗口不在前台时弹 Windows 通知（toast）。
// 未打包的 Win32 程序必须先注册自己的 AUMID（HKCU\Software\Classes\AppUserModelId），
// 否则 Win10/11 会静默丢弃 toast——之前借 PowerShell 的 AUMID 收不到就是这个原因。
// show() 内含 COM/WinRT 调用 + sleep，丢后台线程跑。

#[cfg(target_os = "windows")]
const AUMID: &str = "LocalTransfer.App";

#[cfg(target_os = "windows")]
pub fn desktop_notify(title: &str, body: &str) {
    use std::sync::OnceLock;
    static AUMID_READY: OnceLock<bool> = OnceLock::new();
    let ready = *AUMID_READY.get_or_init(|| {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = hkcu
            .create_subkey(r"Software\Classes\AppUserModelId\LocalTransfer.App")
            .map(|(k, _)| k);
        match key {
            Ok(k) => {
                let _ = k.set_value("DisplayName", &"LocalTransfer");
                // exe 旁带 icon.ico 时用它（安装包/绿色包会附带）
                let ico = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("icon.ico")))
                    .filter(|p| p.exists());
                if let Some(ico) = ico {
                    let uri = format!("file:///{}", ico.to_string_lossy().replace('\\', "/"));
                    let _ = k.set_value("IconUri", &uri);
                }
                true
            }
            Err(e) => {
                tracing::warn!("AUMID 注册失败（通知可能不显示）: {e}");
                false
            }
        }
    });
    if !ready {
        return;
    }
    let t = title.to_string();
    let b = body.to_string();
    std::thread::spawn(move || {
        use tauri_winrt_notification::Toast;
        if let Err(e) = Toast::new(AUMID).title(&t).text1(&b).show() {
            tracing::warn!("系统通知失败: {e}");
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub fn desktop_notify(_title: &str, _body: &str) {}

/// 待确认的接收请求（会话内的请求卡片数据）
#[derive(Clone)]
pub struct IncomingReq {
    pub req_id: String,
    pub peer: Device,
    pub files: Vec<FileMeta>,
}

pub struct RootView {
    pub me: Config,
    pub core: CoreHandle,
    pub store: Arc<Mutex<Store>>,
    /// 本机局域网 IP（启动时取一次，侧栏展示用）
    pub local_ip: Option<String>,

    pub devices: Vec<Device>,
    /// 已知设备名（含离线/历史）
    pub peer_names: HashMap<String, String>,

    pub selected: Option<String>,
    pub chats: HashMap<String, Vec<ChatMessage>>,
    /// 已从库里读过历史的会话。事件流会先把新消息塞进 chats，
    /// 只看 chats 是否存在会导致这些会话永远读不到历史。
    pub loaded: std::collections::HashSet<String>,
    pub unread: HashMap<String, usize>,

    /// 活动传输（按 transfer_id），完成后保留至用户关闭或会话切换
    pub transfers: Vec<TransferState>,
    /// 待确认的接收请求队列（可同时挂多个：新请求不再顶掉/拒绝旧的）
    pub incoming: Vec<IncomingReq>,

    pub input: Entity<TextareaState>,
    pub toast: Option<(String, Instant, bool)>, // (文本, 时间, is_error)

    /// 聊天消息流的虚拟列表状态（尾部跟随，新消息自动滚到底）
    pub scroller: Entity<MessageScrollerState>,

    /// 文本消息的可选中渲染状态（msg.id → TextViewState；懒建缓存）
    pub text_views: HashMap<i64, Entity<gpui_kit::component::text::TextViewState>>,

    /// 清空聊天记录的两段确认状态（peer, 时间）
    pub confirm_clear: Option<(String, Instant)>,

    /// 设置弹窗
    pub show_settings: bool,
    pub set_name: Entity<InputState>,
    pub set_dir: Entity<InputState>,
    pub set_port: Entity<InputState>,

    /// 手动添加设备弹窗
    pub show_connect: bool,
    pub connect_input: Entity<InputState>,

    /// 网页客户端二维码弹窗
    pub show_web_qr: bool,

    /// 开机启动（HKCU Run 键，启动时读一次）
    pub autostart: bool,

    /// 侧栏当前宽度（拖拽调宽，clamp 180~460）
    pub sidebar_w: f32,
    /// 拖拽中：(按下时鼠标 x, 按下时侧栏宽)
    pub sidebar_drag: Option<(f32, f32)>,

    /// 覆盖确认弹窗中的请求（接收前发现同名文件）
    pub overwrite_req: Option<IncomingReq>,

    /// 窗口是否激活（渲染时缓存；仅后台时弹系统通知）
    pub window_active: bool,

    /// 关闭确认弹窗（close_action=ask 时点关闭按钮）
    pub show_close_dialog: bool,
    /// 关闭弹窗里勾了"记住我的选择"
    pub close_remember: bool,
    /// 跳过询问直接关（关闭弹窗里选了"退出"）
    pub force_close: bool,

    _keep: Vec<Subscription>,
}

impl RootView {
    pub fn new(
        me: Config,
        store: Store,
        core: CoreHandle,
        event_rx: smol::channel::Receiver<CoreEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = Arc::new(Mutex::new(store));
        // 多行输入：Enter 发送，Shift+Enter 换行；1~5 行自适应高度
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("输入消息，Enter 发送，Shift+Enter 换行")
                .submit_on_enter(true)
                .auto_grow(1, 5)
        });
        let set_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("设备名")
                .default_value(me.device_name.clone())
        });
        let set_dir = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("下载目录")
                .default_value(me.download_dir.to_string_lossy().to_string())
        });
        let set_port = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("端口")
                .default_value(me.http_port.to_string())
        });
        let connect_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("对方 IP，如 192.168.1.5（可带 :端口）")
        });
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));

        // 初始数据
        let peer_names = store.lock().unwrap().known_peer_names().unwrap_or_default();

        // 事件泵：核心线程 → UI（实体释放后 update 返回 Err，循环自然结束）
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = event_rx.recv().await {
                this.update(cx, |this, cx| this.on_core_event(ev, cx)).ok();
            }
        })
        .detach();

        // 回车发送（submit_on_enter 模式下 Shift+Enter 只换行、不触发该事件）
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut RootView, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { shift: false, .. }) {
                    this.send_current_input(window, cx);
                }
            },
        );

        let view = Self {
            me,
            core,
            store,
            local_ip: transfer_core::local_ip_hint().map(|ip| ip.to_string()),
            devices: Vec::new(),
            peer_names,
            selected: None,
            chats: HashMap::new(),
            loaded: std::collections::HashSet::new(),
            unread: HashMap::new(),
            transfers: Vec::new(),
            incoming: Vec::new(),
            input,
            toast: None,
            scroller,
            text_views: HashMap::new(),
            confirm_clear: None,
            show_settings: false,
            set_name,
            set_dir,
            set_port,
            show_connect: false,
            connect_input,
            show_web_qr: false,
            autostart: autostart_enabled(),
            sidebar_w: crate::sidebar::SIDEBAR_W,
            sidebar_drag: None,
            overwrite_req: None,
            window_active: true,
            show_close_dialog: false,
            close_remember: false,
            force_close: false,
            _keep: vec![sub],
        };
        view
    }

    pub fn select_peer(&mut self, peer_id: &str, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(peer_id) {
            return;
        }
        self.selected = Some(peer_id.to_string());
        self.unread.insert(peer_id.to_string(), 0);
        self.sync_tray_badge();
        // 首次进入该会话时读历史；事件流先塞进来的那几条同样已入库，
        // 历史里已包含，直接覆盖即可
        if self.loaded.insert(peer_id.to_string()) {
            let history = self
                .store
                .lock()
                .unwrap()
                .load_history(peer_id, 300)
                .unwrap_or_default();
            self.chats.insert(peer_id.to_string(), history);
        }
        // 行数对齐（reset 吸底）
        let count = self.chat_row_count(peer_id);
        self.scroller.update(cx, |s, cx| s.reset(count, cx));
        cx.notify();
    }

    pub fn peer_name(&self, id: &str) -> String {
        if let Some(d) = self.devices.iter().find(|d| d.info.id == id) {
            return d.info.name.clone();
        }
        self.peer_names
            .get(id)
            .cloned()
            .unwrap_or_else(|| short_id(id))
    }

    pub fn toast(&mut self, text: impl Into<String>, is_error: bool) {
        self.toast = Some((text.into(), Instant::now(), is_error));
    }

    pub fn send_current_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(peer) = self.selected.clone() else {
            return;
        };
        let text = self.input.read(cx).value().to_string();
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        // 网页会话走专用通道（没有对端设备可寻址）
        let cmd = if peer == transfer_core::proto::WEB_PEER_ID {
            UiCommand::WebText { text }
        } else {
            UiCommand::SendText { peer_id: peer, text }
        };
        if let Err(e) = self.core.send(cmd) {
            self.toast(format!("发送失败: {e}"), true);
        }
        self.input
            .update(cx, |s, cx| s.set_value("", window, cx));
    }

    // -----------------------------------------------------------------------
    // 核心事件处理
    // -----------------------------------------------------------------------

    fn on_core_event(&mut self, ev: CoreEvent, cx: &mut Context<Self>) {
        match ev {
            CoreEvent::DeviceUp(dev) => {
                let name = dev.info.name.clone();
                let id = dev.info.id.clone();
                match self.devices.iter_mut().find(|d| d.info.id == id) {
                    Some(slot) => *slot = dev,
                    None => self.devices.push(dev),
                }
                // 上线设备的未读恢复计数（托盘可能重新亮起）
                self.sync_tray_badge();
                self.devices.sort_by(|a, b| b.last_seen_ms.cmp(&a.last_seen_ms));
                if let Ok(s) = self.store.lock() {
                    let _ = s.upsert_peer(&id, &name);
                }
                // 侧栏只列在线设备，所以默认选中第一台上线的设备，
                // 否则启动后右侧一直是空的
                if self.selected.is_none() {
                    self.select_peer(&id, cx);
                }
                self.peer_names.insert(id, name);
            }
            CoreEvent::DeviceDown { id, .. } => {
                if let Some(d) = self.devices.iter_mut().find(|d| d.info.id == id) {
                    d.online = false;
                }
                // 离线设备的未读不再计入托盘（侧栏看不到、点不掉）
                self.sync_tray_badge();
            }
            CoreEvent::Message { msg } => {
                let peer = msg.peer_id.clone();
                let is_selected = self.selected.as_deref() == Some(peer.as_str());
                // 实时查前台（渲染缓存的 window_active 切走后不刷新，永远 true）
                let foreground = crate::tray::is_foreground();
                // 未读 = 不在对应会话，或窗口在后台（后台收到的消息都算未读——
                // 否则"藏到托盘+会话选中"的场景托盘永不闪、毫无提醒）
                if !is_selected || !foreground {
                    *self.unread.entry(peer.clone()).or_insert(0) += 1;
                    self.sync_tray_badge();
                }
                // 后台时弹系统通知 + 任务栏按钮闪烁（QQ/微信式提醒；
                // 托盘未读红点闪烁由 sync_tray_badge 驱动）
                if !foreground {
                    let name = self.peer_name(&peer);
                    if let MessageKind::Text(t) = &msg.kind {
                        let preview: String = t.chars().take(40).collect();
                        desktop_notify(&name, &preview);
                    }
                    crate::tray::flash_taskbar();
                }
                self.append_message(peer, msg);
            }
            CoreEvent::IncomingRequest {
                req_id,
                peer,
                files,
            } => {
                if !crate::tray::is_foreground() {
                    desktop_notify(
                        &format!("{} 想发送文件", peer.info.name),
                        &format!("{} 个文件 · 点击处理", files.len()),
                    );
                    crate::tray::flash_taskbar();
                    self.sync_tray_badge();   // 请求卡也点亮托盘红点
                }
                // 多个待确认请求并存入队（旧的不再被新请求顶掉/拒绝——
                // 曾经新请求直接回绝旧请求 403，对方的卡片秒变"被拒绝"）
                // 自动接收：直接接受，不进会话卡片
                if self.me.auto_receive {
                    let _ = self.core.send(UiCommand::RespondRequest {
                        req_id: req_id.clone(),
                        accept: true,
                        save_dir: None,
                        overwrite: false,
                    });
                    self.toast(
                        format!("自动接收 {} 的 {} 个文件", peer.info.name, files.len()),
                        false,
                    );
                } else {
                    let peer_id = peer.info.id.clone();
                    self.incoming.push(IncomingReq { req_id, peer, files });
                    // 请求卡片渲染在对应会话里——切过去让用户立刻看到
                    self.select_peer(&peer_id, cx);
                }
            }
            CoreEvent::RequestExpired { req_id } => {
                // 只移除对应的请求（发送方取消/超时），其余照常
                self.incoming.retain(|r| r.req_id != req_id);
            }
            CoreEvent::TransferStarted {
                transfer_id,
                peer_id,
                peer_name: _,
                outgoing,
                files,
                sources,
                save_dir,
            } => {
                self.transfers.insert(
                    0,
                    TransferState {
                        id: transfer_id.clone(),
                        peer_id: peer_id.clone(),
                        // 注意不能用 zip：接收方 sources 为空会把整个列表截断成
                        // 0 个文件（"0 个文件 / 打不开"的根因）。按下标配对。
                        files: files
                            .iter()
                            .enumerate()
                            .map(|(i, meta)| {
                                let src = sources
                                    .get(i)
                                    .filter(|p| !p.as_os_str().is_empty());
                                TransferFile {
                                    meta: meta.clone(),
                                    transferred: 0,
                                    done: false,
                                    saved_path: None,
                                    source: src.cloned(),
                                }
                            })
                            .collect(),
                        error: None,
                        last_sample: None,
                        speed_bps: 0.0,
                    },
                );
                let _ = save_dir; // 接收目录在 core 侧使用
                // 传输列表只保留最近 20 条
                self.transfers.truncate(20);

                // 发送方向：聊天里补文件卡片（接收方由 core 入库）。
                // 消息与库里都带上本地源路径——重启后发送方仍能定位文件
                if outgoing {
                    let now = transfer_core::proto::now_ms();
                    let mut msgs = Vec::new();
                    {
                        let store = self.store.lock().unwrap();
                        for (i, f) in files.iter().enumerate() {
                            let src_path = sources
                                .get(i)
                                .filter(|p| !p.as_os_str().is_empty())
                                .map(|p| p.as_path());
                            if let Ok(id) = store.insert_file(
                                &peer_id, true, &f.name, f.size, src_path, Some(&transfer_id),
                                Some(&f.id), now,
                            ) {
                                msgs.push(ChatMessage {
                                    id,
                                    peer_id: peer_id.clone(),
                                    outgoing: true,
                                    kind: MessageKind::File {
                                        name: f.name.clone(),
                                        size: f.size,
                                        saved_path: src_path.map(|p| p.to_path_buf()),
                                        transfer_id: Some(transfer_id.clone()),
                                        file_id: Some(f.id.clone()),
                                    },
                                    created_at: now,
                                });
                            }
                        }
                    }
                    let chat = self.chats.entry(peer_id.clone()).or_default();
                    chat.extend(msgs);
                }
            }
            CoreEvent::FileProgress {
                transfer_id,
                file_id,
                transferred,
                total,
                done,
                path,
            } => {
                let now = Instant::now();
                if let Some(t) = self.transfers.iter_mut().find(|t| t.id == transfer_id) {
                    if let Some(f) = t.files.iter_mut().find(|f| f.meta.id == file_id) {
                        f.transferred = transferred.min(total.max(transferred));
                        f.done = done;
                        if done {
                            f.saved_path = path;
                        }
                    }
                    // 全局速度采样（按传输累计字节）
                    let sum: u64 = t.files.iter().map(|f| f.transferred).sum();
                    match t.last_sample {
                        Some((at, prev)) if now.duration_since(at) >= Duration::from_millis(400) => {
                            let dt = now.duration_since(at).as_secs_f64();
                            if dt > 0.0 {
                                t.speed_bps = (sum.saturating_sub(prev)) as f64 / dt;
                            }
                            t.last_sample = Some((now, sum));
                        }
                        None => t.last_sample = Some((now, sum)),
                        _ => {}
                    }
                }
                // 聊天中的文件卡片状态同步（chip 按 transfer_id+file_id 匹配）
                if done {
                    if let Some(peer_id) = self.selected.clone() {
                        let _ = peer_id; // 状态在渲染时实时查 transfers，无需改 chats
                    }
                }
            }
            CoreEvent::TransferFinished { transfer_id, error } => {
                if let Some(t) = self.transfers.iter_mut().find(|t| t.id == transfer_id) {
                    t.error = error;
                    t.speed_bps = 0.0;
                    if t.error.is_none() {
                        // 全部完成
                        for f in t.files.iter_mut() {
                            f.done = true;
                            if f.transferred < f.meta.size {
                                f.transferred = f.meta.size;
                            }
                        }
                        // 接收完成 → 后台时弹系统通知 + 任务栏闪烁
                        if !crate::tray::is_foreground() {
                            let what = crate::root::transfer_display_name(&t.files);
                            let peer_id = t.peer_id.clone();
                            let name = self.peer_name(&peer_id);
                            desktop_notify(&format!("已接收 · {name}"), &what);
                            crate::tray::flash_taskbar();
                        }
                    }
                }
            }
            CoreEvent::Error { context, message } => {
                self.toast(format!("{context}：{message}"), true);
            }
            CoreEvent::Info(text) => self.toast(text, false),
        }
        self.sync_scroller(cx);
        // 核心事件必须显式请求重绘：Entity::update 本身不会触发渲染，
        // 少了这一行时进度/消息/设备上下线都只能等鼠标移动等其他事件顺带刷出来
        // （表现为"发送方不显示进度，过一会儿才突然出现"）。
        cx.notify();
    }

    /// 把消息流的虚拟列表行数对齐到当前状态（分组消息数 + 请求卡片）。
    /// 追加走 append（尾部跟随保持吸底），减少走 reset（重置也吸底）。
    /// 在每个核心事件后调用，幂等。
    pub(crate) fn sync_scroller(&mut self, cx: &mut Context<Self>) {
        let Some(peer) = self.selected.clone() else {
            return;
        };
        let count = self.chat_row_count(&peer);
        let known = self.scroller.read(cx).item_count();
        if count == known {
            return;
        }
        if count > known {
            self.scroller
                .update(cx, |s, cx| s.append(count - known, cx));
        } else {
            self.scroller.update(cx, |s, cx| s.reset(count, cx));
        }
    }

    /// 当前会话的渲染行数：分组后的消息 + 该设备的每张挂起请求卡片
    pub(crate) fn chat_row_count(&self, peer: &str) -> usize {
        let groups = self
            .chats
            .get(peer)
            .map(|msgs| crate::chat::group_count(msgs))
            .unwrap_or(0);
        let extra = self
            .incoming
            .iter()
            .filter(|r| r.peer.info.id == peer)
            .count();
        groups + extra
    }

    fn append_message(&mut self, peer: String, msg: ChatMessage) {
        self.chats.entry(peer).or_default().push(msg);
    }

    /// 接收按钮（按 req_id）：预检下载目录同名冲突——有则弹覆盖确认，无则直接接收
    pub fn ask_overwrite(&mut self, req_id: &str, cx: &mut Context<Self>) {
        let Some(req) = self.incoming.iter().find(|r| r.req_id == req_id).cloned() else {
            return;
        };
        let conflict = req
            .files
            .iter()
            .any(|f| self.me.download_dir.join(&f.rel_path).exists());
        if conflict {
            self.overwrite_req = Some(req);
        } else {
            self.accept_request(req, false, cx);
        }
        cx.notify();
    }

    /// 关闭弹窗的按钮动作（remember 在 render_close_dialog 里由 Checkbox 回调维护）
    pub fn apply_close_choice(&mut self, action: &str, cx: &mut Context<Self>) {
        if self.close_remember {
            self.me.close_action = action.to_string();
            if let Err(e) = self.me.save() {
                self.toast(format!("保存设置失败: {e}"), true);
            }
        }
        self.show_close_dialog = false;
        if action == "tray" {
            crate::tray::hide_main_window();
        }
        cx.notify();
    }

    /// 关闭按钮：按设置分流（tray=隐藏到托盘；close=真关；ask=弹窗问）
    pub fn handle_close_request(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        if self.force_close {
            return true;
        }
        match self.me.close_action.as_str() {
            "tray" => {
                crate::tray::hide_main_window();
                false
            }
            "close" => true,
            // 默认 ask
            _ => {
                self.show_close_dialog = true;
                _cx.notify();
                false
            }
        }
    }

    /// 同步托盘未读标记（unread 汇总变化后调用）。
    /// 只统计"看得到/点得到"的会话：在线设备 + 网页会话——
    /// 侧栏只显示在线设备，离线设备的未读无处可点（永久清不掉），
    /// 计入的话托盘会永远闪（用户报"没未读了还在闪"的根因）；
    /// 设备重新上线后其未读恢复计数，点开即清
    pub fn sync_tray_badge(&self) {
        let web = transfer_core::proto::WEB_PEER_ID;
        let visible_unread = self.unread.iter().any(|(id, &n)| {
            n > 0
                && (id == web || self.devices.iter().any(|d| &d.info.id == id && d.online))
        });
        crate::tray::set_unread(visible_unread);
    }

    /// 确认接收（overwrite=true 时同名文件直接覆盖，否则自动改名避让）
    pub fn accept_request(
        &mut self,
        req: IncomingReq,
        overwrite: bool,
        cx: &mut Context<Self>,
    ) {
        let _ = self.core.send(UiCommand::RespondRequest {
            req_id: req.req_id.clone(),
            accept: true,
            save_dir: None,
            overwrite,
        });
        self.incoming.retain(|r| r.req_id != req.req_id);
        self.overwrite_req = None;
        self.sync_scroller(cx);
        cx.notify();
    }

    /// 删除消息（右键菜单）：UI 直写 WAL 库（与 core 并发安全），
    /// 同时清会话缓存。批次卡片传整组的 id。
    pub fn delete_messages(&mut self, peer: &str, ids: &[i64], cx: &mut Context<Self>) {
        if let Ok(s) = self.store.lock() {
            if let Err(e) = s.delete_messages(ids) {
                tracing::warn!("删除消息失败: {e:#}");
            }
        }
        if let Some(msgs) = self.chats.get_mut(peer) {
            msgs.retain(|m| !ids.contains(&m.id));
        }
        // 同步清掉可选中渲染状态缓存
        self.text_views.retain(|id, _| !ids.contains(id));
        self.sync_scroller(cx);
        cx.notify();
    }

    /// OS 文件拖放的统一入口（root 与聊天面板都会调用；gpui 会在第一个
    /// 处理者里 take 掉 active_drag，所以不会重复发送）
    pub fn handle_external_drop(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        tracing::info!("收到 OS 文件拖放：{} 项", paths.len());
        if paths.is_empty() {
            return;
        }
        match self.selected.clone() {
            Some(peer) if peer == transfer_core::proto::WEB_PEER_ID => {
                // 网页会话：发布到网页接收
                if let Err(e) = self.core.send(UiCommand::WebOffer { paths }) {
                    self.toast(format!("发布失败: {e}"), true);
                }
            }
            Some(peer) => {
                if let Err(e) = self.core.send(UiCommand::SendFiles {
                    peer_id: peer,
                    paths,
                }) {
                    self.toast(format!("发送失败: {e}"), true);
                }
            }
            None => self.toast("请先在左侧选择目标设备", true),
        }
        cx.notify();
    }

    /// 拖拽中的全窗遮罩：只在有活动拖拽时存在，所以不会挡住平时的点击。
    /// 它自己带 on_drop —— 遮罩是最上层且铺满窗口，drop 一定命中它，
    /// 不用赌事件能不能派发到底层某个元素。
    fn render_drop_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use gpui_kit::component::ActiveTheme as _;
        if !cx.has_active_drag() {
            return None;
        }
        let target = self
            .selected
            .as_deref()
            .map(|id| self.peer_name(id))
            .unwrap_or_default();
        let hint = if target.is_empty() {
            "请先在左侧选择目标设备".to_string()
        } else {
            format!("松开即发送给 {target}")
        };

        Some(
            v_flex()
                .id("drop-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .bg(cx.theme().drop_target)
                .border_2()
                .border_dashed()
                .border_color(cx.theme().primary)
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                    this.handle_external_drop(paths.paths().to_vec(), cx);
                }))
                .child(
                    Icon::new(IconName::Inbox)
                        .with_size(px(34.))
                        .text_color(cx.theme().primary),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(hint),
                )
                .into_any_element(),
        )
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 超时清理 toast（3 秒）
        if let Some((_, at, _)) = self.toast {
            if at.elapsed() >= Duration::from_secs(3) {
                self.toast = None;
            }
        }

        v_flex()
            .size_full()
            .bg(self.win_bg(cx))
            // OS 文件拖放 → 转发给当前选中设备。
            // gpui 只把 drop 派发给鼠标命中的 hitbox，父元素挂一份不一定够，
            // 所以聊天面板里还挂了一份（见 chat.rs）。
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                this.handle_external_drop(paths.paths().to_vec(), cx);
            }))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Network)
                                .with_size(px(15.))
                                .text_color(self.primary(cx)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("LocalTransfer"),
                        ),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    // h_flex 默认 items_center：不显式拉伸的话，两栏只拿内容高度并被垂直居中
                    .items_stretch()
                    .overflow_hidden()
                    .child(self.render_sidebar(window, cx))
                    .child(self.sidebar_resizer(cx))
                    .child(self.render_chat(window, cx)),
            )
            .children(self.render_overlays(window, cx))
            .children(self.render_drop_overlay(cx))
    }
}

pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

impl RootView {
    /// 侧栏右缘拖条：按下记起点，move 里改宽（window.on_mouse_event
    /// 是每帧注册的窗口级监听——拖得再快也不会丢事件），松手结束。
    fn sidebar_resizer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = cx.entity();
        div()
            .id("sb-resize")
            .w(px(5.))
            .h_full()
            .flex_none()
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _window, cx| {
                    this.sidebar_drag = Some((ev.position.x.into(), this.sidebar_w));
                    cx.notify();
                }),
            )
            .child(canvas(
                move |_, _, _| {},
                move |_, _, window, _cx| {
                    let handle = handle.clone();
                    window.on_mouse_event(
                        move |ev: &MouseMoveEvent, phase, _window, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            handle.update(cx, |this, cx| {
                                if let Some((start_x, start_w)) = this.sidebar_drag {
                                    if ev.pressed_button == Some(MouseButton::Left) {
                                        this.sidebar_w =
                                            (start_w + f32::from(ev.position.x) - start_x).clamp(180., 460.);
                                        cx.notify();
                                    } else {
                                        // 松手（或按下状态丢失）结束拖拽
                                        this.sidebar_drag = None;
                                        cx.notify();
                                    }
                                }
                            });
                        },
                    );
                },
            ))
    }
}

/// 主题辅助（全部走 gpui-kit 主题，自动适配明暗）
impl RootView {
    pub fn border_color(&self, cx: &Context<Self>) -> Hsla {
        use gpui_kit::component::ActiveTheme;
        cx.theme().border
    }

    pub fn card_bg(&self, cx: &Context<Self>) -> Hsla {
        use gpui_kit::component::ActiveTheme;
        cx.theme().popover
    }

    pub fn fg_muted(&self, cx: &Context<Self>) -> Hsla {
        use gpui_kit::component::ActiveTheme;
        cx.theme().muted_foreground
    }

    /// 主色（图标/强调文字）
    pub fn primary(&self, cx: &Context<Self>) -> Hsla {
        use gpui_kit::component::ActiveTheme;
        cx.theme().primary
    }

    pub fn danger(&self, cx: &Context<Self>) -> Hsla {
        use gpui_kit::component::ActiveTheme;
        cx.theme().danger
    }

    pub fn danger_fg(&self, cx: &Context<Self>) -> Hsla {
        use gpui_kit::component::ActiveTheme;
        cx.theme().danger_foreground
    }

    /// 弹窗遮罩
    pub fn overlay(&self, cx: &Context<Self>) -> Hsla {
        use gpui_kit::component::ActiveTheme;
        cx.theme().overlay
    }

    /// toast 用反色底（明主题里是深色片，暗主题里是浅色片）
    pub fn toast_bg(&self, cx: &Context<Self>) -> Hsla {
        use gpui_kit::component::ActiveTheme;
        cx.theme().foreground
    }

    pub fn toast_fg(&self, cx: &Context<Self>) -> Hsla {
        use gpui_kit::component::ActiveTheme;
        cx.theme().background
    }

    /// 窗口底色（显式设置，避免异常路径下窗口黑屏）
    pub fn win_bg(&self, cx: &Context<Self>) -> Hsla {
        use gpui_kit::component::ActiveTheme;
        cx.theme().background
    }
}


/// 毫秒时间戳 → HH:MM
pub fn fmt_time(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

/// 字节格式化
pub fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

pub fn fmt_speed(bps: f64) -> String {
    if bps <= 0.0 {
        String::new()
    } else {
        format!("{}/s", fmt_size(bps as u64))
    }
}
