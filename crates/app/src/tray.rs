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

/// 载入托盘图标：自己解码 PNG 成 RGBA 交给 tray-icon。
/// 不能用 Icon::from_path——它在 Windows 走 Win32 LoadImageW(IMAGE_ICON)，
/// 只认 .ico，喂 .png 直接返回空句柄（日志里"托盘图标缺失"就是这么来的）。
#[cfg(target_os = "windows")]
fn load_icon(name: &str) -> Option<tray_icon::Icon> {
    let path = icon_path(name)?;
    let img = image::ImageReader::open(&path).ok()?.decode().ok()?;
    let (w, h) = (img.width(), img.height());
    let rgba = img.into_rgba8().into_raw();
    tray_icon::Icon::from_rgba(rgba, w, h).ok()
}


/// 任务栏按钮角标（未读持续标记）。windows crate 绑定（ITaskbarList3）。
#[cfg(target_os = "windows")]
fn set_taskbar_overlay(hicon: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::HICON;
    use windows::core::HSTRING;
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let need_uninit = hr.is_ok();
        let created = CoCreateInstance::<_, ITaskbarList3>(
            &TaskbarList, None, CLSCTX_INPROC_SERVER,
        );
        match created {
            Ok(tbl) => {
                if let Some(hwnd) = find_main_window() {
                    // 0 = 摘除角标
                    let icon = HICON(if hicon != 0 {
                        hicon as *mut core::ffi::c_void
                    } else {
                        std::ptr::null_mut()
                    });
                    if let Err(e) = tbl.SetOverlayIcon(
                        HWND(hwnd as _), icon, &HSTRING::from("未读"),
                    ) {
                        tracing::warn!("任务栏角标失败: {e}");
                    }
                }
            }
            Err(e) => tracing::warn!("任务栏角标：ITaskbarList3 创建失败: {e}"),
        }
        if need_uninit {
            let _ = CoUninitialize();
        }
    }
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
    use tray_icon::menu::{Menu, MenuItem};
    use tray_icon::TrayIconBuilder;

    let normal = load_icon("icon-normal.png");
    let Some(normal) = normal else {
        tracing::warn!("托盘图标加载失败（icon-normal.png 解码失败或缺失），托盘不可用");
        return;
    };

    // 任务栏角标 HICON：用 badge PNG 的像素自建 16x16（LoadImageW 不认 PNG）
    // 任务栏角标 HICON：必须真实 ICO 资源（手搓 CreateIcon 的 32bpp 位图会被
    // SetOverlayIcon 拒收 E_INVALIDARG），16x16 小图标尺寸
    let overlay_hicon: isize = icon_path("icon-badge.ico")
        .map(|p| {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                LoadImageW, IMAGE_ICON, LR_DEFAULTSIZE, LR_LOADFROMFILE,
            };
            let wide: Vec<u16> = p.as_os_str().to_string_lossy()
                .encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                LoadImageW(std::ptr::null_mut(), wide.as_ptr(), IMAGE_ICON,
                           16, 16, LR_LOADFROMFILE | LR_DEFAULTSIZE) as usize as isize
            }
        })
        .filter(|h| *h != 0)
        .unwrap_or(0);

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
    // 闪烁挂在 WM_TIMER 上（无窗口 timer 投递到线程队列）；
    // 托盘/菜单事件在每条消息处理完立刻取——不能等 700ms 的 tick，
    // 否则点菜单后事件要等下一个 tick 才生效，表现成"没反应/要点两下"。
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, SetTimer, TranslateMessage, MSG, WM_TIMER,
    };
    // hWnd=NULL 时 Windows 无视传入 id、自分配新 id 并作为返回值——
    // 之前拿 wParam==1 匹配永远不成立（实测 id=4512），闪烁从未生效
    let timer_id = unsafe { SetTimer(std::ptr::null_mut(), 1, 700, None) };
    if timer_id == 0 {
        tracing::warn!("SetTimer 失败，托盘闪烁与事件将不可用");
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
        handle_events(&open_id, &quit_id);
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if r <= 0 {
            break 'pump; // WM_QUIT / 错误
        }
        if msg.message == WM_TIMER && msg.wParam == timer_id {
            let unread = BADGE.load(Ordering::Relaxed);
            // 未读闪烁 = 图标隐藏/显示交替（不用红点图——用户要求闪隐式）
            if unread {
                blink_on = !blink_on;
                if let Err(e) = tray.set_visible(!blink_on) {
                    tracing::warn!("托盘闪烁失败: {e:?}");
                }
                // 未读开始/结束：任务栏按钮挂/摘角标（持续高亮标记）
                if tooltip_unread != unread {
                    set_taskbar_overlay(if unread { overlay_hicon } else { 0 });
                }
            } else {
                if blink_on {
                    blink_on = false;
                    // 确保恢复显示
                    let _ = tray.set_visible(true);
                }
                if tooltip_unread != unread {
                    set_taskbar_overlay(0);
                }
            }
            if tooltip_unread != unread {
                tooltip_unread = unread;
                let tip = if unread {
                    "LocalTransfer · 有未读消息"
                } else {
                    "LocalTransfer"
                };
                if let Err(e) = tray.set_tooltip(Some(tip)) {
                    tracing::warn!("托盘 tooltip 更新失败: {e:?}");
                }
            }
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Dispatch 期间 muda 的 wndproc（菜单模态循环）会把事件塞进 channel，这里立刻取走
        handle_events(&open_id, &quit_id);
    }
}

