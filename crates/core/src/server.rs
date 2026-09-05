//! HTTP 服务端（axum）：文本消息、传输确认、流式接收上传、取消

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::{ConnectInfo, Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post, put},
    Router,
};
use futures::StreamExt;
use smol::channel::Sender;
use tokio_util::sync::CancellationToken;

use crate::discovery::Registry;
use crate::proto::{
    CoreEvent, Device, DeviceInfo, FileMeta, MessageBody, PrepareBody, PrepareOk, ProgressThrottle,
};
use crate::store::Store;

/// 等待接收方确认的超时
pub const PREPARE_TIMEOUT_SECS: u64 = 60;

pub struct Decision {
    pub accept: bool,
    pub save_dir: Option<PathBuf>,
}

pub struct PendingRequest {
    pub peer: Device,
    pub files: Vec<FileMeta>,
    pub responder: tokio::sync::oneshot::Sender<Decision>,
}

/// 接收会话（token 即 transfer_id）
pub struct RecvSession {
    pub peer_id: String,
    pub peer_name: String,
    pub files: Vec<FileMeta>,
    pub save_dir: PathBuf,
    pub cancel: CancellationToken,
    /// 已完成文件数
    pub completed: usize,
    /// file_id → 消息行 id（完成后回填保存路径）
    pub msg_ids: HashMap<String, i64>,
    /// 累计接收字节（僵尸会话检测用）
    pub progress_bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

pub struct ServerState {
    /// 共享身份：改名后所有出口（HTTP sender 字段）都用新名，避免对端
    /// 注册表里名字抖动（announce 新名、消息旧名来回覆盖）
    pub me: Arc<Mutex<DeviceInfo>>,
    pub store: Arc<Mutex<Store>>,
    pub registry: Arc<Mutex<Registry>>,
    pub event_tx: Sender<CoreEvent>,
    pub default_download_dir: PathBuf,
    pub pending: Mutex<HashMap<String, PendingRequest>>,
    pub sessions: Mutex<HashMap<String, RecvSession>>,
    /// 网页接收页的可下载列表（客户端"发送到网页"）
    pub web_offers: Mutex<Vec<crate::web::WebOffer>>,
}

/// 供 web 模块复用的错误响应
pub(crate) fn err_response(status: StatusCode, msg: &str) -> Response {
    (status, msg.to_string()).into_response()
}

pub async fn serve(state: Arc<ServerState>, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/api/info", get(info))
        .route("/api/message", post(message))
        .route("/api/transfer/prepare", post(prepare))
        .route("/api/transfer/upload/{token}/{file_id}", put(upload))
        .route("/api/transfer/cancel/{token}", post(cancel_transfer))
        .route("/", get(crate::web::page))
        .route("/api/web/messages", get(crate::web::messages))
        .route("/api/web/message", post(crate::web::message))
        .route("/api/web/upload", post(crate::web::upload))
        .route("/api/web/list", get(crate::web::list))
        .route("/api/web/download/{id}", get(crate::web::download))
        .route("/api/web/remove/{id}", post(crate::web::remove))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("HTTP 服务监听 0.0.0.0:{port}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, msg.to_string()).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/info
// ---------------------------------------------------------------------------

async fn info(State(st): State<Arc<ServerState>>) -> Json<DeviceInfo> {
    Json(st.me.lock().unwrap().clone())
}

// ---------------------------------------------------------------------------
// POST /api/message
// ---------------------------------------------------------------------------

async fn message(
    State(st): State<Arc<ServerState>>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Json(body): Json<MessageBody>,
) -> Response {
    if body.text.trim().is_empty() || body.sender.id == st.me.lock().unwrap().id {
        return err(StatusCode::BAD_REQUEST, "无效消息");
    }
    note_peer(&st, body.sender.clone(), peer_addr.ip());

    let msg_id = {
        let store = st.store.lock().unwrap();
        match store.insert_text(&body.sender.id, false, &body.text, body.sent_at) {
            Ok(id) => id,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        }
    };
    let _ = st.event_tx.try_send(CoreEvent::Message {
        msg: crate::proto::ChatMessage {
            id: msg_id,
            peer_id: body.sender.id,
            outgoing: false,
            kind: crate::proto::MessageKind::Text(body.text),
            created_at: body.sent_at,
        },
    });
    StatusCode::OK.into_response()
}

/// 未发现的设备直接发来 HTTP 请求时，登记进注册表（保持在线状态）
fn note_peer(st: &ServerState, info: DeviceInfo, addr: IpAddr) {
    let up = {
        let mut reg = st.registry.lock().unwrap();
        reg.on_announce(info, addr, crate::proto::now_ms())
    };
    if let Some(dev) = up {
        let _ = st.event_tx.try_send(CoreEvent::DeviceUp(dev));
    }
}

// ---------------------------------------------------------------------------
// POST /api/transfer/prepare
// ---------------------------------------------------------------------------

async fn prepare(
    State(st): State<Arc<ServerState>>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Json(body): Json<PrepareBody>,
) -> Response {
    if body.files.is_empty() {
        return err(StatusCode::BAD_REQUEST, "文件列表为空");
    }
    if body.sender.id == st.me.lock().unwrap().id {
        return err(StatusCode::BAD_REQUEST, "自己传自己？");
    }
    if body.files.iter().any(|f| !FileMeta::is_rel_path_safe(&f.rel_path)) {
        return err(StatusCode::BAD_REQUEST, "包含不安全的相对路径");
    }
    if body.files.len() > 4096 {
        return err(StatusCode::BAD_REQUEST, "文件数过多");
    }
    note_peer(&st, body.sender.clone(), peer_addr.ip());

    let peer = Device {
        info: body.sender,
        addr: peer_addr.ip(),
        online: true,
        last_seen_ms: crate::proto::now_ms(),
    };
    let peer_id = peer.info.id.clone();
    let peer_name = peer.info.name.clone();

    let req_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<Decision>();
    st.pending.lock().unwrap().insert(
        req_id.clone(),
        PendingRequest {
            peer: peer.clone(),
            files: body.files.clone(),
            responder: tx,
        },
    );
    let _ = st.event_tx.try_send(CoreEvent::IncomingRequest {
        req_id: req_id.clone(),
        peer,
        files: body.files.clone(),
    });

    let decision = match tokio::time::timeout(Duration::from_secs(PREPARE_TIMEOUT_SECS), rx).await
    {
        Ok(Ok(d)) => d,
        _ => {
            // 超时或发送端丢弃
            st.pending.lock().unwrap().remove(&req_id);
            // 通知 UI 关掉弹窗：否则超时后 modal 一直挂着（陈旧请求），
            // 用户下次点确认/拒绝会静默落空，还会干扰后续请求的时序
            let _ = st.event_tx.try_send(CoreEvent::RequestExpired { req_id });
            return err(StatusCode::FORBIDDEN, "等待确认超时");
        }
    };
    st.pending.lock().unwrap().remove(&req_id);

    if !decision.accept {
        return err(StatusCode::FORBIDDEN, "对方拒绝了传输");
    }

    // 落盘准备：确定保存目录、写消息记录、建会话
    let save_dir = decision
        .save_dir
        .unwrap_or_else(|| st.default_download_dir.clone());
    if let Err(e) = crate::store::ensure_dir(&save_dir) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    let token = uuid::Uuid::new_v4().to_string();
    let now = crate::proto::now_ms();
    let mut msg_ids = HashMap::new();
    // 入库的同时收集回显消息：接收方也要在传输过程中就看到卡片和进度，
    // 否则只有数据库有记录、聊天列表要等下次切换会话才刷出来。
    let mut echo: Vec<crate::proto::ChatMessage> = Vec::new();
    {
        let store = st.store.lock().unwrap();
        for f in &body.files {
            match store.insert_file(
                &peer_id, false, &f.name, f.size, None, Some(&token), Some(&f.id), now,
            ) {
                Ok(id) => {
                    msg_ids.insert(f.id.clone(), id);
                    echo.push(crate::proto::ChatMessage {
                        id,
                        peer_id: peer_id.clone(),
                        outgoing: false,
                        kind: crate::proto::MessageKind::File {
                            name: f.name.clone(),
                            size: f.size,
                            saved_path: None,
                            transfer_id: Some(token.clone()),
                            file_id: Some(f.id.clone()),
                        },
                        created_at: now,
                    });
                }
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
    }

    st.sessions.lock().unwrap().insert(
        token.clone(),
        RecvSession {
            peer_id: peer_id.clone(),
            peer_name: peer_name.clone(),
            files: body.files.clone(),
            save_dir: save_dir.clone(),
            cancel: CancellationToken::new(),
            completed: 0,
            msg_ids,
            progress_bytes: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        },
    );
    // 僵尸会话监控：对端断线且未发取消时，30s 无新数据即回收
    {
        let st = st.clone();
        let token = token.clone();
        tokio::spawn(async move {
            let mut last: u64 = 0;
            let mut stall: u32 = 0;
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let dead = {
                    let map = st.sessions.lock().unwrap();
                    match map.get(&token) {
                        None => break,
                        Some(s) => {
                            let b = s
                                .progress_bytes
                                .load(std::sync::atomic::Ordering::Relaxed);
                            if b != last {
                                last = b;
                                stall = 0;
                                false
                            } else {
                                stall += 1;
                                stall >= 6
                            }
                        }
                    }
                };
                if dead {
                    if let Some(sess) = st.sessions.lock().unwrap().remove(&token) {
                        sess.cancel.cancel();
                        let _ = st.event_tx.try_send(CoreEvent::TransferFinished {
                            transfer_id: token.clone(),
                            error: Some("传输超时中断".into()),
                        });
                    }
                    break;
                }
            }
        });
    }
    for msg in echo {
        let _ = st.event_tx.try_send(CoreEvent::Message { msg });
    }
    let _ = st.event_tx.try_send(CoreEvent::TransferStarted {
        transfer_id: token.clone(),
        peer_id: peer_id.clone(),
        peer_name,
        outgoing: false,
        files: body.files,
        sources: Vec::new(),
        save_dir: Some(save_dir),
    });

    Json(PrepareOk { token }).into_response()
}

/// UI 回应接收请求
pub fn respond(st: &ServerState, req_id: &str, accept: bool, save_dir: Option<PathBuf>) {
    let pending = st.pending.lock().unwrap().remove(req_id);
    if let Some(p) = pending {
        let _ = p.responder.send(Decision { accept, save_dir });
    }
}

// ---------------------------------------------------------------------------
// PUT /api/transfer/upload/{token}/{file_id}
// ---------------------------------------------------------------------------

async fn upload(
    State(st): State<Arc<ServerState>>,
    Path((token, file_id)): Path<(String, String)>,
    req: Request,
) -> Response {
    // 取出会话快照（不长期持锁）
    let (meta, save_dir, cancel, msg_id, total_files, progress_bytes) = {
        let sessions = st.sessions.lock().unwrap();
        let Some(sess) = sessions.get(&token) else {
            return err(StatusCode::NOT_FOUND, "会话不存在或已结束");
        };
        if sess.cancel.is_cancelled() {
            return err(StatusCode::CONFLICT, "传输已取消");
        }
        let Some(meta) = sess.files.iter().find(|f| f.id == file_id) else {
            return err(StatusCode::NOT_FOUND, "文件 ID 无效");
        };
        (
            meta.clone(),
            sess.save_dir.clone(),
            sess.cancel.clone(),
            sess.msg_ids.get(&file_id).copied(),
            sess.files.len(),
            sess.progress_bytes.clone(),
        )
    };

    // 目标路径（含子目录）+ 同名避让
    let final_path = match target_path(&save_dir, &meta) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if let Some(parent) = final_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    }
    let tmp_path = final_path.with_extension("ltuploading");

    let mut file = match tokio::fs::File::create(&tmp_path).await {
        Ok(f) => f,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("创建文件失败: {e}")),
    };

    let mut stream = req.into_body().into_data_stream();
    let mut written: u64 = 0;
    let mut throttle = ProgressThrottle::new(150);
    let ev_progress = |transferred: u64, done: bool, path: Option<std::path::PathBuf>| {
        let _ = st.event_tx.try_send(CoreEvent::FileProgress {
            transfer_id: token.clone(),
            file_id: file_id.clone(),
            transferred,
            total: meta.size,
            done,
            path,
        });
    };

    use tokio::io::AsyncWriteExt;
    let mut io_err: Option<String> = None;
    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            io_err = Some("已取消".into());
            break;
        }
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                io_err = Some(format!("网络中断: {e}"));
                break;
            }
        };
        if written + chunk.len() as u64 > meta.size {
            io_err = Some("数据超出声明大小".into());
            break;
        }
        if let Err(e) = file.write_all(&chunk).await {
            io_err = Some(format!("写盘失败: {e}"));
            break;
        }
        written += chunk.len() as u64;
        progress_bytes.fetch_add(chunk.len() as u64, std::sync::atomic::Ordering::Relaxed);
        if throttle.allow() {
            ev_progress(written, false, None);
        }
    }
    if io_err.is_none() {
        if let Err(e) = file.flush().await {
            io_err = Some(format!("写盘失败: {e}"));
        }
    }
    drop(file);

    if let Some(reason) = io_err {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        // 发送端中断/取消：终结会话（若仍在），避免悬挂
        let cancelled_by_sender = reason == "网络中断" || reason == "已取消";
        if cancelled_by_sender {
            if let Some(sess) = st.sessions.lock().unwrap().remove(&token) {
                sess.cancel.cancel();
                let _ = st.event_tx.try_send(CoreEvent::TransferFinished {
                    transfer_id: token.clone(),
                    error: Some("传输中断".into()),
                });
            }
        }
        return err(StatusCode::CONFLICT, &reason);
    }

    if written != meta.size {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return err(
            StatusCode::BAD_REQUEST,
            &format!("大小不符：收到 {written} / 声明 {}", meta.size),
        );
    }
    if let Err(e) = tokio::fs::rename(&tmp_path, &final_path).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("落盘失败: {e}"));
    }

    ev_progress(written, true, Some(final_path.clone()));
    if let Some(id) = msg_id {
        if let Ok(store) = st.store.lock() {
            let _ = store.update_file_path(id, &final_path);
        }
    }

    // 收尾：全部文件完成 → 移除会话 + 终结事件
    let finished = {
        let mut sessions = st.sessions.lock().unwrap();
        if let Some(sess) = sessions.get_mut(&token) {
            sess.completed += 1;
            if sess.completed >= total_files {
                sessions.remove(&token);
                true
            } else {
                false
            }
        } else {
            false // 会话已被取消移除
        }
    };
    if finished {
        let _ = st.event_tx.try_send(CoreEvent::TransferFinished {
            transfer_id: token,
            error: None,
        });
    }
    StatusCode::OK.into_response()
}

