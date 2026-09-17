//! 左侧边栏：本机卡片 + 设备列表（在线/离线分组）+ 添加设备
//!
//! 布局纪律（这套 GPUI 里踩过的坑）：
//! - `h_flex()` 自带 `items_center`，`v_flex()` 不带任何交叉轴对齐（子项默认拉伸）；
//!   放在行里的列必须显式 `h_full()`，否则只拿内容高度并被垂直居中。
//! - 固定尺寸元素一律 `flex_none()`，否则会被同行内容挤压变形（头像被压成竖条就是这个）。
//! - 需要单行显示的文本容器给 `w_full().min_w_0().truncate()`：先有确定宽度再截断，
//!   不要靠 flex 收缩去决定文本宽度（会按 min-content 换行，中文变成一字一行）。
//! - 列表行给确定高度，避免任何撑高。

use gpui_kit::component::button::Button;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::root::{RootView, initial_avatar};

/// 侧栏宽度
pub const SIDEBAR_W: f32 = 244.;
/// 设备行高
const ROW_H: f32 = 40.;

pub fn plat_label(plat: &str) -> String {
    match plat.to_ascii_lowercase().as_str() {
        "windows" => "Windows".into(),
        "mac" | "macos" => "Mac".into(),
        "linux" => "Linux".into(),
        "ios" => "iPhone".into(),
        "android" => "Android".into(),
        other => other.to_string(),
    }
}

/// 平台图标（div 拼几何形状——gpui 的 svg() 只认资产路径，data: URI 渲染为空）。
/// 颜色跟主题；16px 座内 11px 主体。
pub fn plat_svg(plat: &str, size: f32, color: Hsla) -> impl IntoElement {
    let body = px(size * 0.68);   // 主体尺寸
    match plat.to_ascii_lowercase().as_str() {
        // 监视器：屏幕 + 底部支架（Windows / Linux）
        "windows" | "linux" => div()
            .flex_none()
            .size(px(size))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(size * 0.08))
            .child(
                div()
                    .size(px(size * 0.85))
                    .rounded(px(size * 0.12))
                    .border_1()
                    .border_color(color),
            )
            .child(
                div()
                    .w(px(size * 0.45))
                    .h(px(1.))
                    .bg(color),
            ),
        // 笔记本：屏幕 + 底座横线（Mac）
        "mac" | "macos" => div()
            .flex_none()
            .size(px(size))
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .gap(px(size * 0.1))
            .child(
                div()
                    .size(px(size * 0.8))
                    .rounded_t(px(size * 0.1))
                    .border_1()
                    .border_color(color),
            )
            .child(
                div()
                    .w(px(size))
                    .h(px(1.))
                    .rounded_full()
                    .bg(color),
            ),
        // 手机：竖圆角矩形 + 底部圆点（iPhone / Android 通用）
        _ => div()
            .flex_none()
            .size(px(size))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .size(body)
                    .rounded(px(size * 0.18))
                    .border_1()
                    .border_color(color)
                    .flex()
                    .items_end()
                    .justify_center()
                    .pb(px(1.))
                    .child(div().w(px(size * 0.16)).h(px(1.)).bg(color)),
            ),
    }
}

impl RootView {
    pub fn render_sidebar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 只显示在线设备（离线的不再列出——用户要求）
        let mut online: Vec<(String, String, String)> = self
            .devices
            .iter()
            .filter(|d| d.online)
            .map(|d| (d.info.id.clone(), d.info.name.clone(), d.info.plat.clone()))
            .collect();
        online.sort_by(|a, b| a.1.cmp(&b.1));

        let me_name = self.me.effective_name();
        // HTTP Server 会话的未读数（入口在顶部本机卡片）
        let web_unread = self
            .unread
            .get(transfer_core::proto::WEB_PEER_ID)
            .copied()
            .unwrap_or(0);
        // 本机地址：优先真实局域网 IP，取不到时退回环回提示
        let me_addr = match (&self.local_ip, self.me.http_port) {
            (Some(ip), port) => format!("{ip}:{port}"),
            (None, port) => format!("127.0.0.1:{port}"),
        };
        let empty = online.is_empty();