/// 托盘图标 + 菜单事件处理（每条消息处理完立刻调用）
#[cfg(target_os = "windows")]
fn handle_events(open_id: &tray_icon::menu::MenuId, quit_id: &tray_icon::menu::MenuId) {
    use tray_icon::menu::MenuEvent;
    use tray_icon::{MouseButton, TrayIconEvent};
    while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
        match ev {
            // 只认左键——右键是打开菜单，不能跟着弹主窗口
            // （之前任何按键都弹窗：右键开菜单后主窗口又抢出来，菜单被打断，
            //  表现成"打开要点两下、退出点不动"）
            TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => show_main_window(),
            _ => {}
        }
    }
    while let Ok(ev) = MenuEvent::receiver().try_recv() {
        if ev.id == quit_id {
            std::process::exit(0);
        } else if ev.id == open_id {
            show_main_window();
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

/// 本机主窗口当前是否前台（事件时刻实时查询）。
/// 不能用渲染缓存的 window_active——切走窗口后不再重绘，缓存永远是 true，
/// 后台提醒（闪烁/通知）的条件永远不成立。
#[cfg(target_os = "windows")]
pub fn is_foreground() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    match find_main_window() {
        Some(h) => unsafe { GetForegroundWindow() as isize == h },
        None => false,
    }
}

#[cfg(not(target_os = "windows"))]
pub fn is_foreground() -> bool {
    false
}

/// 任务栏按钮橙色闪烁（仿微信：收到新消息且窗口不在前台时触发；
/// FLASHW_TIMERNOFG = 持续闪烁直到用户点回窗口。窗口藏在托盘时
/// 任务栏没有按钮可闪——那种场景靠托盘红点闪烁提醒）
#[cfg(target_os = "windows")]
pub fn flash_taskbar() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FlashWindowEx, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG,
    };
    if let Some(h) = find_main_window() {
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd: h as _,
            // 闪 5 次即停（不无限闪）；持续提醒由任务栏角标 + 托盘闪烁承担
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: 5,
            dwTimeout: 0,
        };
        unsafe { FlashWindowEx(&info) };
    }
}

#[cfg(not(target_os = "windows"))]
pub fn flash_taskbar() {}

#[cfg(not(target_os = "windows"))]
pub fn show_main_window() {}

#[cfg(not(target_os = "windows"))]
pub fn hide_main_window() {}
