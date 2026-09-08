//! 主聊天面板：会话头部 + 消息流（气泡 / 文件卡片）+ 输入区
//!
//! 消息行用组件库的 `Message` + `Bubble`：它们内部已经处理好
//! `flex_none` + `max_w(relative(0.8))` + `min_w_0` 的组合，
//! 比手搓 `h_flex().justify_end()` + `max_w(px)` 稳（后者会被 flex 收缩
//! 按 min-content 测量，中文气泡变成一字一行）。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::component::bubble::{Bubble, BubbleVariant};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::message::{Message, MessageAlignment, MessageContent, MessageFooter};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::message_scroller::MessageScroller;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::root::{fmt_size, fmt_speed, transfer_open_target, initial_avatar, RootView};
use transfer_core::{MessageKind, UiCommand};

/// 固定宽度让卡片整齐（也避开 shrink-to-fit）
const CARD_W: f32 = 296.;

/// 把连续的、属于同一次传输（同 transfer_id、同方向）的文件消息合并成一组，
/// 返回 `[start, end)` 区间。单条消息自成一组。
pub(crate) fn group_transfers(
    messages: &[transfer_core::ChatMessage],
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let mut j = i + 1;
        if let Some(tid) = transfer_of(&messages[i]) {
            while j < messages.len()
                && messages[j].outgoing == messages[i].outgoing
                && transfer_of(&messages[j]) == Some(tid)
            {
                j += 1;
            }
        }
        out.push((i, j));
        i = j;
    }
    out
}

/// 分组后的行数（供虚拟列表对齐行数用）
pub(crate) fn group_count(messages: &[transfer_core::ChatMessage]) -> usize {
    group_transfers(messages).len()
}

fn transfer_of(msg: &transfer_core::ChatMessage) -> Option<&str> {
    match &msg.kind {
        MessageKind::File { transfer_id, .. } => transfer_id.as_deref(),
        _ => None,
    }
}

fn file_size_of(msg: &transfer_core::ChatMessage) -> u64 {
    match &msg.kind {
        MessageKind::File { size, .. } => *size,
        _ => 0,
    }
}

fn saved_path_of(msg: &transfer_core::ChatMessage) -> Option<PathBuf> {
    match &msg.kind {
        MessageKind::File { saved_path, .. } => saved_path.clone(),
        _ => None,
    }
}

