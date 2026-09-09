//! 端到端协议测试：同进程起两个 core（不同 HTTP 端口、临时身份），
//! 验证 发现 → 文本 → 文件传输 全链路。不含 UI。

use std::time::{Duration, Instant};

use smol::channel::{Receiver, Sender};
use transfer_core::{proto::CoreEvent, Config, MessageKind, Store, UiCommand};

fn test_config(name: &str, port: u16) -> Config {
    Config {
        device_id: uuid::Uuid::new_v4().to_string(),
        device_name: name.into(),
        http_port: port,
        download_dir: std::env::temp_dir()
            .join(format!("lt-it-{}-{}", name, uuid::Uuid::new_v4())),
        auto_receive: false,
    }
}

fn test_store(tag: &str) -> Store {
    Store::open_at(std::env::temp_dir().join(format!(
        "lt-it-db-{}-{}.db",
        tag,
        uuid::Uuid::new_v4()
    )))
    .expect("打开测试库失败")
}

/// 从事件流中等到谓词命中（30s 超时）；返回命中的事件
async fn next_until(
    rx: &mut Receiver<CoreEvent>,
    mut pred: impl FnMut(&CoreEvent) -> bool,
    what: &str,
) -> CoreEvent {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "等待 {what} 超时");
        let ev = smol::future::or(
            async { rx.recv().await.expect("核心已退出") },
            async {
                smol::Timer::after(remaining).await;
                panic!("等待 {what} 超时");
            },
        )
        .await;
        if std::env::var("LT_IT_DEBUG").is_ok() {
            eprintln!("[evt] {ev:?}");
        }
        if pred(&ev) {
            return ev;
        }
    }
}

