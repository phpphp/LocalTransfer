//! Windows 系统托盘：最小化到托盘 + 未读闪烁 + 点击恢复窗口。
//! gpui 没有托盘/隐藏窗口的 API——托盘跑在自己的线程上，
//! 显示/隐藏主窗口用 Win32 按窗口标题枚举本进程窗口（不依赖 gpui 暴露 HWND）。

use std::sync::atomic::{AtomicBool, Ordering};

static BADGE: AtomicBool = AtomicBool::new(false);

/// 未读状态（true=托盘图标闪烁红点）
pub fn set_unread(has_unread: bool) {
    BADGE.store(has_unread, Ordering::Relaxed);
}

/// 托盘/通知图标的查找：exe 旁 → exe/assets → 开发时的仓库 assets/
fn icon_path(name: &str) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut candidates = Vec::new();
    if let Some(dir) = exe.parent() {
        candidates.push(dir.join(name));
        candidates.push(dir.join("assets").join(name));
    }
    candidates.push(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets")
            .join(name),
    );
    candidates.into_iter().find(|p| p.exists())
}

/// 启动托盘线程（常驻；图标缺失时静默降级为无托盘）
pub fn start() {
    #[cfg(target_os = "windows")]
    std::thread::spawn(run_tray);
    #[cfg(not(target_os = "windows"))]
    {}
}

#[cfg(target_os = "windows")]
fn run_tray() {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{TrayIconBuilder, TrayIconEvent};

    let normal = icon_path("icon-normal.png")
        .and_then(|p| tray_icon::Icon::from_path(p, None).ok());
    let badge = icon_path("icon-badge.png")
        .and_then(|p| tray_icon::Icon::from_path(p, None).ok());
    let Some(normal) = normal else {
        tracing::warn!("托盘图标缺失（icon-normal.png），托盘不可用");
        return;
    };

    let menu = Menu::new();
    let open = MenuItem::new("打开 LocalTransfer", true, None);
    let quit = MenuItem::new("退出", true, None);
    let _ = menu.append_items(&[&open, &quit]);
    let open_id = open.id().clone();
    let quit_id = quit.id().clone();

    let tray = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("LocalTransfer")
        .with_icon(normal.clone())
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("托盘创建失败: {e:?}");
            return;
        }
    };

    // Windows 要求：托盘所在线程必须跑 win32 消息循环（否则图标不显示）。
    // 闪烁与事件轮询挂在 WM_TIMER 上（无窗口 timer 投递到线程队列）。
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, SetTimer, TranslateMessage, MSG, WM_TIMER,
    };
    const TIMER_ID: usize = 1;
    unsafe {
        SetTimer(std::ptr::null_mut(), TIMER_ID, 700, None);
    }

    let mut blink_on = false;
    let mut tooltip_unread = false;
    let mut msg = MSG {
        hwnd: std::ptr::null_mut(),
        message: 0,
        wParam: 0,
        lParam: 0,
        time: 0,
        pt: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
    };
    'pump: loop {
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if r <= 0 {
            break 'pump; // WM_QUIT / 错误
        }
        if msg.message == WM_TIMER && msg.wParam == TIMER_ID {
            let unread = BADGE.load(Ordering::Relaxed);
            if unread && badge.is_some() {
                blink_on = !blink_on;
                let icon = if blink_on {
                    badge.clone().unwrap()
                } else {
                    normal.clone()
                };
                let _ = tray.set_icon(Some(icon));
            } else if blink_on {
                blink_on = false;
                let _ = tray.set_icon(Some(normal.clone()));
            }
            if tooltip_unread != unread {
                tooltip_unread = unread;
                let tip = if unread {
                    "LocalTransfer · 有未读消息"
                } else {
                    "LocalTransfer"
                };
                let _ = tray.set_tooltip(Some(tip));
            }
            // 托盘/菜单事件
            if let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                match ev {
                    TrayIconEvent::Click { .. } | TrayIconEvent::DoubleClick { .. } => {
                        show_main_window()
                    }
                    _ => {}
                }
            }
            if let Ok(ev) = MenuEvent::receiver().try_recv() {
                if ev.id == quit_id {
                    std::process::exit(0);
                } else if ev.id == open_id {
                    show_main_window();
                }
            }
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Win32：按标题找本进程主窗口
#[cfg(target_os = "windows")]
fn find_main_window() -> Option<isize> {
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::LPARAM;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    };

    struct Ctx {
        pid: u32,
        found: Option<isize>,
    }
    unsafe extern "system" fn proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // edition 2024：unsafe fn 体内也要显式 unsafe 块
        unsafe {
            let ctx = &mut *(lparam as *mut Ctx);
            let mut wpid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut wpid as *mut u32);
            if wpid == ctx.pid {
                let len = GetWindowTextLengthW(hwnd);
                if len > 0 {
                    let mut buf = vec![0u16; len as usize + 1];
                    let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
                    let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
                    if title == "LocalTransfer" {
                        ctx.found = Some(hwnd as isize);
                        return 0; // 找到即停（FALSE）
                    }
                }
            }
            1
        }
    }

    let mut ctx = Ctx {
        pid: std::process::id(),
        found: None,
    };
    unsafe {
        EnumWindows(Some(proc), &mut ctx as *mut Ctx as LPARAM);
    }
    ctx.found
}

/// 恢复并前置主窗口（托盘点击）
#[cfg(target_os = "windows")]
pub fn show_main_window() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };
    if let Some(h) = find_main_window() {
        unsafe {
            ShowWindow(h as _, SW_SHOW);
            ShowWindow(h as _, SW_RESTORE);
            SetForegroundWindow(h as _);
        }
    }
}

/// 隐藏主窗口（最小化到托盘：任务栏消失、托盘常驻）
#[cfg(target_os = "windows")]
pub fn hide_main_window() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    if let Some(h) = find_main_window() {
        unsafe {
            ShowWindow(h as _, SW_HIDE);
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn show_main_window() {}

#[cfg(not(target_os = "windows"))]
pub fn hide_main_window() {}