impl RootView {
    pub fn render_chat(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(peer) = self.selected.clone() else {
            return self.no_peer_placeholder(cx);
        };

        // 网页会话：名称与副标题特判（右侧显示本机网页地址）
        let is_web = peer == transfer_core::proto::WEB_PEER_ID;
        let peer_plat = self
            .devices
            .iter()
            .find(|d| d.info.id == peer)
            .map(|d| d.info.plat.clone())
            .unwrap_or_else(|| "web".into());
        let name = if is_web {
            "HTTP Server".to_string()
        } else {
            self.peer_name(&peer)
        };
        let dev = self.devices.iter().find(|d| d.info.id == peer);
        let online = if is_web {
            true // 网页会话永远"在线"（浏览器随开随用）
        } else {
            dev.map(|d| d.online).unwrap_or(false)
        };
        let addr = if is_web {
            Some(format!(
                "http://{}:{}/",
                self.local_ip.clone().unwrap_or_else(|| "127.0.0.1".into()),
                self.me.http_port
            ))
        } else {
            dev.filter(|d| d.online)
                .map(|d| format!("{}:{}", d.addr, d.info.port))
        };

        // 与当前会话相关的进行中传输（按 peer_id 精确匹配，收发两个方向都算）
        let active: Vec<(String, f64, f64)> = self
            .transfers
            .iter()
            .filter(|t| t.error.is_none() && !t.all_done())
            .filter(|t| t.peer_id == peer)
            .map(|t| {
                let total = t.total_bytes();
                let done = t.transferred_bytes();
                let frac = if total > 0 {
                    done as f64 / total as f64
                } else {
                    1.0
                };
                (t.id.clone(), frac, t.speed_bps)
            })
            .collect();
        let active_count = active.len();
        let active_frac = if active.is_empty() {
            0.0
        } else {
            active.iter().map(|(_, f, _)| *f).sum::<f64>() / active.len() as f64
        };
        let active_speed = active.iter().map(|(_, _, s)| *s).sum::<f64>();
        let cancel_ids: Vec<String> = active.iter().map(|(id, _, _)| id.clone()).collect();
        let row_count = self.chat_row_count(&peer);

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.chat_header(
                is_web,
                &name,
                &peer_plat,
                online,
                addr,
                active_count,
                active_frac,
                active_speed,
                cancel_ids,
                cx,
            ))
            // 消息流：虚拟列表 + 尾部跟随（新消息自动滚到底，滚上去看历史时
            // 会出现"回到底部"按钮）。行渲染走 entity.update 拿到 &mut Context。
            .child(
                if row_count == 0 {
                    self.chat_empty(cx).into_any_element()
                } else {
                    let entity = cx.entity();
                    MessageScroller::new("chat-scroll", self.scroller.clone(), move |ix, _w, cx| {
                        entity.update(cx, |this, cx| this.render_chat_row(&peer, ix, cx))
                    })
                    .into_any_element()
                },
            )
            .child(self.input_bar(cx))
            .into_any_element()
    }

    /// 没有选中设备时的占位
    fn no_peer_placeholder(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Icon::new(IconName::Network)
                    .with_size(px(30.))
                    .text_color(cx.theme().muted_foreground)
                    .opacity(0.5),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("在左侧选择一台设备开始传输"),
            )
            .into_any_element()
    }



    fn chat_header(
        &self,
        is_web: bool,
        name: &str,
        plat: &str,
        online: bool,
        addr: Option<String>,
        _active_count: usize,
        _active_frac: f64,
        _active_speed: f64,
        cancel_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // 右侧文字：在线显示地址，离线显示"离线"（仅普通设备会话）
        let tail = match (&addr, online) {
            (Some(a), true) => a.clone(),
            (None, true) => "在线".to_string(),
            (_, false) => "离线".to_string(),
        };

        h_flex()
            .id("chat-header")
            .flex_none()
            .w_full()
            .h(px(52.))
            .px_4()
            .gap_2p5()
            .border_b_1()
            .border_color(cx.theme().border)
            // 头像：网页会话 = 三维地球；设备 = 名字首字
            .child(if is_web {
                // 地球头像（与侧栏网页行同款主色调）
                div()
                    .flex_none()
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(cx.theme().primary.opacity(0.18))
                    .child(
                        Icon::new(IconName::Globe)
                            .with_size(px(15.))
                            .text_color(cx.theme().primary),
                    )
                    .into_any_element()
            } else {
                initial_avatar(name, 28., cx).into_any_element()
            })
            // 设备名（flex_1 吃掉中间空间，名字不被挤）
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(px(240.))
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name.to_string()),
            )
            // 在线状态点 + 设备类型组：网页会话不显示（无"在线/平台"概念）
            .when(!is_web, |el| {
                el.child(
                    div()
                        .flex_none()
                        .size(px(6.))
                        .rounded_full()
                        .when(online, |el| el.bg(cx.theme().success))
                        .when(!online, |el| {
                            el.bg(cx.theme().muted_foreground).opacity(0.4)
                        }),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .child(crate::sidebar::plat_svg(
                            plat,
                            11.,
                            if online {
                                cx.theme().primary
                            } else {
                                cx.theme().muted_foreground
                            },
                        ))
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(crate::sidebar::plat_label(plat)),
                        ),
                )
            })
            // 弹性空隙：右侧内容顶到最右（传输进度只在消息流里，不占标题栏）
            .child(div().flex_1())
            // 网页会话右侧：地址图标（打开地址/二维码弹窗，弹窗里地址可点击复制）
            .when(is_web, |el| {
                el.child(
                    div()
                        .id("hdr-web-addr")
                        .flex_none()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_color(cx.theme().muted_foreground)
                        .hover(|s| s.text_color(cx.theme().primary))
                        .child(Icon::new(IconName::Network).with_size(px(15.)))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.show_web_qr = true;
                            cx.notify();
                        })),
                )
            })
            // 普通设备：地址文字（最右，点击复制）
            .when(!is_web, |el| {
                el.child(
                    div()
                        .id("hdr-addr")
                        .flex_none()
                        .text_xs()
                        .cursor_pointer()
                        .text_color(cx.theme().muted_foreground)
                        .hover(|s| s.text_color(cx.theme().primary))
                        .child(tail.clone())
                        .on_click({
                            let addr = tail.clone();
                            cx.listener(move |this, _ev, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(addr.clone()));
                                this.toast("已复制地址", false);
                            })
                        }),
                )
            })
            .into_any_element()
    }

    fn chat_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .id("chat-empty")
            .w_full()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Icon::new(IconName::Inbox)
                    .with_size(px(32.))
                    .text_color(cx.theme().muted_foreground)
                    .opacity(0.5),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("还没有消息"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .opacity(0.75)
                    .child("发送文本，或拖拽文件到窗口"),
            )
            .into_any_element()
    }

    fn input_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let peer_for_clear = self.selected.clone().unwrap_or_default();
        // 3 秒内点两次才清空（防误触）
        let armed = self
            .confirm_clear
            .as_ref()
            .is_some_and(|(p, at)| p == &peer_for_clear && at.elapsed() < Duration::from_secs(3));

        h_flex()
            .id("input-bar")
            .flex_none()
            .w_full()
            .px_4()
            .py_3()
            .gap_2()
            .items_end()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("btn-pick-file")
                    .ghost()
                    .small()
                    .icon(IconName::File)
                    .tooltip("发送文件")
                    .on_click(cx.listener(|this, _ev, window, cx| {
                        this.pick_and_send(window, cx, true);
                    })),
            )
            .child(
                Button::new("btn-pick-folder")
                    .ghost()
                    .small()
                    .icon(IconName::Folder)
                    .tooltip("发送文件夹")
                    .on_click(cx.listener(|this, _ev, window, cx| {
                        this.pick_and_send(window, cx, false);
                    })),
            )
            .child(
                Button::new("btn-paste")
                    .ghost()
                    .small()
                    .icon(IconName::Copy)
                    .tooltip("粘贴剪贴板并发送")
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.paste_and_send(cx);
                    })),
            )
            // 发送到网页接收页：浏览器打开 http://本机IP:端口/ 下载
            .child(
                Button::new("btn-web-offer")
                    .ghost()
                    .small()
                    .icon(IconName::Globe)
                    .tooltip("发送到网页（浏览器打开本机地址下载）")
                    .on_click(cx.listener(|this, _ev, window, cx| {
                        this.web_offer_pick(window, cx);
                    })),
            )
            // 清空聊天记录（两段确认），紧跟粘贴按钮
            .child(
                Button::new("btn-clear-chat")
                    .ghost()
                    .small()
                    .icon(IconName::Delete)
                    .tooltip(if armed { "再点一次确认清空" } else { "清空聊天记录" })
                    .when(armed, |b| b.danger())
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        let peer = match this.selected.clone() {
                            Some(p) => p,
                            None => return,
                        };
                        // 过了 3 秒或换了会话 → 重新武装
                        let still = this
                            .confirm_clear
                            .as_ref()
                            .is_some_and(|(p, at)| {
                                p == &peer && at.elapsed() < Duration::from_secs(3)
                            });
                        if still {
                            this.confirm_clear = None;
                            this.chats.remove(&peer);
                            this.unread.insert(peer.clone(), 0);
                            if let Ok(s) = this.store.lock() {
                                let _ = s.clear_history(&peer);
                            }
                            this.scroller.update(cx, |s, cx| s.reset(0, cx));
                            this.toast("已清空聊天记录", false);
                        } else {
                            this.confirm_clear = Some((peer.clone(), Instant::now()));
                            this.toast("再点一次清空（3 秒内）", false);
                        }
                        cx.notify();
                    })),
            )
            .child(div().flex_1().min_w_0().child(Textarea::new(&self.input)))
            .child(
                Button::new("btn-send")
                    .primary()
                    .small()
                    .label("发送")
                    .on_click(cx.listener(|this, _ev, window, cx| {
                        this.send_current_input(window, cx);
                    })),
            )
            .into_any_element()
    }

    /// 虚拟列表的一行：分组消息或（最后一行）接收请求卡片。
    /// 只克隆本行涉及的消息，不搬整个会话。
    fn render_chat_row(&mut self, peer: &str, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let group = self.chats.get(peer).map(|msgs| {
            let groups = group_transfers(msgs);
            groups.get(ix).map(|(start, end)| msgs[*start..*end].to_vec())
        });
        match group {
            Some(Some(rows)) if rows.len() == 1 => self.render_message(&rows[0], cx),
            Some(Some(rows)) => self.render_batch(&rows, cx),
            Some(None) => self.render_request_card(cx),
            None => div().into_any_element(),
        }
    }

    /// 接收请求卡片：替代原先的全屏弹窗，直接出现在会话底部
    fn render_request_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(req) = self.incoming.clone() else {
            return div().into_any_element();
        };
        let total: u64 = req.files.iter().map(|f| f.size).sum();
        let peer_name = req.peer.info.name.clone();

        Message::new()
            .alignment(MessageAlignment::Start)
            .content(MessageContent::new().bubble(
                Bubble::new()
                    .alignment(MessageAlignment::Start)
                    .with_variant(BubbleVariant::Outline)
                    .child(
                        v_flex()
                            .id("req-card")
                            .w(px(CARD_W))
                            .flex_none()
                            .gap_2p5()
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .gap_2p5()
                                    .child(
                                        h_flex()
                                            .flex_none()
                                            .size(px(36.))
                                            .items_center()
                                            .justify_center()
                                            .rounded_lg()
                                            .bg(cx.theme().muted)
                                            .child(
                                                Icon::new(IconName::Inbox)
                                                    .with_size(px(18.))
                                                    .text_color(cx.theme().primary),
                                            ),
                                    )
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
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(format!(
                                                        "{} 想发送 {} 个文件",
                                                        peer_name,
                                                        req.files.len()
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .w_full()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(format!(
                                                        "{} · 保存到 {}",
                                                        fmt_size(total),
                                                        self.me.download_dir.display()
                                                    )),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .child(
                                        Button::new("req-reject")
                                            .label("拒绝")
                                            .outline()
                                            .small()
                                            .flex_1()
                                            .on_click(cx.listener(
                                                move |this, _ev, _window, cx| {
                                                    if let Some(r) = this.incoming.take() {
                                                        let _ = this.core.send(
                                                            UiCommand::RespondRequest {
                                                                req_id: r.req_id,
                                                                accept: false,
                                                                save_dir: None,
                                                                overwrite: false,
                                                            },
                                                        );
                                                    }
                                                    this.sync_scroller(cx);
                                                    cx.notify();
                                                },
                                            )),
                                    )
                                    .child(
                                        Button::new("req-accept")
                                            .label("接收")
                                            .primary()
                                            .flex_1()
                                            .small()
                                            .flex_1()
                                            .on_click(cx.listener(
                                                move |this, _ev, _window, cx| {
                                                    // 同名文件预检：有冲突先弹覆盖确认
                                                    this.ask_overwrite(cx);
                                                },
                                            )),
                                    ),
                            ),
                    ),
            ))
            .into_any_element()
    }

    fn render_message(
        &self,
        msg: &transfer_core::ChatMessage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mine = msg.outgoing;
        let align = if mine {
            MessageAlignment::End
        } else {
            MessageAlignment::Start
        };
        let time = crate::root::fmt_time(msg.created_at);

        let bubble = match &msg.kind {
            MessageKind::Text(text) => {
                let text_for_copy = text.clone();
                // 右键菜单：复制 / 删除（收发两侧都有）
                let text_for_menu = text.clone();
                let peer_id = msg.peer_id.clone();
                let msg_id = msg.id;
                let handle = cx.entity();
                Bubble::new()
                    .alignment(align)
                    .with_variant(if mine {
                        BubbleVariant::Filled
                    } else {
                        BubbleVariant::Secondary
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("msg-{}", msg.id)))
                            .min_w_0()
                            // 双击复制（收发两侧都是；单击不响应）
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(
                                    move |this, ev: &MouseDownEvent, _window, cx| {
                                        if ev.click_count >= 2 {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                text_for_copy.clone(),
                                            ));
                                            this.toast("已复制到剪贴板", false);
                                        }
                                    },
                                ),
                            )
                            .context_menu(move |menu, _window, _cx| {
                                let (h, t, p) =
                                    (handle.clone(), text_for_menu.clone(), peer_id.clone());
                                menu.item(
                                    PopupMenuItem::new("复制").on_click(
                                        move |_ev, _window, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                t.clone(),
                                            ));
                                        },
                                    ),
                                )
                                .item(PopupMenuItem::new("删除").on_click(
                                    move |_ev, _window, cx| {
                                        h.update(cx, |this, cx| {
                                            this.delete_messages(&p, &[msg_id], cx)
                                        });
                                    },
                                ))
                            })
                            // gpui 文本元素把 \n 按 CSS 语义折叠成空格，
                            // 换行要按行拆开渲染
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .children(
                                        text.lines().map(|l| {
                                            div()
                                                .min_w_0()
                                                .line_height(px(20.))
                                                .child(l.to_string())
                                        }),
                                    ),
                            ),
                    )
            }
            MessageKind::File {
                name,
                size,
                saved_path,
                transfer_id,
                file_id,
            } => {
                // 右键菜单：删除（文件消息不提供复制）
                let peer_id = msg.peer_id.clone();
                let msg_id = msg.id;
                let handle = cx.entity();
                let card = self.file_card(
                    name,
                    *size,
                    saved_path.clone(),
                    transfer_id.as_deref(),
                    file_id.as_deref(),
                    mine,
                    cx,
                );
                Bubble::new()
                    .alignment(align)
                    .with_variant(BubbleVariant::Outline)
                    .child(
                        div()
                            .min_w_0()
                            .context_menu(move |menu, _window, _cx| {
                                let (h, p) = (handle.clone(), peer_id.clone());
                                menu.item(PopupMenuItem::new("删除").on_click(
                                    move |_ev, _window, cx| {
                                        h.update(cx, |this, cx| {
                                            this.delete_messages(&p, &[msg_id], cx)
                                        });
                                    },
                                ))
                            })
                            .child(card),
                    )
            }
        };

        Message::new()
            .alignment(align)
            .content(MessageContent::new().bubble(bubble))
            .footer(
                MessageFooter::new().child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .opacity(0.8)
                        .child(time),
                ),
            )
            .into_any_element()
    }

    /// 一次传输里的多个文件合并成一张卡片（发文件夹时会产生几十上百条
    /// 文件消息，逐条画卡片会把聊天刷爆），显示聚合进度。
    fn render_batch(
        &self,
        msgs: &[transfer_core::ChatMessage],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let first = &msgs[0];
        let mine = first.outgoing;
        let align = if mine {
            MessageAlignment::End
        } else {
            MessageAlignment::Start
        };
        let time = crate::root::fmt_time(first.created_at);
        let tid = transfer_of(first).unwrap_or_default().to_string();
        let count = msgs.len();
        let total: u64 = msgs.iter().map(file_size_of).sum();

        // 聚合状态取自活动传输；传输记录已被回收时退回消息里的落盘路径
        let st = self.transfers.iter().find(|t| t.id == tid);
        let sent: u64 = st.map(|t| t.transferred_bytes()).unwrap_or(0);
        let files_done = st
            .map(|t| t.files.iter().filter(|f| f.done).count())
            .unwrap_or(count);
        // 传输记录不在（重启/回收后的历史卡片）→ 视为已完成，按方向定论，
        // 不能停在"进行中/等待对方接收"
        let all_done = st.map(|t| t.all_done()).unwrap_or(true);
        let err = st.and_then(|t| t.error.clone());
        let speed = st.map(|t| t.speed_bps).unwrap_or(0.0);
        // 标题：单一文件夹 → 文件夹名；混合/散文件 → "N 个文件"。
        // 历史态（无 TransferState，如网页上传）从落盘路径公共祖先推断
        let hist: Vec<PathBuf> = msgs.iter().filter_map(saved_path_of).collect();
        let title = st
            .map(|t| crate::root::transfer_display_name(&t.files))
            .unwrap_or_else(|| {
                match transfer_core::client::common_ancestor_dir(&hist)
                    .as_ref()
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                {
                    // 公共祖先就是"网页上传"根 → 散文件；否则 = 文件夹名
                    Some(name) if name != transfer_core::web::WEB_UPLOAD_DIR => name,
                    _ => format!("{count} 个文件"),
                }
            });
        // 打开目标（reveal_path：打开上一级并选中）：活动传输走 rel_path
        // 深度回溯（收方落盘/发方源路径）；历史卡片（重启后）没有 rel_path，
        // 用整组落盘路径的公共祖先反推文件夹根
        let live = st.and_then(|t| {
            t.files
                .iter()
                .find_map(|f| f.saved_path.clone().or_else(|| f.source.clone()))
        });
        let fallback = live
            .or_else(|| transfer_core::client::common_ancestor_dir(&hist))
            .or_else(|| hist.first().cloned());
        let open = transfer_open_target(&self.transfers, Some(&tid), None).or(fallback);
        let frac = if total > 0 {
            (sent as f64 / total as f64).clamp(0.0, 1.0)
        } else {
            1.0
        };

        let card = v_flex()
            .id(SharedString::from(format!("batch-{tid}")))
            .w(px(CARD_W))
            .flex_none()
            .gap_2p5()
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2p5()
                    .child(
                        h_flex()
                            .flex_none()
                            .size(px(36.))
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .bg(cx.theme().muted)
                            .child(
                                Icon::new(IconName::Folder)
                                    .with_size(px(18.))
                                    .text_color(cx.theme().primary),
                            ),
                    )
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
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{} · {files_done}/{count} 已完成", fmt_size(total))),
                            ),
                    ),
            );

        let card = match &err {
            // 失败/取消
            Some(msg) => card.child(
                h_flex()
                    .w_full()
                    .gap_1p5()
                    .child(
                        Icon::new(IconName::CircleX)
                            .with_size(px(13.))
                            .flex_none()
                            .text_color(cx.theme().danger),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .w_full()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(msg.clone()),
                    ),
            ),
            // 进行中
            None if !all_done => card
                .child(
                    Progress::new(SharedString::from(format!("bprog-{tid}")))
                        .value((frac * 100.0) as f32)
                        .xsmall(),
                )
                .child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if sent == 0 {
                            "等待对方接收".to_string()
                        } else {
                            let speed = fmt_speed(speed);
                            let tail = if speed.is_empty() {
                                String::new()
                            } else {
                                format!(" · {speed}")
                            };
                            format!(
                                "{:.0}% · {} / {}{tail}",
                                frac * 100.0,
                                fmt_size(sent),
                                fmt_size(total)
                            )
                        }),
                ),
            // 已完成
            None => card.child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1p5()
                            .child(
                                Icon::new(IconName::Check)
                                    .with_size(px(13.))
                                    .flex_none()
                                    .text_color(cx.theme().success),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if mine {
                                        "已发送"
                                    } else if st.is_some() {
                                        "已保存"
                                    } else {
                                        "已接收"
                                    }),
                            ),
                    )
                    // 收发双方都能打开：资源管理器定位到上一级并选中
                    .when_some(open, |el, p| {
                        el.child(
                            Button::new(SharedString::from(format!("bopen-{tid}")))
                                .label("打开")
                                .outline()
                                .xsmall()
                                .on_click(move |_, _, cx| {
                                    let _ = cx.reveal_path(&p);
                                }),
                        )
                    }),
            ),
        };

        // 右键菜单：删除整批（一次传输的所有文件消息）
        let ids: Vec<i64> = msgs.iter().map(|m| m.id).collect();
        let peer_id = first.peer_id.clone();
        let handle = cx.entity();

        Message::new()
            .alignment(align)
            .content(
                MessageContent::new().bubble(
                    Bubble::new()
                        .alignment(align)
                        .with_variant(BubbleVariant::Outline)
                        .child(
                            div()
                                .min_w_0()
                                .context_menu(move |menu, _window, _cx| {
                                    let (h, p, ids) =
                                        (handle.clone(), peer_id.clone(), ids.clone());
                                    menu.item(PopupMenuItem::new("删除").on_click(
                                        move |_ev, _window, cx| {
                                            h.update(cx, |this, cx| {
                                                this.delete_messages(&p, &ids, cx)
                                            });
                                        },
                                    ))
                                })
                                .child(card),
                        )
                ),
            )
            .footer(
                MessageFooter::new().child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .opacity(0.8)
                        .child(time),
                ),
            )
            .into_any_element()
    }

    /// 文件卡片：图标 + 名称 + 大小 +（进度 / 打开）
    #[allow(clippy::too_many_arguments)]
    fn file_card(
        &self,
        name: &str,
        size: u64,
        saved_path: Option<PathBuf>,
        transfer_id: Option<&str>,
        file_id: Option<&str>,
        mine: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // 实时进度（活动传输里找）
        let mut progress: Option<(u64, u64, bool)> = None;
        // 落盘路径：接收完成时由 FileProgress 带回，聊天消息里那份仍是 None，
        // 所以优先取传输记录里的，否则"打开"按钮永远不出现
        let mut saved_path = saved_path;
        let mut err: Option<String> = None;
        let mut speed = 0.0f64;
        if let (Some(tid), Some(fid)) = (transfer_id, file_id) {
            if let Some(t) = self.transfers.iter().find(|t| t.id == tid) {
                err = t.error.clone();
                speed = t.speed_bps;
                if let Some(f) = t.files.iter().find(|f| f.meta.id == fid) {
                    progress = Some((f.transferred, f.meta.size, f.done));
                    if let Some(p) = &f.saved_path {
                        saved_path = Some(p.clone());
                    }
                }
            }
        }
        let done = progress.map(|p| p.2).unwrap_or(saved_path.is_some());

        // 元素 id 必须全列表唯一：同一文件多次传输会出多张同名卡片，
        // 按文件名拼 id 会冲突（同名 id 的交互元素只有第一个能点）——
        // 用 传输id+文件id 拼，缺省回退文件名
        let key = match (transfer_id, file_id) {
            (Some(t), Some(f)) => format!("{t}-{f}"),
            _ => name.to_string(),
        };

        v_flex()
            .id(SharedString::from(format!("card-{key}")))
            .w(px(CARD_W))
            .flex_none()
            .gap_2p5()
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2p5()
                    // 图标块
                    .child(
                        h_flex()
                            .flex_none()
                            .size(px(36.))
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .bg(cx.theme().muted)
                            .child(
                                Icon::new(IconName::FileText)
                                    .with_size(px(18.))
                                    .text_color(cx.theme().primary),
                            ),
                    )
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
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(name.to_string()),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(fmt_size(size)),
                            ),
                    ),
            )
            .child(match (&err, progress) {
                // 失败/取消
                (Some(msg), _) => h_flex()
                    .w_full()
                    .gap_1p5()
                    .child(
                        Icon::new(IconName::CircleX)
                            .with_size(px(13.))
                            .flex_none()
                            .text_color(cx.theme().danger),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .w_full()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(msg.clone()),
                    )
                    .into_any_element(),
                // 传输中：进度条 + 百分比 · 速度
                (None, Some((done_bytes, total, _))) if !done => {
                    let frac = if total > 0 {
                        done_bytes as f64 / total as f64
                    } else {
                        1.0
                    };
                    let speed = fmt_speed(speed);
                    let tail = if speed.is_empty() {
                        String::new()
                    } else {
                        format!(" · {speed}")
                    };
                    v_flex()
                        .w_full()
                        .gap_1p5()
                        .child(
                            Progress::new(SharedString::from(format!("prog-{key}")))
                                .value((frac * 100.0) as f32)
                                .xsmall(),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if done_bytes == 0 {
                                    "等待对方接收".to_string()
                                } else {
                                    format!(
                                        "{:.0}% · {} / {}{tail}",
                                        frac * 100.0,
                                        fmt_size(done_bytes),
                                        fmt_size(total)
                                    )
                                }),
                        )
                        .into_any_element()
                }
                // 已完成（有路径：接收方=落盘，发送方=源文件）
                _ => match &saved_path {
                    Some(p) => {
                        // 文件夹里的单个文件 → 定位到所在文件夹根并选中；散文件 → 选中该文件
                        let target = transfer_open_target(&self.transfers, transfer_id, file_id)
                            .unwrap_or_else(|| p.clone());
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1p5()
                                    .child(
                                        Icon::new(IconName::Check)
                                            .with_size(px(13.))
                                            .flex_none()
                                            .text_color(cx.theme().success),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(if mine { "已发送" } else { "已保存" }),
                                    ),
                            )
                            .child(
                                Button::new(SharedString::from(format!("open-{key}")))
                                    .label("打开")
                                    .outline()
                                    .xsmall()
                                    .on_click(move |_, _, cx| {
                                        let _ = cx.reveal_path(&target);
                                    }),
                            )
                            .into_any_element()
                    }
                    _ => {
                        // 三种状态：传输记录还在且完成 → 已完成；
                        // 传输记录已不在（重启/回收后的历史卡片）→ 按收发方向定论；
                        // 只有"记录在且未完成"才可能是等待接收
                        let label = if progress.is_none() {
                            if mine { "已发送" } else { "已接收" }
                        } else if done {
                            "已完成"
                        } else {
                            "等待对方接收"
                        };
                        let show_check = label != "等待对方接收";
                        h_flex()
                            .w_full()
                            .gap_1p5()
                            .when(show_check, |el| {
                                el.child(
                                    Icon::new(IconName::Check)
                                        .with_size(px(13.))
                                        .flex_none()
                                        .text_color(cx.theme().success),
                                )
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(label),
                            )
                            .into_any_element()
                    }
                },
            })
            .into_any_element()
    }

    /// 打开原生对话框选择并发送。pick_files=true 选文件（多选），
    /// false 选文件夹——Windows 的 IFileOpenDialog 里 FOS_PICKFOLDERS 是切换语义，
    /// 文件与文件夹不能混选，所以分成两个入口。
    pub fn pick_and_send(&mut self, window: &mut Window, cx: &mut Context<Self>, pick_files: bool) {
        let Some(peer) = self.selected.clone() else {
            self.toast("请先选择设备", true);
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            // prompt_for_paths 返回 oneshot，await 后是双层 Result<Option<Vec<PathBuf>>>
            let picked: Vec<PathBuf> = match cx.update(|_, cx| {
                cx.prompt_for_paths(PathPromptOptions {
                    files: pick_files,
                    directories: !pick_files,
                    multiple: true,
                    prompt: None,
                })
            }) {
                Ok(rx) => rx
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                    .flatten()
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            if picked.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                // 网页会话：发布到网页接收而非发给对端
                let cmd = if peer == transfer_core::proto::WEB_PEER_ID {
                    UiCommand::WebOffer { paths: picked }
                } else {
                    UiCommand::SendFiles {
                        peer_id: peer,
                        paths: picked,
                    }
                };
                if let Err(e) = this.core.send(cmd) {
                    this.toast(format!("发送失败: {e}"), true);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 选择文件发布到网页接收页（浏览器打开本机地址即可下载，无需装客户端）
    pub fn web_offer_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let picked: Vec<PathBuf> = match cx.update(|_, cx| {
                cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: None,
                })
            }) {
                Ok(rx) => rx
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                    .flatten()
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            if picked.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = this.core.send(UiCommand::WebOffer { paths: picked }) {
                    this.toast(format!("发布失败: {e}"), true);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 读剪贴板文本，直接发给当前设备
    pub fn paste_and_send(&mut self, cx: &mut Context<Self>) {
        let Some(peer) = self.selected.clone() else {
            self.toast("请先选择设备", true);
            return;
        };
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        let text = text.trim().to_string();
        if text.is_empty() {
            self.toast("剪贴板没有文本", true);
            return;
        }
        let cmd = if peer == transfer_core::proto::WEB_PEER_ID {
            UiCommand::WebText { text }
        } else {
            UiCommand::SendText { peer_id: peer, text }
        };
        if let Err(e) = self.core.send(cmd) {
            self.toast(format!("发送失败: {e}"), true);
        }
    }
}
