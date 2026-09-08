//! 网页客户端：浏览器打开 http://<本机IP>:<端口>/ 得到完整聊天界面，
//! 可收发文本、上传文件/文件夹（保存到 下载目录/网页上传/）、下载电脑
//! 端发布的文件。页面为内嵌 HTML + fetch 轮询，无外部依赖。

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
};
use futures::StreamExt;

use crate::proto::{ChatMessage, CoreEvent, FileMeta, MessageKind, WEB_PEER_ID};
use crate::server::ServerState;

/// 网页上传文件的保存子目录
pub const WEB_UPLOAD_DIR: &str = "网页上传";
/// 网页历史轮询返回的最大条数
const WEB_HISTORY_LIMIT: i64 = 200;
/// 单个网页上传的大小上限（10GB 兜底，防异常请求写满磁盘）
const WEB_UPLOAD_MAX: u64 = 10 * 1024 * 1024 * 1024;

/// 网页接收页的一个可下载文件（电脑端"发送到网页"）
pub struct WebOffer {
    pub id: String,
    pub name: String,
    /// 相对路径（含文件夹名前缀；浏览器端分组显示用）
    pub rel: String,
    pub size: u64,
    pub path: PathBuf,
    pub created_at: i64,
    /// 同一次发布共用的批次 id（= 消息 transfer_id；打包下载按它取整批）
    pub transfer_id: String,
}

/// 把文件发布到网页：注册下载项 + 写入网页会话消息（电脑端聊天可见）。
/// 同一次发布共用 batch id 作为 transfer_id → 桌面端合并为一张文件夹卡片
pub fn web_offer(st: &ServerState, files: &[(FileMeta, PathBuf)]) -> usize {
    let now = crate::proto::now_ms();
    let batch = uuid::Uuid::new_v4().to_string();
    let mut offers = st.web_offers.lock().unwrap();
    let mut msgs = Vec::new();
    for (meta, path) in files {
        let id = uuid::Uuid::new_v4().to_string();
        offers.push(WebOffer {
            id: id.clone(),
            name: meta.name.clone(),
            rel: meta.rel_path.clone(),
            size: meta.size,
            path: path.clone(),
            created_at: now,
            transfer_id: batch.clone(),
        });
        // 入库（outgoing，带源路径 → 电脑端"打开"可用；重启后仍在）
        let msg = (|| -> Option<ChatMessage> {
            let s = st.store.lock().ok()?;
            s.insert_file(
                WEB_PEER_ID,
                true,
                &meta.name,
                meta.size,
                Some(path.as_path()),
                Some(&batch),
                Some(&id),
                now,
            )
            .ok()
            .map(|row_id| ChatMessage {
                id: row_id,
                peer_id: WEB_PEER_ID.into(),
                outgoing: true,
                kind: MessageKind::File {
                    name: meta.name.clone(),
                    size: meta.size,
                    saved_path: Some(path.clone()),
                    transfer_id: Some(batch.clone()),
                    file_id: Some(id),
                },
                created_at: now,
            })
        })();
        if let Some(m) = msg {
            msgs.push(m);
        }
    }
    // 上限防无限堆积
    let len = offers.len();
    if len > 500 {
        offers.drain(0..len - 500);
    }
    for m in msgs {
        let _ = st.event_tx.try_send(CoreEvent::Message { msg: m });
    }
    files.len()
}

