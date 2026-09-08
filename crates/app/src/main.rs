//! LocalTransfer 入口：解析参数 → 起核心线程 → 起 GPUI 窗口
// Windows 下不显示控制台（日志写入各平台配置目录的 LocalTransfer/logs/）
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
// gpui 的宏链较深，#[test] 展开会撞默认递归上限
#![recursion_limit = "256"]

mod chat;
mod modals;
mod root;
mod sidebar;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;
use transfer_core::{Config, Store};

use crate::root::RootView;

fn main() {
    // RDP 远程桌面会话下 DirectComposition 行为异常（窗口黑屏/画面错位——
    // gpui 的 WS_EX_NOREDIRECTIONBITMAP + DComp 组合在 RDP 管道里不可靠），
    // 检测到 RDP 会话时自动改用传统呈现路径。
    // 必须在任何 gpui 初始化之前设置；main 开头单线程，unsafe 安全。
    if let Ok(session) = std::env::var("SESSIONNAME") {
        if session.to_ascii_uppercase().starts_with("RDP-") {
            unsafe {
                std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true");
            }
        }
    }

    // 调试参数：--name <名> --port <端口>（同机双实例测试；会使用临时身份，不写回配置）
    let args: Vec<String> = std::env::args().collect();
    let mut name_override: Option<String> = None;
    let mut port_override: Option<u16> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--name" if i + 1 < args.len() => {
                name_override = Some(args[i + 1].clone());
                i += 2;
            }
            "--port" if i + 1 < args.len() => {
                port_override = args[i + 1].parse().ok();
                i += 2;
            }
            _ => i += 1,
        }
    }

    let mut config = Config::load_or_create().expect("加载配置失败");

    // 单实例锁：同机第 2+ 个实例自动换临时身份（否则共用 device_id，
    // 互相把对方广播当成自己过滤掉 → 多开互不可见）。
    // 锁 listener 故意 forget 保活到进程结束——内核在进程退出时回收 socket，
    // 若在此主动 drop 会立刻释放锁，第 2 个实例就能拿到同一把锁（bug）。
    let (instance_lock, instance_no) = transfer_core::acquire_instance_lock();
    std::mem::forget(instance_lock);
    let ephemeral = instance_no > 1 || name_override.is_some() || port_override.is_some();
    if let Some(n) = &name_override {
        config.device_name = n.clone();
    } else if instance_no > 1 && instance_no < 99 {
        config.device_name = format!("{} ({})", config.device_name, instance_no);
    } else if instance_no == 99 {
        // 锁端口耗尽（≥10 个实例），随机后缀兜底
        let suffix: String = uuid::Uuid::new_v4().simple().to_string().chars().take(4).collect();
        config.device_name = format!("{} ({suffix})", config.device_name);
    }
    if let Some(p) = port_override {
        config.http_port = p;
    }
    if ephemeral {
        config.device_id = uuid::Uuid::new_v4().to_string();
    }

    let ui_store = Store::open().expect("打开历史数据库失败");
    let core_store = Store::open().expect("打开核心数据库失败");
    let (event_tx, event_rx) = smol::channel::unbounded();

    // 端口被占用时自动向后找可用端口（不写回配置，下次启动仍从配置端口找起）
    match transfer_core::pick_free_port(config.http_port, 20) {
        Some(port) if port != config.http_port => {
            let _ = event_tx.try_send(transfer_core::CoreEvent::Info(format!(
                "端口 {} 已被占用，本次改用 {}",
                config.http_port, port
            )));
            config.http_port = port;
        }
        Some(_) => {}
        None => {
            let _ = event_tx.try_send(transfer_core::CoreEvent::Error {
                context: "端口".into(),
                message: format!(
                    "端口 {} 至 {} 均被占用，传输服务可能无法启动",
                    config.http_port,
                    config.http_port.saturating_add(19)
                ),
            });
        }
    }

    let core = transfer_core::start(config.clone(), core_store, event_tx)
        .expect("启动核心失败");

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            // 组件库内置文案（输入框右键菜单的 剪切/复制/粘贴/全选 等）切中文，
            // rust-i18n 全局 locale，默认 en
            gpui_kit::component::set_locale("zh-CN");

            // 显式绝对坐标窗口（WindowBounds::centered 在部分 Windows 显示器配置下
            // 会被 check_given_bounds 判无效而退回系统默认小窗）；
            // 多实例按序号级联错位，避免窗口完全重叠
            let win_size = size(px(1040.), px(680.));
            let cascade = ((instance_no.saturating_sub(1)) as f32) * 34.0;
            let bounds = Bounds {
                origin: point(px(140. + cascade), px(70. + cascade * 0.7)),
                size: win_size,
            };
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(win_size),
                ..TitleBar::window_options()
            };

            cx.spawn(async move |cx| {
                cx.open_window(options, |window, cx| {
                    window.set_window_title("LocalTransfer");
                    // 双保险：部分环境下初始 bounds 会被平台层判无效退回默认小窗，
                    // 创建后显式再设一次尺寸
                    window.resize(win_size);
                    let view = cx.new(|cx| {
                        RootView::new(config, ui_store, core, event_rx, window, cx)
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("打开窗口失败");
            })
            .detach();
        });
}
