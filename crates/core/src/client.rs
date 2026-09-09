//! HTTP 客户端（reqwest）：发送文本、发起传输并流式上传（带进度/取消）

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail};
use smol::channel::Sender;
use tokio_util::{io::ReaderStream, sync::CancellationToken};

use crate::proto::{CoreEvent, DeviceInfo, FileMeta, MessageBody, PrepareBody, PrepareOk, ProgressThrottle};

/// 发送中传输的登记项（lib.rs 的命令循环用来处理取消）
pub struct SendReg {
    pub cancel: CancellationToken,
    pub base: String,
    /// prepare 成功后回填（接收方的 token，取消时通知对方用）
    pub token: Option<String>,
}

pub type Sendings = Arc<Mutex<HashMap<String, SendReg>>>;

/// 发送一条文本消息（不含入库，入库由调用方在发送前后自行决定）
pub async fn send_text(base: &str, me: &DeviceInfo, text: &str, sent_at: i64) -> Result<()> {
    // 局域网直连：绝不吃系统代理（用户开 Clash 等会把 LAN 请求劫持）
    let client = reqwest::Client::builder()
        // 局域网直连：绝不吃系统代理
        .no_proxy().build().unwrap();
    let resp = client
        .post(format!("{base}/api/message"))
        .json(&MessageBody {
            sender: me.clone(),
            text: text.to_string(),
            sent_at,
        })
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .context("连接对方失败")?;
    if !resp.status().is_success() {
        bail!("对方返回 {}", resp.status());
    }
    Ok(())
}

/// 发送一组文件（含 prepare 与逐文件上传）。transfer_id 由调用方生成并已登记进 sendings。
pub async fn send_files(
    base: String,
    me: DeviceInfo,
    files: Vec<(FileMeta, PathBuf)>,
    transfer_id: String,
    peer_id: String,
    peer_name: String,
    event_tx: Sender<CoreEvent>,
    cancel: CancellationToken,
    sendings: Sendings,
) {
    match send_files_inner(
        &base,
        &me,
        &files,
        &transfer_id,
        &peer_id,
        &peer_name,
        &event_tx,
        &cancel,
        &sendings,
    )
    .await
    {
        Ok(()) => {
            let _ = event_tx.try_send(CoreEvent::TransferFinished {
                transfer_id,
                error: None,
            });
        }
        Err(e) => {
            // 区分用户主动取消与真实错误
            let reason = if cancel.is_cancelled() {
                "已取消".to_string()
            } else {
                e.to_string()
            };
            let _ = event_tx.try_send(CoreEvent::TransferFinished {
                transfer_id,
                error: Some(reason),
            });
        }
    }
}