/// 电脑端发文本到网页会话（入库 + 事件）
pub fn web_text(st: &ServerState, text: &str) {
    let now = crate::proto::now_ms();
    let Ok(s) = st.store.lock() else { return };
    let Ok(id) = s.insert_text(WEB_PEER_ID, true, text, now) else {
        return;
    };
    drop(s);
    let _ = st.event_tx.try_send(CoreEvent::Message {
        msg: ChatMessage {
            id,
            peer_id: WEB_PEER_ID.into(),
            outgoing: true,
            kind: MessageKind::Text(text.to_string()),
            created_at: now,
        },
    });
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub async fn page(State(st): State<Arc<ServerState>>) -> Response {
    let name = st.me.lock().unwrap().name.clone();
    let html = WEB_PAGE.replace("__DEVICE_NAME__", &html_escape(&name));
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/web/messages —— 浏览器轮询聊天记录（最近 N 条，按 id 增量）
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct WebMessageDto {
    id: i64,
    /// 电脑端视角的 outgoing：true=电脑发出（浏览器左侧+可下载），false=浏览器发出
    outgoing: bool,
    kind: &'static str, // "text" | "file"
    text: String,
    name: String,
    size: u64,
    /// 同批文件共用的批次 id（浏览器分组为文件夹卡）
    tid: String,
    /// 相对路径（含文件夹前缀；offer 失效后仍可用于显示）
    rel: String,
    /// 文件消息：可下载 id（电脑端发布的 offer）；空 = 已失效
    offer: String,
    created_at: i64,
}

#[derive(serde::Deserialize, Default)]
pub struct SinceParams {
    since: Option<i64>,
}

pub async fn messages(
    State(st): State<Arc<ServerState>>,
    Query(p): Query<SinceParams>,
) -> Json<Vec<WebMessageDto>> {
    let since = p.since.unwrap_or(0);
    let history = st
        .store
        .lock()
        .ok()
        .and_then(|s| s.load_history(WEB_PEER_ID, WEB_HISTORY_LIMIT).ok())
        .unwrap_or_default();
    // 活跃 offer：id → rel（判断下载链接是否有效 + 显示相对路径）
    let live: HashMap<String, String> = st
        .web_offers
        .lock()
        .unwrap()
        .iter()
        .map(|o| (o.id.clone(), o.rel.clone()))
        .collect();
    let out = history
        .into_iter()
        .filter(|m| m.id > since)
        .map(|m| {
            let (kind, text, name, size, tid, rel, offer) = match &m.kind {
                MessageKind::Text(t) => (
                    "text",
                    t.clone(),
                    String::new(),
                    0u64,
                    String::new(),
                    String::new(),
                    String::new(),
                ),
                MessageKind::File {
                    name,
                    size,
                    saved_path,
                    transfer_id,
                    file_id,
                } => {
                    let fid = file_id.clone().unwrap_or_default();
                    match live.get(&fid) {
                        Some(o_rel) => (
                            "file",
                            String::new(),
                            name.clone(),
                            *size,
                            transfer_id.clone().unwrap_or_default(),
                            o_rel.clone(),
                            fid,
                        ),
                        // offer 已失效（重启/移除）或浏览器上传的消息（本就无 offer）：
                        // 从落盘路径反推 rel（下载目录/网页上传/<rel>），
                        // 浏览器端凭它显示文件夹名
                        None => {
                            let upload_root = st.default_download_dir.join(WEB_UPLOAD_DIR);
                            let rel = saved_path
                                .as_deref()
                                .and_then(|p| p.strip_prefix(&upload_root).ok())
                                .filter(|p| !p.as_os_str().is_empty())
                                .map(|p| p.to_string_lossy().replace('\\', "/"))
                                .unwrap_or_default();
                            (
                                "file",
                                String::new(),
                                name.clone(),
                                *size,
                                transfer_id.clone().unwrap_or_default(),
                                rel,
                                String::new(),
                            )
                        }
                    }
                }
            };
            WebMessageDto {
                id: m.id,
                outgoing: m.outgoing,
                kind,
                text,
                name,
                size,
                tid,
                rel,
                offer,
                created_at: m.created_at,
            }
        })
        .collect();
    Json(out)
}

// ---------------------------------------------------------------------------
// POST /api/web/message —— 浏览器发文本
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub struct WebTextBody {
    pub text: String,
}

pub async fn message(
    State(st): State<Arc<ServerState>>,
    axum::Json(body): axum::Json<WebTextBody>,
) -> Response {
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return crate::server::err_response(StatusCode::BAD_REQUEST, "空消息");
    }
    let now = crate::proto::now_ms();
    let id = {
        let Ok(s) = st.store.lock() else {
            return crate::server::err_response(StatusCode::INTERNAL_SERVER_ERROR, "存储不可用");
        };
        match s.insert_text(WEB_PEER_ID, false, &text, now) {
            Ok(id) => id,
            Err(e) => {
                return crate::server::err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            }
        }
    };
    let _ = st.event_tx.try_send(CoreEvent::Message {
        msg: ChatMessage {
            id,
            peer_id: WEB_PEER_ID.into(),
            outgoing: false,
            kind: MessageKind::Text(text),
            created_at: now,
        },
    });
    StatusCode::OK.into_response()
}

// ---------------------------------------------------------------------------
// POST /api/web/upload?name=<文件名>&rel=<相对路径> —— 浏览器上传（原始流）
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub struct UploadParams {
    pub name: Option<String>,
    pub rel: Option<String>,
    /// 上传批次 id（浏览器每次选择生成一个；同批文件在桌面端合并为一张文件夹卡片）
    pub batch: Option<String>,
}

pub async fn upload(
    State(st): State<Arc<ServerState>>,
    Query(p): Query<UploadParams>,
    req: axum::extract::Request,
) -> Response {
    let name = p.name.unwrap_or_else(|| "file".into());
    // rel 可带目录（webkitdirectory 上传文件夹时为 folder/sub/a.txt）
    let rel = p
        .rel
        .unwrap_or_else(|| name.clone())
        .replace('\\', "/");
    // 防路径逃逸：去掉 ..、绝对段、盘符
    let safe: Vec<&str> = rel
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != ".." && !s.contains(':'))
        .collect();
    if safe.is_empty() {
        return crate::server::err_response(StatusCode::BAD_REQUEST, "非法路径");
    }
    let base = st.default_download_dir.join(WEB_UPLOAD_DIR);
    let final_path = base.join(safe.join("/"));
    if let Some(parent) = final_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return crate::server::err_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("创建目录失败: {e}"));
        }
    }
    // 同名避让
    let mut target = final_path.clone();
    let mut n = 1;
    while tokio::fs::try_exists(&target).await.unwrap_or(false) {
        let stem = final_path.file_stem().and_then(|s| s.to_str()).unwrap_or("f");
        let ext = final_path.extension().and_then(|s| s.to_str());
        target = match ext {
            Some(e) => final_path.with_file_name(format!("{stem} ({n}).{e}")),
            None => final_path.with_file_name(format!("{stem} ({n})")),
        };
        n += 1;
        if n > 9999 {
            return crate::server::err_response(StatusCode::CONFLICT, "同名文件过多");
        }
    }

    // 流式落盘
    let tmp = target.with_extension("ltwebuploading");
    let mut file = match tokio::fs::File::create(&tmp).await {
        Ok(f) => f,
        Err(e) => {
            return crate::server::err_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("创建文件失败: {e}"))
        }
    };
    use tokio::io::AsyncWriteExt;
    let mut stream = req.into_body().into_data_stream();
    let mut written: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return crate::server::err_response(StatusCode::BAD_REQUEST, &format!("读取失败: {e}"));
            }
        };
        written += chunk.len() as u64;
        if written > WEB_UPLOAD_MAX {
            let _ = tokio::fs::remove_file(&tmp).await;
            return crate::server::err_response(StatusCode::PAYLOAD_TOO_LARGE, "文件过大");
        }
        if let Err(e) = file.write_all(&chunk).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return crate::server::err_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("写入失败: {e}"));
        }
    }
    if let Err(e) = file.flush().await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return crate::server::err_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("写入失败: {e}"));
    }
    drop(file);
    if let Err(e) = tokio::fs::rename(&tmp, &target).await {
        return crate::server::err_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("落盘失败: {e}"));
    }

    // 入库 + 事件（电脑端网页会话出现文件卡片）。
    // 同批上传共用 batch id 作为 transfer_id → 桌面端合并为一张卡片
    let display_name = target
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(name);
    let now = crate::proto::now_ms();
    let tid = p
        .batch
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let row = (|| -> Option<ChatMessage> {
        let s = st.store.lock().ok()?;
        s.insert_file(
            WEB_PEER_ID,
            false,
            &display_name,
            written,
            Some(target.as_path()),
            Some(&tid),
            None,
            now,
        )
        .ok()
        .map(|id| ChatMessage {
            id,
            peer_id: WEB_PEER_ID.into(),
            outgoing: false,
            kind: MessageKind::File {
                name: display_name,
                size: written,
                saved_path: Some(target.clone()),
                transfer_id: Some(tid),
                file_id: None,
            },
            created_at: now,
        })
    })();
    if let Some(msg) = row {
        let _ = st.event_tx.try_send(CoreEvent::Message { msg });
    }
    StatusCode::OK.into_response()
}

