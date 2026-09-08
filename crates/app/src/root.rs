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
    pub incoming: Option<IncomingReq>,

    pub input: Entity<TextareaState>,
    pub toast: Option<(String, Instant, bool)>, // (文本, 时间, is_error)

    /// 聊天消息流的虚拟列表状态（尾部跟随，新消息自动滚到底）
    pub scroller: Entity<MessageScrollerState>,

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
            incoming: None,
            input,
            toast: None,
            scroller,
            confirm_clear: None,
            show_settings: false,
            set_name,
            set_dir,
            set_port,
            show_connect: false,
            connect_input,
            show_web_qr: false,
            autostart: autostart_enabled(),
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
            }
            CoreEvent::Message { msg } => {
                let peer = msg.peer_id.clone();
                let is_selected = self.selected.as_deref() == Some(peer.as_str());
                if !is_selected {
                    *self.unread.entry(peer.clone()).or_insert(0) += 1;
                }
                self.append_message(peer, msg);
            }
            CoreEvent::IncomingRequest {
                req_id,
                peer,
                files,
            } => {
                // 同一时间只展示最新请求；旧的按拒绝处理（对方收到 403）
                if let Some(old) = self.incoming.take() {
                    let _ = self.core.send(UiCommand::RespondRequest {
                        req_id: old.req_id,
                        accept: false,
                        save_dir: None,
                    });
                }
                // 自动接收：直接接受，不进会话卡片
                if self.me.auto_receive {
                    let _ = self.core.send(UiCommand::RespondRequest {
                        req_id: req_id.clone(),
                        accept: true,
                        save_dir: None,
                    });
                    self.toast(
                        format!("自动接收 {} 的 {} 个文件", peer.info.name, files.len()),
                        false,
                    );
                } else {
                    let peer_id = peer.info.id.clone();
                    self.incoming = Some(IncomingReq { req_id, peer, files });
                    // 请求卡片渲染在对应会话里——切过去让用户立刻看到
                    self.select_peer(&peer_id, cx);
                }
            }
            CoreEvent::RequestExpired { req_id } => {
                // 只关掉对应的弹窗（新请求可能已经顶掉了旧的）
                if self.incoming.as_ref().is_some_and(|r| r.req_id == req_id) {
                    self.incoming = None;
                }
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

    /// 当前会话的渲染行数：分组后的消息 + （有挂起请求时）1 张请求卡片
    pub(crate) fn chat_row_count(&self, peer: &str) -> usize {
        let groups = self
            .chats
            .get(peer)
            .map(|msgs| crate::chat::group_count(msgs))
            .unwrap_or(0);
        let extra = if self
            .incoming
            .as_ref()
            .is_some_and(|r| r.peer.info.id == peer)
        {
            1
        } else {
            0
        };
        groups + extra
    }

    fn append_message(&mut self, peer: String, msg: ChatMessage) {
        self.chats.entry(peer).or_default().push(msg);
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
                    .child(self.render_chat(window, cx)),
            )
            .children(self.render_overlays(window, cx))
            .children(self.render_drop_overlay(cx))
    }
}

pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
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
