//! 覆盖层：设置弹窗、添加设备弹窗、toast

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex, Icon, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::root::RootView;
use transfer_core::UiCommand;

impl RootView {
    pub fn render_overlays(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if self.show_settings {
            out.push(self.render_settings_modal(cx).into_any_element());
        }
        if self.show_connect {
            out.push(self.render_connect_modal(cx).into_any_element());
        }
        if self.show_web_qr {
            out.push(self.render_web_qr_modal(cx).into_any_element());
        }
        if self.overwrite_req.is_some() {
            out.push(self.render_overwrite_modal(cx).into_any_element());
        }
        if self.show_close_dialog {
            out.push(self.render_close_dialog(_window, cx).into_any_element());
        }
        if let Some((text, _, is_error)) = &self.toast {
            let (bg, fg, icon) = if *is_error {
                (self.danger(cx), self.danger_fg(cx), IconName::TriangleAlert)
            } else {
                (self.toast_bg(cx), self.toast_fg(cx), IconName::Check)
            };
            out.push(
                h_flex()
                    .id("toast")
                    .absolute()
                    .bottom_6()
                    .right_6()
                    .max_w(px(360.))
                    .px_3p5()
                    .py_2p5()
                    .gap_2()
                    .items_start()
                    .rounded_xl()
                    .shadow_lg()
                    .bg(bg)
                    .child(
                        Icon::new(icon)
                            .with_size(px(14.))
                            .flex_none()
                            .mt(px(2.))
                            .text_color(fg),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_sm()
                            .text_color(fg)
                            .child(text.clone()),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// 手动添加设备弹窗（多播/广播都不通时用 IP 直连）
    fn render_connect_modal(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("connect-overlay")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .bg(self.overlay(cx))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .id("connect-card")
                    .w(px(400.))
                    .p_4()
                    .gap_3()
                    .rounded_2xl()
                    .shadow_lg()
                    .border_1()
                    .border_color(self.border_color(cx))
                    .bg(self.card_bg(cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Globe)
                                    .text_size(px(18.))
                                    .text_color(self.primary(cx)),
                            )
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("添加设备"),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(self.fg_muted(cx))
                                    .child("输入对方设备的 IP 地址（默认端口 17878，可带 :端口）"),
                            )
                            .child(Input::new(&self.connect_input)),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("conn-cancel")
                                    .label("取消")
                                    .outline()
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        this.show_connect = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("conn-ok")
                                    .primary()
                                    .label("连接")
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        let host = this.connect_input.read(cx).value().to_string();
                                        if host.trim().is_empty() {
                                            this.toast("请输入 IP 地址", true);
                                            return;
                                        }
                                        let _ = this.core.send(UiCommand::ConnectPeer { host });
                                        this.show_connect = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 关闭确认弹窗：最小化到托盘 / 退出（勾选"记住"写进设置）
    fn render_close_dialog(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {

        v_flex()
            .id("close-overlay")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .bg(self.overlay(cx))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .id("close-card")
                    .w(px(360.))
                    .p_4()
                    .gap_3()
                    .rounded_2xl()
                    .shadow_lg()
                    .border_1()
                    .border_color(self.border_color(cx))
                    .bg(self.card_bg(cx))
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("关闭 LocalTransfer？"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.fg_muted(cx))
                            .child("最小化到托盘可保持后台接收与通知"),
                    )
                    // 自绘勾选框（组件库 Checkbox 在该 overlay 内点击无响应，
                    // 换最朴素可靠的实现）
                    .child(
                        h_flex()
                            .id("close-remember")
                            .gap_1p5()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                this.close_remember = !this.close_remember;
                                cx.notify();
                            }))
                            .child(
                                h_flex()
                                    .flex_none()
                                    .size(px(14.))
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(if self.close_remember {
                                        self.primary(cx)
                                    } else {
                                        self.border_color(cx)
                                    })
                                    .when(self.close_remember, |el| {
                                        el.bg(self.primary(cx)).child(
                                            Icon::new(IconName::Check)
                                                .with_size(px(10.))
                                                .text_color(gpui_kit::rgb(0xffffff)),
                                        )
                                    }),
                            )
                            .child(
                                div().text_sm().child("记住我的选择（不再提示）"),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("close-cancel")
                                    .label("取消")
                                    .outline()
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        this.show_close_dialog = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("close-to-tray")
                                    .label("最小化到托盘")
                                    .primary()
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        this.apply_close_choice("tray", cx);
                                    })),
                            )
                            .child(
                                Button::new("close-quit")
                                    .label("退出程序")
                                    .outline()
                                    .on_click(cx.listener(|this, _ev, window, cx| {
                                        this.apply_close_choice("close", cx);
                                        this.force_close = true;
                                        window.remove_window();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 覆盖确认弹窗：接收的文件与下载目录同名
    fn render_overwrite_modal(&self, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::component::ActiveTheme as _;
        let Some(req) = &self.overwrite_req else {
            return div().into_any_element();
        };
        let conflicts: Vec<String> = req
            .files
            .iter()
            .filter(|f| self.me.download_dir.join(&f.rel_path).exists())
            .map(|f| f.name.clone())
            .collect();
        let req = req.clone();

        v_flex()
            .id("overwrite-overlay")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .bg(self.overlay(cx))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .id("overwrite-card")
                    .w(px(400.))
                    .p_4()
                    .gap_3()
                    .rounded_2xl()
                    .shadow_lg()
                    .border_1()
                    .border_color(self.border_color(cx))
                    .bg(self.card_bg(cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .text_size(px(18.))
                                    .text_color(cx.theme().warning),
                            )
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{} 个同名文件已存在", conflicts.len())),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(self.fg_muted(cx))
                                    .child("下载目录里已有同名文件："),
                            )
                            .child(
                                v_flex().gap_0p5().children(
                                    conflicts.iter().take(5).map(|n| {
                                        div()
                                            .text_xs()
                                            .truncate()
                                            .child(format!("· {n}"))
                                    }),
                                ),
                            )
                            .when(conflicts.len() > 5, |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(self.fg_muted(cx))
                                        .child(format!("… 等共 {} 个", conflicts.len())),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("ow-cancel")
                                    .label("取消")
                                    .outline()
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        // 只关弹窗回请求卡片（用户还能拒绝或再接收）
                                        this.overwrite_req = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("ow-overwrite")
                                    .label("覆盖接收")
                                    .primary()
                                    .on_click(cx.listener(
                                        move |this, _ev, _window, cx| {
                                            let req = req.clone();
                                            this.accept_request(req, true, cx);
                                        },
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 设置弹窗：设备名 / 下载目录 / 端口（端口重启生效）
    fn render_settings_modal(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("settings-overlay")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .bg(self.overlay(cx))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .id("settings-card")
                    .w(px(420.))
                    .p_4()
                    .gap_3()
                    .rounded_2xl()
                    .shadow_lg()
                    .border_1()
                    .border_color(self.border_color(cx))
                    .bg(self.card_bg(cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Icon::new(IconName::Settings).text_size(px(18.)))
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("设置"),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_xs().opacity(0.7).child("设备名"))
                            .child(Input::new(&self.set_name)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_xs().opacity(0.7).child("接收保存目录"))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Input::new(&self.set_dir).flex_1())
                                    .child(
                                        Button::new("set-dir-pick")
                                            .label("浏览…")
                                            .outline()
                                            .small()
                                            .on_click(cx.listener(|this, _ev, window, cx| {
                                                this.pick_download_dir(window, cx);
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_xs().opacity(0.7).child("HTTP 端口（重启后生效）"))
                            .child(Input::new(&self.set_port)),
                    )
                    // 关闭按钮行为：三选（同关闭弹窗的"记住选择"）
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .opacity(0.7)
                                    .child("关闭按钮行为"),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("close-act-ask")
                                            .label("每次询问")
                                            .when(self.me.close_action == "ask", |b| b.primary())
                                            .when(self.me.close_action != "ask", |b| b.outline())
                                            .small()
                                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                                this.me.close_action = "ask".into();
                                                let _ = this.me.save();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("close-act-tray")
                                            .label("最小化到托盘")
                                            .when(self.me.close_action == "tray", |b| b.primary())
                                            .when(self.me.close_action != "tray", |b| b.outline())
                                            .small()
                                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                                this.me.close_action = "tray".into();
                                                let _ = this.me.save();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("close-act-close")
                                            .label("直接退出")
                                            .when(self.me.close_action == "close", |b| b.primary())
                                            .when(self.me.close_action != "close", |b| b.outline())
                                            .small()
                                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                                this.me.close_action = "close".into();
                                                let _ = this.me.save();
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    )
                    // 自动接收：开关即时生效并写盘（独立于下面的"保存"按钮）
                    .child(
                        h_flex()
                            .id("set-auto-recv")
                            .w_full()
                            .gap_2()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(div().text_sm().child("自动接收文件"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(self.fg_muted(cx))
                                            .child("跳过确认，直接保存到下载目录"),
                                    ),
                            )
                            .child(
                                Switch::new("sw-auto-recv")
                                    .checked(self.me.auto_receive)
                                    .on_click(cx.listener(
                                        |this, checked: &bool, _window, cx| {
                                            this.me.auto_receive = *checked;
                                            if let Err(e) = this.me.save() {
                                                this.toast(format!("保存失败: {e}"), true);
                                            }
                                            cx.notify();
                                        },
                                    )),
                            ),
                    )
                    // 开机启动：写 HKCU Run 键（注册当前 exe 路径，即时生效）
                    .child(
                        h_flex()
                            .id("set-autostart")
                            .w_full()
                            .gap_2()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(div().text_sm().child("开机启动"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(self.fg_muted(cx))
                                            .child("登录 Windows 后自动运行"),
                                    ),
                            )
                            .child(
                                Switch::new("sw-autostart")
                                    .checked(self.autostart)
                                    .on_click(cx.listener(
                                        |this, checked: &bool, _window, cx| {
                                            match crate::root::set_autostart(*checked) {
                                                Ok(()) => {
                                                    this.autostart = *checked;
                                                    this.toast(
                                                        if *checked { "已开启开机启动" }
                                                        else { "已关闭开机启动" },
                                                        false,
                                                    );
                                                }
                                                Err(e) => {
                                                    this.toast(format!("设置失败: {e:#}"), true)
                                                }
                                            }
                                            cx.notify();
                                        },
                                    )),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("set-cancel")
                                    .label("关闭")
                                    .outline()
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        this.show_settings = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("set-save")
                                    .primary()
                                    .label("保存")
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        this.save_settings(cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn pick_download_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let picked = match cx.update(|_, cx| {
                cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: None,
                })
            }) {
                Ok(rx) => rx.await.ok().and_then(|r| r.ok()).flatten(),
                Err(_) => None,
            };
            if let Some(dir) = picked
                .as_ref()
                .and_then(|v| v.first())
                .map(|p| p.to_string_lossy().to_string())
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.set_dir
                        .update(cx, |state, cx| state.set_value(dir, window, cx));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 本机二维码弹窗（手机扫码添加本机 / 打开网页客户端）
    fn render_web_qr_modal(&self, cx: &mut Context<Self>) -> AnyElement {
        let url = format!(
            "http://{}:{}/",
            self.local_ip.clone().unwrap_or_else(|| "127.0.0.1".into()),
            self.me.http_port
        );

        // 离线生成二维码：纯 Rust qrcode crate，div 网格渲染（无需图片资源）
        let code = qrcode::QrCode::new(url.as_bytes()).ok();
        let qr = code.map(|c| {
            let n = c.width();
            let cell = 5.0f32;
            let colors = c.to_colors();
            v_flex()
                .id("qr-grid")
                .p_2()
                .bg(rgb(0xffffff))
                .rounded_lg()
                .children((0..n).map(|y| {
                    h_flex()
                        .h(px(cell))
                        .children((0..n).map(|x| {
                            let dark = colors[y * n + x] == qrcode::Color::Dark;
                            div()
                                .w(px(cell))
                                .h(px(cell))
                                .when(dark, |d| d.bg(rgb(0x111111)))
                        }))
                }))
        });

        // 样式闭包要 'static，先取色值
        use gpui_kit::component::ActiveTheme as _;
        let primary = self.primary(cx);
        let pill_bg = cx.theme().list_hover;
        let hover_bg2 = cx.theme().list_hover;
        let fg_muted = self.fg_muted(cx);
        let url_for_click = url.clone();

        v_flex()
            .id("web-qr-overlay")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .bg(self.overlay(cx))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _ev, _window, cx| {
                    this.show_web_qr = false;
                    cx.notify();
                }),
            )
            .child(
                v_flex()
                    .id("web-qr-card")
                    .w(px(340.))
                    .p_5()
                    .gap_3()
                    .items_center()
                    .relative()
                    .rounded_2xl()
                    .shadow_lg()
                    .border_1()
                    .border_color(self.border_color(cx))
                    .bg(self.card_bg(cx))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|_, _ev: &gpui::MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    // 右上角关闭
                    .child(
                        div()
                            .id("qr-close-btn")
                            .absolute()
                            .top_2()
                            .right_2()
                            .flex_none()
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover_bg2))
                            .child(
                                Icon::new(IconName::Close)
                                    .with_size(px(13.))
                                    .text_color(fg_muted),
                            )
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                this.show_web_qr = false;
                                cx.notify();
                            })),
                    )
                    // 标题 + 副标题
                    .child(
                        v_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("本机二维码"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(self.fg_muted(cx))
                                    .child("手机扫码添加本机或打开网页客户端"),
                            ),
                    )
                    .children(qr)
                    // 地址胶囊：点击复制（明确提示）
                    .child(
                        h_flex()
                            .id("qr-url")
                            .mt_1()
                            .w_full()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .items_center()
                            .rounded_lg()
                            .bg(pill_bg)
                            .border_1()
                            .border_color(self.border_color(cx))
                            .cursor_pointer()
                            .hover(move |s| s.border_color(primary))
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    url_for_click.clone(),
                                ));
                                this.toast("已复制地址", false);
                            }))
                            .child(
                                Icon::new(IconName::Copy)
                                    .with_size(px(13.))
                                    .flex_none()
                                    .text_color(self.primary(cx)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .child(url.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(self.primary(cx))
                                    .child("点击复制"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.fg_muted(cx))
                            .opacity(0.85)
                            .child("同一局域网内使用 · 点击上方地址可复制"),
                    ),
            )
            .into_any_element()
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        let name = self.set_name.read(cx).value().to_string();
        let dir = self.set_dir.read(cx).value().to_string();
        let port = self.set_port.read(cx).value().to_string();

        let mut cfg = self.me.clone();
        let name = name.trim().to_string();
        if !name.is_empty() {
            cfg.device_name = name;
        }
        let dir = dir.trim().to_string();
        if !dir.is_empty() {
            cfg.download_dir = std::path::PathBuf::from(&dir);
        }
        if let Ok(p) = port.trim().parse::<u16>() {
            if p != self.me.http_port {
                cfg.http_port = p;
            }
        }

        match cfg.save() {
            Ok(()) => {
                let port_changed = cfg.http_port != self.me.http_port;
                // 改名要通知核心更新广播身份，否则对端看到的还是旧名
                let name_changed = cfg.device_name != self.me.device_name;
                if name_changed {
                    let _ = self.core.send(UiCommand::Rename {
                        name: cfg.device_name.clone(),
                    });
                }
                self.me = cfg;
                self.show_settings = false;
                if port_changed {
                    self.toast("已保存；端口修改将在重启后生效", false);
                } else {
                    self.toast("已保存", false);
                }
            }
            Err(e) => self.toast(format!("保存失败: {e}"), true),
        }
        cx.notify();
    }

}