// ---------------------------------------------------------------------------
// GET /api/web/list / download / remove（旧列表接口保留兼容）
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct WebOfferDto {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub created_at: i64,
}

pub async fn list(State(st): State<Arc<ServerState>>) -> Json<Vec<WebOfferDto>> {
    let offers = st.web_offers.lock().unwrap();
    Json(
        offers
            .iter()
            .map(|o| WebOfferDto {
                id: o.id.clone(),
                name: o.name.clone(),
                size: o.size,
                created_at: o.created_at,
            })
            .collect(),
    )
}

/// Content-Disposition 的 UTF-8 文件名（percent-encode，中文文件名必需）
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub async fn download(State(st): State<Arc<ServerState>>, Path(id): Path<String>) -> Response {
    let offer = st
        .web_offers
        .lock()
        .unwrap()
        .iter()
        .find(|o| o.id == id)
        .map(|o| (o.name.clone(), o.path.clone()));
    let Some((name, path)) = offer else {
        return crate::server::err_response(StatusCode::NOT_FOUND, "文件不存在或已被移除");
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => {
            return crate::server::err_response(StatusCode::NOT_FOUND, &format!("打开文件失败: {e}"))
        }
    };
    let len = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
    let encoded = percent_encode(&name);
    let fallback = name.replace('"', "_").replace('\n', "_");
    let stream = tokio_util::io::ReaderStream::new(file);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"),
            ),
            (header::CONTENT_LENGTH, len.to_string()),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

pub async fn remove(State(st): State<Arc<ServerState>>, Path(id): Path<String>) -> Response {
    let mut offers = st.web_offers.lock().unwrap();
    let before = offers.len();
    offers.retain(|o| o.id != id);
    if offers.len() == before {
        return crate::server::err_response(StatusCode::NOT_FOUND, "不存在");
    }
    StatusCode::OK.into_response()
}

// ---------------------------------------------------------------------------
// GET /api/web/download-zip/{tid} —— 整批打包下载（Stored 不压缩，局域网重速度）
// ---------------------------------------------------------------------------

/// 流结束后（或客户端中断、body 被 drop）删掉临时 zip
struct TempFileGuard(PathBuf);
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 给 ReaderStream 套一层持有 guard 的包装流（Unpin 即可，不引 pin-project）
struct CleanupStream<S> {
    inner: S,
    _guard: TempFileGuard,
}
impl<S> futures::Stream for CleanupStream<S>
where
    S: futures::Stream + Unpin,
{
    type Item = S::Item;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        futures::Stream::poll_next(std::pin::Pin::new(&mut self.inner), cx)
    }
}

fn write_zip(out: &std::path::Path, files: &[(String, PathBuf)]) -> anyhow::Result<()> {
    let f = std::fs::File::create(out)?;
    let mut w = std::io::BufWriter::new(f);
    let mut zip = zip::ZipWriter::new(&mut w);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    for (rel, path) in files {
        zip.start_file(rel.as_str(), opts)?;
        let mut src = std::fs::File::open(path)?;
        std::io::copy(&mut src, &mut zip)?;
    }
    zip.finish()?;
    Ok(())
}