/// 计算接收落盘路径：save_dir/rel_path，同名自动加 " (n)"
fn target_path(save_dir: &std::path::Path, meta: &FileMeta) -> anyhow::Result<PathBuf> {
    let p = save_dir.join(&meta.rel_path);
    if !p.starts_with(save_dir) {
        anyhow::bail!("路径逃逸: {}", meta.rel_path);
    }
    if !p.exists() {
        return Ok(p);
    }
    // 同名避让：name.ext → name (1).ext
    let mut n = 1;
    loop {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
        let ext = p.extension().and_then(|s| s.to_str());
        let candidate = match ext {
            Some(ext) => p.with_file_name(format!("{stem} ({n}).{ext}")),
            None => p.with_file_name(format!("{stem} ({n})")),
        };
        if !candidate.exists() {
            return Ok(candidate);
        }
        n += 1;
        if n > 9999 {
            anyhow::bail!("同名文件过多: {}", meta.rel_path);
        }
    }
}

// ---------------------------------------------------------------------------
// POST /api/transfer/cancel/{token}（发送端调用）
// ---------------------------------------------------------------------------

async fn cancel_transfer(State(st): State<Arc<ServerState>>, Path(token): Path<String>) -> Response {
    let sess = st.sessions.lock().unwrap().remove(&token);
    if let Some(sess) = sess {
        sess.cancel.cancel();
        let _ = st.event_tx.try_send(CoreEvent::TransferFinished {
            transfer_id: token,
            error: Some("对方取消了传输".into()),
        });
    }
    StatusCode::OK.into_response()
}

/// 本机 UI 取消接收（非 HTTP 入口）
pub fn cancel_local(st: &ServerState, token: &str) {
    let sess = st.sessions.lock().unwrap().remove(token);
    if let Some(sess) = sess {
        sess.cancel.cancel();
        let _ = st.event_tx.try_send(CoreEvent::TransferFinished {
            transfer_id: token.to_string(),
            error: Some("已取消".into()),
        });
    }
}

/// 关闭时取消所有接收会话
pub fn cancel_all(st: &ServerState) {
    let mut sessions = st.sessions.lock().unwrap();
    for (_, sess) in sessions.drain() {
        sess.cancel.cancel();
    }
}