        v_flex()
            .id("sidebar")
            .w(px(self.sidebar_w))
            .h_full()
            .flex_none()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            // 本机卡片：名称 + 地址；右侧是 HTTP Server（网页客户端）入口
            .child(
                h_flex()
                    .id("me-card")
                    .flex_none()
                    .w_full()
                    .h(px(64.))
                    .px_3()
                    .gap_2p5()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(initial_avatar(&me_name, 36., cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .line_height(px(20.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(me_name),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        div()
                                            .flex_none()
                                            .size(px(6.))
                                            .rounded_full()
                                            .bg(cx.theme().success),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_xs()
                                            .line_height(px(16.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(me_addr),
                                    ),
                            ),
                    )
                    // HTTP Server（网页客户端）会话入口：未读时带角标
                    .child(
                        h_flex()
                            .id("btn-http-server")
                            .flex_none()
                            .gap_1()
                            .px_2()
                            .h(px(32.))
                            .items_center()
                            .rounded_lg()
                            .cursor_pointer()
                            .hover(|s| s.bg(cx.theme().list_hover))
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                this.select_peer(transfer_core::proto::WEB_PEER_ID, cx);
                                cx.notify();
                            }))
                            .child(
                                Icon::new(IconName::Globe)
                                    .with_size(px(16.))
                                    .text_color(cx.theme().primary),
                            )
                            .when(web_unread > 0, |el| {
                                el.child(
                                    h_flex()
                                        .flex_none()
                                        .h(px(16.))
                                        .min_w(px(16.))
                                        .px_1()
                                        .justify_center()
                                        .rounded_full()
                                        .bg(cx.theme().danger)
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().danger_foreground)
                                                .child(if web_unread > 99 {
                                                    "99+".to_string()
                                                } else {
                                                    web_unread.to_string()
                                                }),
                                        ),
                                )
                            }),
                    ),
            )
            // 设备列表（滚动区）
            .child(
                v_flex()
                    .id("sidebar-list")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .px_2()
                    .py_1()
                    .gap_0p5()
                    // 顶部功能行：添加设备 + 本机二维码 + 设置
                    .child(
                        h_flex()
                            .id("sidebar-tools")
                            .flex_none()
                            .w_full()
                            .gap_2()
                            .pb_1()
                            .child(
                                div().flex_1().min_w_0().child(
                                    Button::new("btn-add-peer")
                                        .outline()
                                        .small()
                                        .w_full()
                                        .icon(IconName::Plus)
                                        .label("添加设备")
                                        .on_click(cx.listener(|this, _ev, _window, cx| {
                                            this.show_connect = true;
                                            cx.notify();
                                        })),
                                ),
                            )
                            .child(
                                Button::new("btn-qr")
                                    .outline()
                                    .small()
                                    .flex_none()
                                    .icon(IconName::LayoutDashboard)
                                    .tooltip("本机二维码（手机扫码添加 / 网页客户端）")
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        this.show_web_qr = true;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("btn-settings")
                                    .outline()
                                    .small()
                                    .flex_none()
                                    .icon(IconName::Settings)
                                    .tooltip("设置")
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        this.show_settings = !this.show_settings;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(!online.is_empty(), |el| {
                        el.child(self.group_label("在线", online.len(), cx))
                            .children(
                                online
                                    .iter()
                                    .map(|(id, name, plat)| {
                                        self.device_row(id, name, plat, true, cx)
                                    }),
                            )
                    })
                    .when(empty, |el| el.child(self.sidebar_empty(cx))),
            )
    }

    fn group_label(&self, label: &str, count: usize, cx: &Context<Self>) -> AnyElement {
        h_flex()
            .id(SharedString::from(format!("group-{label}")))
            .flex_none()
            .w_full()
            .px_2()
            .pt_2()
            .pb_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{label} · {count}")),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn device_row(
        &self,
        id: &str,
        name: &str,
        plat: &str,
        online: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let selected = self.selected.as_deref() == Some(id);
        let unread = self.unread.get(id).copied().unwrap_or(0);
        let id_for_click = id.to_string();
        let hover_bg = cx.theme().list_hover;
        let icon_color = if online {
            cx.theme().primary
        } else {
            cx.theme().muted_foreground
        };

        h_flex()
            .id(SharedString::from(format!("dev-{id}")))
            .flex_none()
            .w_full()
            .h(px(ROW_H))
            .px_2()
            .gap_2p5()
            .rounded_lg()
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |el| {
                el.bg(cx.theme().sidebar_accent)
                    .text_color(cx.theme().sidebar_accent_foreground)
            })
            .when(!selected, |el| el.hover(move |s| s.bg(hover_bg)))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                this.select_peer(&id_for_click, cx);
                cx.notify();
            }))
            // 首字头像（设备名首字，圆底）
            .child(initial_avatar(name, 22., cx))
            // 名字（吃掉剩余宽度）
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .when(!online, |el| el.text_color(cx.theme().muted_foreground))
                    .child(name.to_string()),
            )
            // 设备类型：图标 + 平台名紧贴（整组 flex_none 贴右侧）
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(plat_svg(plat, 10., icon_color))
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(plat_label(plat)),
                    ),
            )
            // 在线状态点
            .child(
                div()
                    .flex_none()
                    .ml_1()
                    .size(px(7.))
                    .rounded_full()
                    .when(online, |el| el.bg(cx.theme().success))
                    .when(!online, |el| {
                        el.bg(cx.theme().muted_foreground).opacity(0.35)
                    }),
            )
            // 未读角标
            .when(unread > 0, |el| {
                el.child(
                    h_flex()
                        .flex_none()
                        .h(px(18.))
                        .min_w(px(18.))
                        .px_1()
                        .justify_center()
                        .rounded_full()
                        .bg(cx.theme().danger)
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().danger_foreground)
                                .child(if unread > 99 {
                                    "99+".to_string()
                                } else {
                                    unread.to_string()
                                }),
                        ),
                )
            })
            .into_any_element()
    }

    /// 空态：还没发现任何设备
    fn sidebar_empty(&self, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .id("sidebar-empty")
            .w_full()
            .items_center()
            .gap_2()
            .px_3()
            .py_10()
            .child(
                Icon::new(IconName::Globe)
                    .with_size(px(26.))
                    .text_color(cx.theme().muted_foreground)
                    .opacity(0.6),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("等待设备上线"),
            )
            .child(
                div()
                    .w_full()
                    .text_xs()
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .opacity(0.75)
                    .child("需在同一局域网；也可手动添加 IP"),
            )
            .into_any_element()
    }
}