pub async fn download_zip(State(st): State<Arc<ServerState>>, Path(tid): Path<String>) -> Response {
    let files: Vec<(String, PathBuf)> = st
        .web_offers
        .lock()
        .unwrap()
        .iter()
        .filter(|o| o.transfer_id == tid)
        .map(|o| (o.rel.clone(), o.path.clone()))
        .collect();
    if files.is_empty() {
        return crate::server::err_response(StatusCode::NOT_FOUND, "没有可下载的文件（可能已失效）");
    }
    let folder = files[0]
        .0
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("LocalTransfer")
        .to_string();
    let zip_path = std::env::temp_dir().join(format!("ltweb-{}.zip", uuid::Uuid::new_v4()));
    let zp = zip_path.clone();
    let packed = tokio::task::spawn_blocking(move || write_zip(&zp, &files)).await;
    match packed {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = std::fs::remove_file(&zip_path);
            return crate::server::err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("打包失败: {e:#}"),
            );
        }
        Err(e) => {
            let _ = std::fs::remove_file(&zip_path);
            return crate::server::err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("打包任务失败: {e}"),
            );
        }
    }
    let file = match tokio::fs::File::open(&zip_path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_file(&zip_path);
            return crate::server::err_response(StatusCode::NOT_FOUND, &format!("打开打包失败: {e}"));
        }
    };
    let len = tokio::fs::metadata(&zip_path).await.map(|m| m.len()).unwrap_or(0);
    let encoded = percent_encode(&format!("{folder}.zip"));
    let fallback = folder.replace('"', "_").replace('\n', "_");
    let stream = CleanupStream {
        inner: tokio_util::io::ReaderStream::new(file),
        _guard: TempFileGuard(zip_path),
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{fallback}.zip\"; filename*=UTF-8''{encoded}"),
            ),
            (header::CONTENT_LENGTH, len.to_string()),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /api/web/messages/delete —— 删除消息（body: {"ids":[...]}）
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub struct DeleteMessagesBody {
    pub ids: Vec<i64>,
}

pub async fn delete_messages(
    State(st): State<Arc<ServerState>>,
    axum::Json(body): axum::Json<DeleteMessagesBody>,
) -> Response {
    if body.ids.is_empty() {
        return (StatusCode::OK, Json(serde_json::json!({"deleted": 0}))).into_response();
    }
    let n = st
        .store
        .lock()
        .ok()
        .and_then(|s| s.delete_messages(&body.ids).ok())
        .unwrap_or(0);
    (StatusCode::OK, Json(serde_json::json!({"deleted": n}))).into_response()
}