async fn send_files_inner(
    base: &str,
    me: &DeviceInfo,
    files: &[(FileMeta, PathBuf)],
    transfer_id: &str,
    peer_id: &str,
    peer_name: &str,
    event_tx: &Sender<CoreEvent>,
    cancel: &CancellationToken,
    sendings: &Sendings,
) -> Result<()> {
    let client = reqwest::Client::builder()
        // 局域网直连：绝不吃系统代理
        .no_proxy()
        .timeout(std::time::Duration::from_secs(600)) // 单文件上传上限；空闲时由 body 驱动
        .build()?;

    // 1) prepare
    let metas: Vec<FileMeta> = files.iter().map(|(m, _)| m.clone()).collect();

    // 先上报"传输已开始"：prepare 会阻塞到对方点确认，若等它返回才上报，
    // 发送方在整个等待期间看不到任何卡片（用户报过的问题）。
    // 卡片此时显示"等待对方接收"，upload 一开始就有进度。
    let _ = event_tx.try_send(CoreEvent::TransferStarted {
        transfer_id: transfer_id.to_string(),
        peer_id: peer_id.to_string(),
        peer_name: peer_name.to_string(),
        outgoing: true,
        files: metas.clone(),
        // 发送方的本地源路径：聊天卡片"打开/定位"用（与 files 一一对应）
        sources: files.iter().map(|(_, p)| p.clone()).collect(),
        save_dir: None,
    });

    let resp = client
        .post(format!("{base}/api/transfer/prepare"))
        .json(&PrepareBody {
            sender: me.clone(),
            files: metas.clone(),
        })
        .send()
        .await
        .context("连接对方失败")?;
    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        bail!("对方拒绝了传输");
    }
    if !resp.status().is_success() {
        bail!("对方返回 {}", resp.status());
    }
    let token = resp
        .json::<PrepareOk>()
        .await
        .context("解析对方响应失败")?
        .token;

    // 回填对方 token，供取消时通知
    if let Some(reg) = sendings.lock().unwrap().get_mut(transfer_id) {
        reg.token = Some(token.clone());
    }

    // 2) 逐文件上传（LAN 顺序传，避免互相抢带宽）
    for (meta, path) in files {
        if cancel.is_cancelled() {
            notify_cancel(&client, base, &token).await;
            bail!("已取消");
        }
        let file = tokio::fs::File::open(path)
            .await
            .with_context(|| format!("打开 {} 失败", path.display()))?;

        let counting = CountingStream {
            inner: ReaderStream::with_capacity(file, 256 * 1024),
            transferred: 0,
            total: meta.size,
            transfer_id: transfer_id.to_string(),
            file_id: meta.id.clone(),
            event_tx: event_tx.clone(),
            throttle: ProgressThrottle::new(150),
            cancel: cancel.clone(),
        };
        // reqwest 0.13 的 Body::wrap 接收 HttpBody；用 StreamBody 把计数流转成 HttpBody
        let body = reqwest::Body::wrap(http_body_util::StreamBody::new(counting));

        let url = format!("{base}/api/transfer/upload/{token}/{}", meta.id);
        let resp = client.put(&url).body(body).send().await;
        match resp {
            Ok(r) if r.status().is_success() => {}
            Ok(r) if r.status() == reqwest::StatusCode::CONFLICT => {
                notify_cancel(&client, base, &token).await;
                bail!("传输被中断");
            }
            Ok(r) => {
                notify_cancel(&client, base, &token).await;
                bail!("上传失败：{}", r.status());
            }
            Err(e) => {
                if !cancel.is_cancelled() {
                    notify_cancel(&client, base, &token).await;
                }
                bail!("网络错误：{e}");
            }
        }
    }
    Ok(())
}