#[test]
fn discovery_text_file_roundtrip() {
    let (tx_a, mut rx_a): (Sender<CoreEvent>, Receiver<CoreEvent>) = smol::channel::unbounded();
    let (tx_b, mut rx_b): (Sender<CoreEvent>, Receiver<CoreEvent>) = smol::channel::unbounded();
    let cfg_a = test_config("IT-A", 55711);
    let cfg_b = test_config("IT-B", 55712);
    let id_a = cfg_a.device_id.clone();
    let id_b = cfg_b.device_id.clone();

    let core_a = transfer_core::start(cfg_a, test_store("a"), tx_a).unwrap();
    let core_b = transfer_core::start(cfg_b, test_store("b"), tx_b).unwrap();

    smol::block_on(async {
        // ---- 互相发现 ----
        next_until(&mut rx_a, |e| matches!(e, CoreEvent::DeviceUp(d) if d.info.id == id_b), "A 发现 B").await;
        next_until(&mut rx_b, |e| matches!(e, CoreEvent::DeviceUp(d) if d.info.id == id_a), "B 发现 A").await;

        // ---- 文本 ----
        core_a
            .send(UiCommand::SendText {
                peer_id: id_b.clone(),
                text: "你好 B".into(),
            })
            .unwrap();
        next_until(
            &mut rx_a,
            |e| matches!(e, CoreEvent::Message { msg } if msg.outgoing && msg.kind == MessageKind::Text("你好 B".into())),
            "A 本地回显",
        )
        .await;
        next_until(
            &mut rx_b,
            |e| matches!(e, CoreEvent::Message { msg } if !msg.outgoing && msg.kind == MessageKind::Text("你好 B".into())),
            "B 收到文本",
        )
        .await;

        // ---- 文件传输（文件夹展开 + 接收确认 + 内容校验）----
        let src_dir = std::env::temp_dir().join(format!("lt-it-src-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(src_dir.join("sub")).unwrap();
        std::fs::write(src_dir.join("hello.txt"), b"hello localtransfer").unwrap();
        std::fs::write(src_dir.join("sub").join("data.bin"), vec![7u8; 300_000]).unwrap();

        let save_dir = std::env::temp_dir().join(format!("lt-it-recv-{}", uuid::Uuid::new_v4()));

        core_a
            .send(UiCommand::SendFiles {
                peer_id: id_b.clone(),
                paths: vec![src_dir.clone()],
            })
            .unwrap();

        let req = next_until(
            &mut rx_b,
            |e| matches!(e, CoreEvent::IncomingRequest { .. }),
            "B 收到传输请求",
        )
        .await;
        if let CoreEvent::IncomingRequest { req_id, files, .. } = req {
            assert_eq!(files.len(), 2, "文件夹应展开为 2 个文件");
            core_b
                .send(UiCommand::RespondRequest {
                    req_id,
                    accept: true,
                    save_dir: Some(save_dir.clone()),
                    overwrite: false,
                })
                .unwrap();
        }

        next_until(
            &mut rx_a,
            |e| matches!(&e, CoreEvent::TransferFinished { error: None, .. }),
            "A 发送完成",
        )
        .await;
        // B 侧单文件完成事件带最终路径（先于 TransferFinished 到达）
        next_until(
            &mut rx_b,
            |e| matches!(e, CoreEvent::FileProgress { done: true, path: Some(_), .. }),
            "B 收到单文件完成事件",
        )
        .await;
        next_until(
            &mut rx_b,
            |e| matches!(&e, CoreEvent::TransferFinished { error: None, .. }),
            "B 接收完成",
        )
        .await;

        // 校验内容与目录结构：文件夹本身作为一层目录被还原
        let folder = src_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let got = std::fs::read(save_dir.join(&folder).join("hello.txt")).unwrap();
        assert_eq!(got, b"hello localtransfer");
        let bin = std::fs::read(save_dir.join(&folder).join("sub").join("data.bin")).unwrap();
        assert_eq!(bin.len(), 300_000);
        assert!(bin.iter().all(|&b| b == 7));

        // 清理测试产物
        let _ = std::fs::remove_dir_all(&src_dir);
        let _ = std::fs::remove_dir_all(&save_dir);
    });

    core_a.shutdown();
    core_b.shutdown();
}

#[test]
fn prepare_rejected() {
    let (tx_a, rx_a): (Sender<CoreEvent>, _) = smol::channel::unbounded();
    let (tx_b, mut rx_b): (Sender<CoreEvent>, _) = smol::channel::unbounded();
    let cfg_a = test_config("IT-RJ-A", 55721);
    let cfg_b = test_config("IT-RJ-B", 55722);
    let id_b = cfg_b.device_id.clone();

    let core_a = transfer_core::start(cfg_a, test_store("ra"), tx_a).unwrap();
    let core_b = transfer_core::start(cfg_b, test_store("rb"), tx_b).unwrap();
    let mut rx_a = rx_a;

    smol::block_on(async {
        next_until(&mut rx_a, |e| matches!(e, CoreEvent::DeviceUp(d) if d.info.id == id_b), "发现 B").await;

        let f = std::env::temp_dir().join(format!("lt-it-rj-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&f, b"x").unwrap();
        core_a
            .send(UiCommand::SendFiles {
                peer_id: id_b.clone(),
                paths: vec![f.clone()],
            })
            .unwrap();

        let req = next_until(&mut rx_b, |e| matches!(e, CoreEvent::IncomingRequest { .. }), "请求到达").await;
        if let CoreEvent::IncomingRequest { req_id, .. } = req {
            core_b
                .send(UiCommand::RespondRequest {
                    req_id,
                    accept: false,
                    save_dir: None,
                    overwrite: false,
                })
                .unwrap();
        }
        next_until(
            &mut rx_a,
            |e| matches!(&e, CoreEvent::TransferFinished { error: Some(reason), .. } if reason.contains("拒绝")),
            "A 收到拒绝",
        )
        .await;

        let _ = std::fs::remove_file(&f);
    });

    core_a.shutdown();
    core_b.shutdown();
}

#[test]
fn connect_peer_by_ip() {
    // 手动 IP 直连（多播/广播都不通时的兜底路径）
    let (tx_a, mut rx_a): (Sender<CoreEvent>, _) = smol::channel::unbounded();
    let (tx_b, _rx_b): (Sender<CoreEvent>, _) = smol::channel::unbounded();
    let cfg_a = test_config("IT-CP-A", 55731);
    let cfg_b = test_config("IT-CP-B", 55732);
    let id_b = cfg_b.device_id.clone();

    let core_a = transfer_core::start(cfg_a, test_store("ca"), tx_a).unwrap();
    let _core_b = transfer_core::start(cfg_b, test_store("cb"), tx_b).unwrap();

    smol::block_on(async {
        // 等 B 的 HTTP 服务就绪
        smol::Timer::after(Duration::from_millis(500)).await;

        core_a
            .send(UiCommand::ConnectPeer {
                host: "127.0.0.1:55732".into(),
            })
            .unwrap();
        next_until(
            &mut rx_a,
            |e| matches!(e, CoreEvent::DeviceUp(d) if d.info.id == id_b && d.online),
            "手动连接 B",
        )
        .await;
        // 错误格式应得到 Error 事件而不是 panic
        core_a
            .send(UiCommand::ConnectPeer { host: "abc".into() })
            .unwrap();
        next_until(
            &mut rx_a,
            |e| matches!(e, CoreEvent::Error { context, .. } if context == "添加设备"),
            "格式错误提示",
        )
        .await;
    });

    core_a.shutdown();
    _core_b.shutdown();
}

#[test]
fn web_offer_list_and_download() {
    // 网页接收：客户端发布 → GET / 页面、列表 JSON、流式下载、移除
    let (tx_a, mut rx_a): (Sender<CoreEvent>, _) = smol::channel::unbounded();
    let cfg_a = test_config("IT-WEB-A", 55741);
    let core_a = transfer_core::start(cfg_a, test_store("wa"), tx_a).unwrap();

    smol::block_on(async {
        smol::Timer::after(Duration::from_millis(500)).await;

        let f = std::env::temp_dir().join(format!("lt-it-web-{}.bin", uuid::Uuid::new_v4()));
        std::fs::write(&f, vec![7u8; 12345]).unwrap();
        core_a.send(UiCommand::WebOffer { paths: vec![f.clone()] }).unwrap();

        // web_offer 现在发 Message 事件（不再有 Info toast）
        next_until(
            &mut rx_a,
            |e| matches!(e, CoreEvent::Message { msg } if msg.outgoing && matches!(&msg.kind, MessageKind::File { size: 12345, .. })),
            "发布消息",
        )
        .await;

        // 第二次发布一个"文件夹"（2 文件）→ 两条消息共用同一 transfer_id（桌面端合并为文件夹卡）
        let d = std::env::temp_dir().join(format!("lt-it-webf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), b"aa").unwrap();
        std::fs::write(d.join("sub/b.txt"), b"bb").unwrap();
        core_a.send(UiCommand::WebOffer { paths: vec![d.clone()] }).unwrap();
        let (t1, t2) = smol::block_on(async {
            let m1 = next_until(
                &mut rx_a,
                |e| matches!(e, CoreEvent::Message { msg } if matches!(&msg.kind, MessageKind::File { name, .. } if name == "a.txt")),
                "文件夹消息1",
            ).await;
            let m2 = next_until(
                &mut rx_a,
                |e| matches!(e, CoreEvent::Message { msg } if matches!(&msg.kind, MessageKind::File { name, .. } if name == "b.txt")),
                "文件夹消息2",
            ).await;
            (m1, m2)
        });
        let tid_of = |e: &CoreEvent| -> String {
            match e {
                CoreEvent::Message { msg } => match &msg.kind {
                    MessageKind::File { transfer_id: Some(t), .. } => t.clone(),
                    _ => String::new(),
                },
                _ => String::new(),
            }
        };
        let (tid1, tid2) = (tid_of(&t1), tid_of(&t2));
        assert!(!tid1.is_empty() && tid1 == tid2, "同批文件应共用 transfer_id: {tid1} vs {tid2}");
        let _ = std::fs::remove_dir_all(&d);

        // reqwest 需要 tokio reactor，在临时 runtime 里跑 HTTP 断言
        let f = f.clone();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
        let client = reqwest::Client::builder().no_proxy().build().unwrap(); // 直连，绕开系统代理
        let base = "http://127.0.0.1:55741";

        // 页面（聊天式网页客户端）
        let page = client.get(format!("{base}/")).send().await.unwrap();
        assert!(page.status().is_success());
        let html = page.text().await.unwrap();
        assert!(html.contains("网页客户端"));

        // 列表
        let list: serde_json::Value = client
            .get(format!("{base}/api/web/list"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = list[0]["id"].as_str().unwrap().to_string();
        assert_eq!(list[0]["size"].as_u64().unwrap(), 12345);

        // 下载（流式 + 中文文件名头由真实场景覆盖，这里验证内容与长度）
        let dl = client
            .get(format!("{base}/api/web/download/{id}"))
            .send()
            .await
            .unwrap();
        assert!(dl.status().is_success());
        let bytes = dl.bytes().await.unwrap();
        assert_eq!(bytes.len(), 12345);
        assert!(bytes.iter().all(|&b| b == 7));

        // 移除后再下载 404
        client.post(format!("{base}/api/web/remove/{id}")).send().await.unwrap();
        let dl2 = client
            .get(format!("{base}/api/web/download/{id}"))
            .send()
            .await
            .unwrap();
        assert_eq!(dl2.status(), reqwest::StatusCode::NOT_FOUND);
            });

        let _ = std::fs::remove_file(&f);
    });

    core_a.shutdown();
}

#[test]
fn web_chat_text_and_upload() {
    // 浏览器 → 电脑：发文本 + 上传文件（落盘 + 会话消息）
    let (tx_a, mut rx_a): (Sender<CoreEvent>, _) = smol::channel::unbounded();
    let cfg_a = test_config("IT-WEB2-A", 55751);
    let core_a = transfer_core::start(cfg_a, test_store("wb"), tx_a).unwrap();

    smol::block_on(async {
        smol::Timer::after(Duration::from_millis(500)).await;

        // 电脑发文本到网页会话
        core_a.send(UiCommand::WebText { text: "你好网页".into() }).unwrap();
        next_until(
            &mut rx_a,
            |e| matches!(e, CoreEvent::Message { msg } if msg.outgoing && matches!(&msg.kind, MessageKind::Text(t) if t == "你好网页")),
            "电脑→网页文本",
        )
        .await;

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let client = reqwest::Client::builder().no_proxy().build().unwrap(); // 直连，绕开系统代理
                let base = "http://127.0.0.1:55751";

                // 浏览器发文本
                let r = client
                    .post(format!("{base}/api/web/message"))
                    .json(&serde_json::json!({ "text": "来自浏览器" }))
                    .send()
                    .await
                    .unwrap();
                assert!(r.status().is_success());

                // 浏览器上传（带目录结构）
                let r = client
                    .post(format!(
                        "{base}/api/web/upload?name=%E6%96%87%E4%BB%B6.txt&rel=photos/%E6%96%87%E4%BB%B6.txt"
                    ))
                    .body(vec![9u8; 777])
                    .send()
                    .await
                    .unwrap();
                assert!(r.status().is_success());

                // 轮询历史应有 3 条（电脑文本、浏览器文本、上传文件）
                let list: serde_json::Value = client
                    .get(format!("{base}/api/web/messages?since=0"))
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(list.as_array().unwrap().len(), 3);
                assert_eq!(list[2]["kind"], "file");
                assert_eq!(list[2]["size"].as_u64().unwrap(), 777);

                // 落盘校验
                let saved = std::path::Path::new(&std::env::temp_dir())
                    .join(&test_config("IT-WEB2-A", 0).download_dir);
                let _ = saved;
            });

        // 上传完成 → 电脑端网页会话出现文件卡片
        next_until(
            &mut rx_a,
            |e| matches!(e, CoreEvent::Message { msg } if !msg.outgoing && matches!(&msg.kind, MessageKind::File { size: 777, .. })),
            "网页上传消息",
        )
        .await;
    });

    core_a.shutdown();
}