/// 接收页 HTML（__DEVICE_NAME__ 会被替换为设备名）。
/// 结构：消息流（文本气泡 + 文件卡片）+ 输入行（文本/文件/文件夹）。
const WEB_PAGE: &str = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1">
<title>LocalTransfer</title>
<style>
  :root { color-scheme: light dark; }
  * { box-sizing: border-box; margin: 0; }
  html, body { height: 100%; }
  body { font-family: system-ui, "Microsoft YaHei", sans-serif; background: #f4f5f7; color: #1f2328;
         display: flex; flex-direction: column; }
  @media (prefers-color-scheme: dark) {
    body { background: #17181c; color: #e6e8eb; }
    .fcard { background: #26282e !important; }
    .bub-in { background: #2a2c33 !important; }
  }
  header { padding: 14px 16px 10px; border-bottom: 1px solid rgba(127,127,127,.18); }
  header h1 { font-size: 16px; display: flex; align-items: center; gap: 8px; }
  header .sub { font-size: 12px; opacity: .55; margin-top: 3px; }
  #msgs { flex: 1; overflow-y: auto; padding: 16px; display: flex; flex-direction: column; gap: 10px; }
  .row { display: flex; }
  .row.out { justify-content: flex-end; }
  .bub { max-width: 78%; padding: 8px 12px; border-radius: 14px; font-size: 14px;
         white-space: pre-wrap; word-break: break-word; }
  .bub-in { background: #e7e9ee; border-bottom-left-radius: 4px; }
  .row.out .bub { background: #6366f1; color: #fff; border-bottom-right-radius: 4px; }
  .fcard { max-width: 78%; background: #fff; border: 1px solid rgba(127,127,127,.2);
           border-radius: 12px; padding: 10px; display: flex; gap: 10px; align-items: center; }
  .ffolder { flex-direction: column; align-items: stretch; }
  .ffolder > div { display: flex; gap: 10px; align-items: center; }
  .flist { margin-top: 8px; border-top: 1px solid rgba(127,127,127,.15); padding-top: 4px; }
  .fitem2 { display: flex; align-items: center; gap: 8px; padding: 5px 2px; font-size: 12px; }
  .finame { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .fidl a { color: #6366f1; text-decoration: none; }
  .fidl a:hover { text-decoration: underline; }
  .ficon { width: 36px; height: 36px; border-radius: 8px; background: rgba(99,102,241,.15);
           color: #6366f1; display: flex; align-items: center; justify-content: center; flex: none; }
  .fmeta { flex: 1; min-width: 0; }
  .fname { font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .fsize { font-size: 12px; opacity: .55; margin-top: 2px; }
  .time { font-size: 10px; opacity: .4; margin-top: 2px; }
  a.dlbtn, .upbtn { border: 0; border-radius: 8px; padding: 7px 12px; font-size: 13px; cursor: pointer;
        background: #6366f1; color: #fff; text-decoration: none; display: inline-block; flex: none; }
  .exp { font-size: 11px; opacity: .5; }
  .prog { height: 3px; background: rgba(127,127,127,.25); border-radius: 2px; overflow: hidden; margin-top: 6px; }
  .prog > div { height: 100%; background: #6366f1; width: 0; transition: width .15s; }
  #inputbar { display: flex; gap: 8px; padding: 10px 12px; border-top: 1px solid rgba(127,127,127,.18);
              background: inherit; align-items: flex-end; }
  #txt { flex: 1; border: 1px solid rgba(127,127,127,.35); border-radius: 10px; padding: 9px 12px;
         font-size: 14px; background: transparent; color: inherit; resize: none; min-height: 40px; max-height: 110px; }
  .ibtn { border: 0; background: transparent; color: inherit; font-size: 19px; cursor: pointer;
          padding: 8px; border-radius: 8px; flex: none; }
  .ibtn:hover { background: rgba(127,127,127,.12); }
  #send { background: #6366f1; color: #fff; border-radius: 10px; padding: 9px 16px; border: 0;
          font-size: 14px; cursor: pointer; flex: none; }
  #empty { text-align: center; opacity: .45; font-size: 13px; margin: auto; padding: 0 24px; line-height: 1.8; }
  .fclick { cursor: pointer; }
  .fclick:hover { border-color: rgba(99,102,241,.55); }
  #modal { position: fixed; inset: 0; background: rgba(0,0,0,.45); display: none;
           align-items: center; justify-content: center; z-index: 10; padding: 20px; }
  #modal.show { display: flex; }
  .sheet { background: #fff; color: #1f2328; border-radius: 14px; max-width: 420px; width: 100%;
           max-height: 70vh; display: flex; flex-direction: column; overflow: hidden;
           box-shadow: 0 12px 40px rgba(0,0,0,.25); }
  @media (prefers-color-scheme: dark) {
    .sheet { background: #26282e; color: #e6e8eb; }
  }
  .sheet h3 { font-size: 14px; padding: 14px 16px 10px; display: flex; align-items: center; gap: 8px; }
  .sheet h3 .cnt { font-size: 11px; opacity: .5; font-weight: normal; }
  .sheet .body { overflow-y: auto; padding: 0 12px 12px; }
  .sheet .fitem2 { padding: 8px 4px; border-top: 1px solid rgba(127,127,127,.12); }
  .sheet .fitem2:first-child { border-top: 0; }
  .fbtn { border: 0; border-radius: 8px; padding: 6px 10px;
          font-size: 12px; cursor: pointer; color: #6366f1; text-decoration: none;
          display: inline-block; flex: none; white-space: nowrap; }
  a.fbtn:hover, div.fbtn:hover { background: rgba(99,102,241,.12); }
  .spd { min-height: 15px; }
  .cmenu { position: fixed; z-index: 30; background: #fff; color: #1f2328; border-radius: 10px;
           box-shadow: 0 6px 24px rgba(0,0,0,.22); padding: 4px; min-width: 120px; }
  @media (prefers-color-scheme: dark) { .cmenu { background: #2a2c33; color: #e6e8eb; } }
  .citem { padding: 8px 14px; font-size: 13px; border-radius: 7px; cursor: pointer; }
  .citem:hover { background: rgba(127,127,127,.14); }
  .citem.danger { color: #dc2626; }
  .citem.disabled { opacity: .4; cursor: default; }
  .citem.disabled:hover { background: transparent; }
  #toast { position: fixed; left: 50%; bottom: 76px; transform: translateX(-50%) translateY(8px);
           background: rgba(30,32,38,.92); color: #fff; padding: 8px 16px; border-radius: 9px;
           font-size: 13px; opacity: 0; pointer-events: none; transition: all .2s; z-index: 40; }
  #toast.show { opacity: 1; transform: translateX(-50%) translateY(0); }
</style>
</head>
<body>
<header>
  <h1>&#128172; <span id="dev"></span></h1>
  <div class="sub">LocalTransfer 网页客户端 · 与电脑互发文字和文件</div>
</header>
<div id="msgs"><div id="empty">和电脑端互发消息、文件、文件夹<br>电脑端「发送到网页」的文件会出现在这里</div></div>
<div id="modal" onclick="if(event.target===this)this.classList.remove('show')">
  <div class="sheet">
    <h3 id="mtitle"></h3>
    <div class="body" id="mlist"></div>
  </div>
</div>
<div id="toast"></div>
<div id="inputbar">
  <button class="ibtn" title="发送文件" onclick="document.getElementById('fpick').click()">&#128196;</button>
  <button class="ibtn" title="发送文件夹" onclick="document.getElementById('dpick').click()">&#128193;</button>
  <textarea id="txt" rows="1" placeholder="输入消息，Enter 发送"></textarea>
  <button id="send" onclick="sendText()">发送</button>
</div>
<input type="file" id="fpick" multiple hidden>
<input type="file" id="dpick" webkitdirectory hidden>
<script>
var DEV = "__DEVICE_NAME__";
var lastId = 0, known = {};
document.getElementById("dev").textContent = DEV;
document.title = DEV + " · 网页客户端";
var msgsEl = document.getElementById("msgs");

function fmt(b) { var u = ["B","KB","MB","GB"]; var i = 0; while (b >= 1024 && i < 4) { b /= 1024; i++; } return i ? b.toFixed(1) + " " + u[i] : b + " B"; }
function fmtTime(t) { var d = new Date(t); return d.toTimeString().slice(0,5); }
function esc(s) { var d = document.createElement("div"); d.textContent = s; return d.innerHTML; }

function refresh() {
  return fetch("/api/web/messages?since=" + lastId).then(function(r){ return r.json(); }).then(function(list) {
    if (!list.length) return;
    var e = document.getElementById("empty"); if (e) e.remove();
    for (var i = 0; i < list.length; i++) {
      var m = list[i];
      if (m.id > lastId) lastId = m.id;
      if (known[m.id]) continue;
      known[m.id] = true;
      addMessage(m);
    }
    msgsEl.scrollTop = msgsEl.scrollHeight;
  }).catch(function(){});
}

// 批次归并：同 tid 的文件消息共用一张卡。必须跨轮询稳定——
// 上传逐文件完成、轮询可能分次拿到（旧版只合并同一次响应里的相邻消息，
// 批次被轮询撕碎后每个文件一行）
var batches = {};   // tid -> { el, files, outgoing }

function addMessage(m) {
  if (m.kind === "text") { msgsEl.appendChild(renderText(m)); return; }
  if (m.tid) {
    var b = batches[m.tid];
    if (!b) {
      b = batches[m.tid] = { files: [], outgoing: m.outgoing,
                             el: document.createElement("div") };
      b.el.className = "row " + (m.outgoing ? "" : "out");
    }
    b.files.push(m);
    // 上传进行中的批（pending）不画卡——完成时一次性显示，
    // 避免"进度卡 + 半成品批卡"两行并存
    if (!b.pending) drawBatch(b);
  } else {
    msgsEl.appendChild(renderFileCard(m));
  }
}

/// 批卡重绘：1 个无目录文件=普通单文件卡；≥2 个或带目录=文件夹卡（点击弹窗看清单）
function drawBatch(b) {
  var files = b.files;
  var withDir = false, total = 0;
  for (var k = 0; k < files.length; k++) {
    if (files[k].rel && files[k].rel.indexOf("/") >= 0) withDir = true;
    total += files[k].size;
  }
  var c;
  if (files.length > 1 || withDir) {
    var name = withDir ? files[0].rel.split("/")[0] : (files.length + " 个文件");
    c = document.createElement("div");
    c.className = "fcard fclick";
    var icon = document.createElement("div"); icon.className = "ficon"; icon.innerHTML = "&#128193;";
    var meta = document.createElement("div"); meta.className = "fmeta";
    var nm = document.createElement("div"); nm.className = "fname"; nm.textContent = name;
    var sz = document.createElement("div"); sz.className = "fsize";
    sz.textContent = files.length + " 个文件 · " + fmt(total) + " · " +
                     fmtTime(files[0].created_at);
    meta.appendChild(nm); meta.appendChild(sz);
    c.appendChild(icon); c.appendChild(meta);
    // 查看按钮（整卡点击也开弹窗）
    var vb = document.createElement("div");
    vb.className = "fbtn"; vb.textContent = "🔍 查看";
    vb.onclick = function(e) { e.stopPropagation(); openFolder(name, files); };
    c.appendChild(vb);
    // 有可下载文件 → 打包下载整批
    if (files.some(function(f){ return f.offer; })) {
      var z = document.createElement("a");
      z.className = "fbtn"; z.textContent = "📦 打包下载";
      z.setAttribute("download", "");
      z.href = "/api/web/download-zip/" + encodeURIComponent(files[0].tid);
      z.onclick = function(e) { e.stopPropagation(); };
      c.appendChild(z);
    }
    c.onclick = function() { openFolder(name, files); };
  } else {
    c = fileCardEl(files[0]);
  }
  // 右键：删除整批
  var ids = files.map(function(f){ return f.id; });
  attachMenu(c, [menuDel(function(){ deleteMsg(ids, b.el, b); })]);
  if (!b.el.parentNode) msgsEl.appendChild(b.el);
  b.el.replaceChildren(c);
}

/// 文件夹弹窗：标题 + 逐文件（下载 / 已失效 / 大小）
function openFolder(name, files) {
  var total = 0; for (var k = 0; k < files.length; k++) total += files[k].size;
  var mt = document.getElementById("mtitle");
  mt.innerHTML = "&#128193; " + esc(name) +
    ' <span class="cnt">' + files.length + " 个文件 · " + fmt(total) + "</span>";
  var ml = document.getElementById("mlist");
  ml.textContent = "";
  for (var k = 0; k < files.length; k++) {
    var f = files[k];
    var it = document.createElement("div"); it.className = "fitem2";
    var d = document.createElement("div"); d.className = "finame";
    d.textContent = f.rel && f.rel.indexOf("/") >= 0
      ? f.rel.slice(f.rel.indexOf("/") + 1) : f.name;
    d.title = d.textContent;
    var dl = document.createElement("div"); dl.className = "fidl";
    if (f.offer) {
      var a = document.createElement("a");
      a.textContent = "下载"; a.setAttribute("download", "");
      a.href = "/api/web/download/" + f.offer;
      dl.appendChild(a);
    } else if (f.outgoing) {
      dl.textContent = "已失效"; dl.className = "exp";
    } else {
      dl.textContent = fmt(f.size);
    }
    it.appendChild(d); it.appendChild(dl);
    ml.appendChild(it);
  }
  document.getElementById("modal").classList.add("show");
}
document.addEventListener("keydown", function(e) {
  if (e.key === "Escape") document.getElementById("modal").classList.remove("show");
});

function renderText(m) {
  var row = document.createElement("div");
  row.className = "row " + (m.outgoing ? "" : "out");
  var b = document.createElement("div");
  b.className = "bub bub-in";
  b.textContent = m.text;
  row.appendChild(b);
  // 右键：复制 / 删除
  attachMenu(b, [
    { label: "复制", fn: function() { copyText(m.text); } },
    menuDel(function() { deleteMsg([m.id], row, null); }),
  ]);
  return row;
}

/// 单个无批次文件的消息行
function renderFileCard(m) {
  var row = document.createElement("div");
  row.className = "row " + (m.outgoing ? "" : "out");
  row.appendChild(fileCardEl(m));
  // 右键：删除
  attachMenu(row, [menuDel(function(){ deleteMsg([m.id], row, null); })]);
  return row;
}

// ---------------------------------------------------------------- 右键菜单
var menuEl = null;
function closeMenu() {
  if (menuEl) { menuEl.remove(); menuEl = null; }
}
document.addEventListener("click", closeMenu);
window.addEventListener("blur", closeMenu);
/// item：{label, fn}
function attachMenu(el, items) {
  el.addEventListener("contextmenu", function(e) {
    e.preventDefault(); e.stopPropagation();
    closeMenu();
    menuEl = document.createElement("div");
    menuEl.className = "cmenu";
    for (var i = 0; i < items.length; i++) {
      var it = document.createElement("div");
      it.className = "citem" + (items[i].danger ? " danger" : "");
      it.textContent = items[i].label;
      it.onclick = items[i].fn;
      menuEl.appendChild(it);
    }
    document.body.appendChild(menuEl);
    var x = Math.min(e.clientX, window.innerWidth - menuEl.offsetWidth - 8);
    var y = Math.min(e.clientY, window.innerHeight - menuEl.offsetHeight - 8);
    menuEl.style.left = Math.max(8, x) + "px";
    menuEl.style.top = Math.max(8, y) + "px";
  });
}
function menuDel(fn) { return { label: "删除", danger: true, fn: fn }; }

/// 删除消息（服务器 + DOM；批卡顺带清 batches 登记）
function deleteMsg(ids, el, b) {
  fetch("/api/web/messages/delete", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ ids: ids })
  }).then(function() { el.remove(); }).catch(function(){});
  if (b) {
    for (var k in batches) {
      if (batches[k] === b) { delete batches[k]; break; }
    }
  }
}

/// 复制文本（http 非 secure context 没有 navigator.clipboard，用 execCommand）
function copyText(t) {
  var ta = document.createElement("textarea");
  ta.value = t;
  ta.style.position = "fixed"; ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  try { document.execCommand("copy"); } catch (e) {}
  ta.remove();
}

/// 文件卡元素（批卡的单文件形态也用它）
function fileCardEl(m) {
  var c = document.createElement("div");
  c.className = "fcard";
  var icon = document.createElement("div"); icon.className = "ficon"; icon.innerHTML = "&#128196;";
  var meta = document.createElement("div"); meta.className = "fmeta";
  var nm = document.createElement("div"); nm.className = "fname"; nm.textContent = m.name;
  var sz = document.createElement("div"); sz.className = "fsize";
  sz.textContent = fmt(m.size) + " · " + fmtTime(m.created_at);
  meta.appendChild(nm); meta.appendChild(sz);
  c.appendChild(icon); c.appendChild(meta);
  if (m.outgoing && m.offer) {
    var a = document.createElement("a");
    a.className = "dlbtn"; a.textContent = "下载"; a.setAttribute("download", "");
    a.href = "/api/web/download/" + m.offer;
    c.appendChild(a);
  } else if (m.outgoing) {
    var x = document.createElement("div"); x.className = "exp"; x.textContent = "已失效";
    c.appendChild(x);
  }
  return c;
}

function sendText() {
  var el = document.getElementById("txt");
  var t = el.value.trim();
  if (!t) return;
  el.value = ""; fitTxt();
  fetch("/api/web/message", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ text: t })
  }).then(refresh);
}
document.getElementById("txt").addEventListener("keydown", function(e) {
  if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); sendText(); }
  // Shift+Enter = 换行（textarea 默认行为，配 auto-grow 才看得见）
});

// 输入框 auto-grow：单行起步，随内容长高（上限后内滚）
var txtEl = document.getElementById("txt");
function fitTxt() {
  txtEl.style.height = "auto";
  txtEl.style.height = Math.min(txtEl.scrollHeight, 110) + "px";
}
txtEl.addEventListener("input", fitTxt);

// toast 提示
var toastTimer = null;
function toast(s) {
  var t = document.getElementById("toast");
  t.textContent = s; t.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(function() { t.classList.remove("show"); }, 2200);
}

// 输入框右键：剪切/复制/粘贴/全选（与电脑端一致）
// 粘贴受浏览器限制：http 非 secure context 读不了剪贴板（localhost 可以），
// 失败时提示改用 Ctrl+V
txtEl.addEventListener("contextmenu", function(e) {
  e.preventDefault(); e.stopPropagation();
  closeMenu();
  var hasSel = txtEl.selectionStart !== txtEl.selectionEnd;
  menuEl = document.createElement("div");
  menuEl.className = "cmenu";
  function add(label, enabled, fn) {
    var it = document.createElement("div");
    it.className = "citem" + (enabled ? "" : " disabled");
    it.textContent = label;
    if (enabled) it.onclick = fn;
    menuEl.appendChild(it);
  }
  add("剪切", hasSel, function() {
    txtEl.focus(); document.execCommand("cut"); fitTxt();
  });
  add("复制", hasSel, function() {
    txtEl.focus(); document.execCommand("copy");
  });
  add("粘贴", true, function() { pasteInto(); });
  add("全选", txtEl.value.length > 0, function() {
    txtEl.focus(); txtEl.select();
  });
  document.body.appendChild(menuEl);
  var x = Math.min(e.clientX, window.innerWidth - menuEl.offsetWidth - 8);
  var y = Math.min(e.clientY, window.innerHeight - menuEl.offsetHeight - 8);
  menuEl.style.left = Math.max(8, x) + "px";
  menuEl.style.top = Math.max(8, y) + "px";
});

/// 在光标处插入文本（粘贴用）
function insertAtCursor(el, text) {
  var s = el.selectionStart, e2 = el.selectionEnd;
  el.value = el.value.slice(0, s) + text + el.value.slice(e2);
  var pos = s + text.length;
  el.setSelectionRange(pos, pos);
  el.focus(); fitTxt();
}
function pasteInto() {
  if (navigator.clipboard && navigator.clipboard.readText) {
    navigator.clipboard.readText().then(function(t) {
      insertAtCursor(txtEl, t);
    }).catch(function() {
      txtEl.focus(); toast("此环境无法读取剪贴板，请按 Ctrl+V 粘贴");
    });
  } else {
    txtEl.focus(); toast("此环境无法读取剪贴板，请按 Ctrl+V 粘贴");
  }
}

// 上传：逐个文件 XHR（有进度），rel 用 webkitRelativePath 保留目录结构；
// 每次选择生成一个 batch id，同批文件在电脑端合并为一张文件夹卡片
function uploadFiles(files) {
  var arr = Array.prototype.slice.call(files);
  if (!arr.length) return;
  var batch = Date.now().toString(36) + Math.random().toString(36).slice(2, 10);
  // 批登记为 pending：上传中不渲染批卡（避免与进度卡两行并存），
  // 完成后一次性显示以文件夹名命名的卡片
  var pb = batches[batch] = { files: [], outgoing: false,
                              el: document.createElement("div"), pending: true };
  pb.el.className = "row out";
  var cur = document.createElement("div");
  cur.className = "row out";
  cur.innerHTML = '<div class="fcard"><div class="ficon">&#9207;</div><div class="fmeta">' +
    '<div class="fname">正在发送 ' + arr.length + ' 个文件…</div>' +
    '<div class="fsize spd"></div>' +
    '<div class="prog"><div></div></div></div></div>';
  msgsEl.appendChild(cur);
  msgsEl.scrollTop = msgsEl.scrollHeight;
  var bar = cur.querySelector(".prog > div");
  var nameEl = cur.querySelector(".fname");
  var speedEl = cur.querySelector(".spd");
  var total = arr.reduce(function(s, f){ return s + f.size; }, 0), done = 0;
  var failed = 0;
  var lastT = 0, lastB = 0;   // 速度采样（400ms 窗口）
  function next(i) {
    if (i >= arr.length) {
      // 全部完成：拉回服务器消息，一次性显示批卡；成功则移除进度卡
      refresh().then(function() {
        pb.pending = false;
        if (pb.files.length) drawBatch(pb); else delete batches[batch];
        if (failed) nameEl.textContent = "发送完成（" + failed + " 个失败）";
        else if (cur.parentNode) cur.remove();
        msgsEl.scrollTop = msgsEl.scrollHeight;
      });
      return;
    }
    var f = arr[i];
    nameEl.textContent = "正在发送 (" + (i+1) + "/" + arr.length + ") " + f.name;
    var rel = f.webkitRelativePath || f.name;
    var xhr = new XMLHttpRequest();
    xhr.open("POST", "/api/web/upload?name=" + encodeURIComponent(f.name) +
             "&rel=" + encodeURIComponent(rel) + "&batch=" + encodeURIComponent(batch));
    xhr.upload.onprogress = function(ev) {
      if (ev.lengthComputable) {
        bar.style.width = Math.round(((done + ev.loaded) / total) * 100) + "%";
        var now = Date.now(), curB = done + ev.loaded;
        if (now - lastT >= 400) {
          if (lastT > 0 && curB > lastB)
            speedEl.textContent = fmt((curB - lastB) / ((now - lastT) / 1000)) + "/s";
          lastT = now; lastB = curB;
        }
      }
    };
    xhr.onloadend = function() {
      done += f.size;
      if (xhr.status !== 200) {
        failed++;
        nameEl.textContent = "发送失败: " + f.name + " (" + xhr.status + ")";
      }
      next(i + 1);
    };
    xhr.send(f);
  }
  next(0);
}
document.getElementById("fpick").addEventListener("change", function() {
  uploadFiles(this.files); this.value = "";
});
document.getElementById("dpick").addEventListener("change", function() {
  uploadFiles(this.files); this.value = "";
});

refresh();
setInterval(refresh, 1500);
</script>
</body>
</html>
"#;