/// 通知接收方取消（对方会清理会话与半成品文件）
pub async fn notify_cancel(client: &reqwest::Client, base: &str, token: &str) {
    let _ = client
        .post(format!("{base}/api/transfer/cancel/{token}"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await;
}

/// 探测远端设备信息（手动添加设备用）
pub async fn probe_info(base: &str) -> Result<DeviceInfo> {
    // 局域网直连：绝不吃系统代理（用户开 Clash 等会把 LAN 请求劫持）
    let client = reqwest::Client::builder()
        // 局域网直连：绝不吃系统代理
        .no_proxy().build().unwrap();
    let resp = client
        .get(format!("{base}/api/info"))
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .context("连接失败，请检查 IP 与端口")?;
    if !resp.status().is_success() {
        bail!("对方返回 {}", resp.status());
    }
    let info: DeviceInfo = resp.json().await.context("对方不是 LocalTransfer 设备")?;
    if info.v != crate::proto::PROTOCOL_VERSION {
        bail!("协议版本不兼容（对方 v{}）", info.v);
    }
    Ok(info)
}

// ---------------------------------------------------------------------------
// 计数流：包装文件读取，边读边发进度事件
// ---------------------------------------------------------------------------

struct CountingStream {
    inner: ReaderStream<tokio::fs::File>,
    transferred: u64,
    total: u64,
    transfer_id: String,
    file_id: String,
    event_tx: Sender<CoreEvent>,
    throttle: ProgressThrottle,
    cancel: CancellationToken,
}

impl futures::Stream for CountingStream {
    // StreamBody（新版 http-body-util）要求产出 Frame
    type Item = std::io::Result<http_body::Frame<bytes::Bytes>>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // 所有字段 Unpin（ReaderStream<File> Unpin）
        let this = self.get_mut();
        if this.cancel.is_cancelled() {
            return std::task::Poll::Ready(Some(Err(std::io::Error::other("已取消"))));
        }
        match std::pin::Pin::new(&mut this.inner).poll_next(cx) {
            std::task::Poll::Ready(Some(Ok(bytes))) => {
                this.transferred += bytes.len() as u64;
                if this.throttle.allow() || this.transferred >= this.total {
                    let _ = this.event_tx.try_send(CoreEvent::FileProgress {
                        transfer_id: this.transfer_id.clone(),
                        file_id: this.file_id.clone(),
                        transferred: this.transferred,
                        total: this.total,
                        done: this.transferred >= this.total,
                        path: None,
                    });
                }
                std::task::Poll::Ready(Some(Ok(http_body::Frame::data(bytes))))
            }
            std::task::Poll::Ready(Some(Err(e))) => std::task::Poll::Ready(Some(Err(e))),
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

// ---------------------------------------------------------------------------
// 文件收集：把用户选的路径（文件/文件夹混合）展开为扁平 FileMeta 列表
// ---------------------------------------------------------------------------

/// rel_path 规则：
/// - 单个文件 → 文件名
/// - 文件夹 → 以该文件夹自身名字为根的相对路径（接收方还原出整个文件夹）
pub fn collect_files(paths: &[PathBuf]) -> Result<Vec<(FileMeta, PathBuf)>> {
    let mut out = Vec::new();
    for p in paths {
        let meta = std::fs::symlink_metadata(p)
            .with_context(|| format!("读取 {} 失败", p.display()))?;
        if meta.is_dir() {
            // 文件夹本身要传：rel_path 前缀 = 文件夹名
            collect_dir(p, &file_name_of(p), &mut out)?;
        } else if meta.is_file() {
            let name = file_name_of(p);
            out.push((
                FileMeta {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: name.clone(),
                    rel_path: name,
                    size: meta.len(),
                },
                p.clone(),
            ));
        }
        // 其他类型（符号链接指向特殊对象等）跳过
    }
    if out.is_empty() {
        bail!("没有可发送的文件");
    }
    if out.len() > 4096 {
        bail!("文件数过多（{}），请分批发送", out.len());
    }
    Ok(out)
}

fn collect_dir(dir: &Path, prefix: &str, out: &mut Vec<(FileMeta, PathBuf)>) -> Result<()> {
    let rd = std::fs::read_dir(dir)
        .with_context(|| format!("读取目录 {} 失败", dir.display()))?;
    for entry in rd {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            collect_dir(&path, &format!("{prefix}/{}", file_name_of(&path)), out)?;
        } else if meta.is_file() {
            out.push((
                FileMeta {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: file_name_of(&path),
                    rel_path: format!("{prefix}/{}", file_name_of(&path)),
                    size: meta.len(),
                },
                path.clone(),
            ));
        }
    }
    Ok(())
}

fn file_name_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into())
}

/// 传输完成后的"打开目标"（用于 reveal_path：资源管理器打开上一级并选中）：
/// 文件夹内文件 → 所在文件夹根（选中文件夹）；根目录散文件 → 文件本身。
/// saved 优先（接收方落盘路径），其次 source（发送方本地路径）。
pub fn open_target(
    rel_path: &str,
    saved: Option<&Path>,
    source: Option<&Path>,
) -> Option<PathBuf> {
    let p = saved.or(source)?.to_path_buf();
    // rel_path "photos/sub/a.jpg" → 根是往上数 (深度-1) 层
    let depth = rel_path.split('/').count();
    if depth >= 2 {
        Some(p.ancestors().nth(depth - 1)?.to_path_buf())
    } else {
        Some(p)
    }
}

/// 一批文件的显示名：全部位于同一文件夹（rel_path 首段一致且存在子路径）
/// → 文件夹名；混合/散文件 → "N 个文件"。
pub fn batch_display_name(rel_paths: &[&str]) -> String {
    let mut folder: Option<String> = None;
    for rel in rel_paths {
        let mut segs = rel.split('/');
        let first = segs.next().unwrap_or_default().to_string();
        if segs.next().is_none() {
            return format!("{} 个文件", rel_paths.len());
        }
        match &folder {
            Some(name) if name != &first => return format!("{} 个文件", rel_paths.len()),
            _ => folder = Some(first),
        }
    }
    folder.unwrap_or_else(|| format!("{} 个文件", rel_paths.len()))
}

/// 一组已保存路径的最长公共祖先目录。历史卡片没有 rel_path，
/// 用它从实际落盘路径反推文件夹根（同一文件夹下落的文件，
/// 公共祖先就是文件夹根；不同子树的文件会停在共同父目录）。
pub fn common_ancestor_dir(paths: &[PathBuf]) -> Option<PathBuf> {
    if paths.len() < 2 {
        return None;
    }
    let mut anc: Vec<_> = paths[0].components().collect();
    for p in &paths[1..] {
        let comps: Vec<_> = p.components().collect();
        let mut i = 0;
        while i < anc.len() && i < comps.len() && anc[i] == comps[i] {
            i += 1;
        }
        anc.truncate(i);
    }
    if anc.is_empty() {
        return None;
    }
    let dir: PathBuf = anc.iter().collect();
    // 公共前缀可能正好等于某条路径（同名重复）——退回其父目录
    if paths.iter().any(|p| p == &dir) {
        dir.parent().map(|p| p.to_path_buf())
    } else {
        Some(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_flat_and_nested() {
        let dir = std::env::temp_dir().join(format!("lt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub/deep")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        std::fs::write(dir.join("sub/b.bin"), [0u8; 1024]).unwrap();
        std::fs::write(dir.join("sub/deep/c.md"), b"x").unwrap();
        let folder = dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let files = collect_files(&[dir.clone()]).unwrap();
        assert_eq!(files.len(), 3);
        let rels: Vec<&str> = files.iter().map(|(m, _)| m.rel_path.as_str()).collect();
        // 文件夹本身作为 rel_path 根，接收方还原出整个文件夹
        assert!(rels.contains(&format!("{folder}/a.txt").as_str()));
        assert!(rels.contains(&format!("{folder}/sub/b.bin").as_str()));
        assert!(rels.contains(&format!("{folder}/sub/deep/c.md").as_str()));
        let sizes: Vec<u64> = files.iter().map(|(m, _)| m.size).collect();
        assert!(sizes.contains(&5));
        assert!(sizes.contains(&1024));

        // 单文件路径
        let single = collect_files(&[dir.join("a.txt")]).unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].0.rel_path, "a.txt");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn collect_empty_fails() {
        let dir = std::env::temp_dir().join(format!("lt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(collect_files(&[dir.clone()]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_target_math() {
        // 文件夹内（深度 3）→ 文件夹根往上数 2 层
        let p = open_target(
            "mydocs/sub/b.md",
            Some(Path::new("/d/mydocs/sub/b.md")),
            None,
        )
        .unwrap();
        assert_eq!(p, PathBuf::from("/d/mydocs"));
        // 深度 2 → 直接父目录
        let p = open_target("mydocs/a.txt", Some(Path::new("/d/mydocs/a.txt")), None).unwrap();
        assert_eq!(p, PathBuf::from("/d/mydocs"));
        // 根目录散文件 → reveal 文件本身
        let p = open_target("a.txt", Some(Path::new("/d/a.txt")), None).unwrap();
        assert_eq!(p, PathBuf::from("/d/a.txt"));
        // 发送方：无 saved 用 source
        let p = open_target("photos/x.jpg", None, Some(Path::new("/tmp/photos/x.jpg"))).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/photos"));
        // 两者皆无 → None
        assert!(open_target("a/b", None, None).is_none());
    }

    #[test]
    fn batch_name_rules() {
        assert_eq!(batch_display_name(&["mydocs/a.txt", "mydocs/sub/b.md"]), "mydocs");
        assert_eq!(batch_display_name(&["loose.txt", "mydocs/a.txt"]), "2 个文件");
        assert_eq!(batch_display_name(&["a/x", "b/y"]), "2 个文件");
        assert_eq!(batch_display_name(&["only/a"]), "only");
    }

    #[test]
    fn ancestor_dir_math() {
        use std::path::Path;
        // 同一文件夹（含子目录）→ 文件夹根
        let d = common_ancestor_dir(&[
            PathBuf::from("/d/mydocs/a.txt"),
            PathBuf::from("/d/mydocs/sub/b.md"),
        ])
        .unwrap();
        assert_eq!(d, Path::new("/d/mydocs"));
        // 不同文件夹 → 共同父目录（下载根）
        let d = common_ancestor_dir(&[
            PathBuf::from("/d/a/x.txt"),
            PathBuf::from("/d/b/y.txt"),
        ])
        .unwrap();
        assert_eq!(d, Path::new("/d"));
        // 少于两条 → None（单文件走 reveal）
        assert!(common_ancestor_dir(&[PathBuf::from("/d/a")]).is_none());
        // Windows 风格
        let d = common_ancestor_dir(&[
            PathBuf::from(r"C:\dl\fd\a.txt"),
            PathBuf::from(r"C:\dl\fd\b.txt"),
        ])
        .unwrap();
        assert_eq!(d, PathBuf::from(r"C:\dl\fd"));
    }
}
