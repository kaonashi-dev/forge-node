//! The integrated shell: `SplitView` + `Sidebar` + `Toolbar`, a session tab
//! strip, and one terminal pane in the center drawn by the terminal compositor.

use std::time::Instant;

use rune::{Cx, El, Icon, col, effect, el, row, text};
use rune_kit::shell::{
    Sidebar, SidebarRow, SidebarToggle, SplitView, TabItem, TabItemStatus, TabOrientation,
    TabStrip, Toolbar,
};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::{AppState, CommandStatus, LinkStatus, SessionRow, SessionStatus};
use crate::terminal::input::{Pointer, Press};
use crate::terminal::renderer::{CELL_PT, MARK_BASE};

static PLUS: Icon = Icon {
    name: "shell-plus",
    view: 16.0,
    body: r#"<path d="M8 3.5V12.5M3.5 8H12.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};

pub fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    s.metrics.note_ui_thread();
    let started = Instant::now();
    let k = Kit::neutral(cx.system_dark());
    let main = col()
        .key("main-pane")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(tabbar(s, cx, &k))
        .child(center(s, &k));
    let shell = SplitView::new(main)
        .sidebar(
            &s.sidebar,
            |s: &mut AppState| &mut s.sidebar,
            sidebar(s, &k),
        )
        .toolbar(toolbar(s, &k))
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(18.0, 10.0)
        .sidebar_toggle(
            SidebarToggle::new()
                .size(ControlSize::Large)
                .past_lights(16.0)
                .edge(18.0)
                .gap(4.0)
                .in_header(),
        )
        .reduced_motion(cx.is_reduced_motion())
        .build(&k);
    // A focusable ancestor is the only public way to move focus on a sidebar
    // press in this Rune pin (`Runtime::set_focus` is crate-private). Sidebar
    // rows are select-only, so the press walks up to this proxy; the proxy's
    // key handler is the same terminal route as `AppSpec::on_key`.
    let root = col()
        .key("terminal-focus-proxy")
        .w_full()
        .h_full()
        .min_h(0.0)
        .focusable(true)
        .no_focus_ring()
        .on_key(|s: &mut AppState, ev, tx| s.terminal_key(ev, tx))
        .child(shell)
        .on_paste(|s: &mut AppState, text: &str, tx: &mut rune::Tx| {
            s.terminal_paste(text, tx);
        });
    s.view_ms
        .set(Some(started.elapsed().as_secs_f64() * 1000.0));
    if let Some(switch) = s.switch_started.take() {
        s.switch_ms
            .set(Some(switch.elapsed().as_secs_f64() * 1000.0));
    }
    root
}

fn toolbar(s: &AppState, k: &Kit) -> Toolbar {
    let create = Button::new("New shell")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut AppState| s.request_create())
        .build(k)
        .key("new-session");
    let rename = Button::new("Rename")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut AppState| s.request_rename())
        .build(k)
        .key("rename-session");
    let refresh = Button::new("Refresh")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut AppState| s.request_refresh())
        .build(k)
        .key("refresh-snapshot");
    let reconnect = Button::new("Reconnect")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut AppState| s.request_reconnect())
        .build(k)
        .key("force-reconnect");
    Toolbar::new()
        .title(format!("Forge Rune · {}", s.status.label()))
        .trailing(refresh)
        .trailing(reconnect)
        .trailing(rename)
        .trailing(create)
}

fn tabbar(s: &AppState, cx: &mut Cx<AppState>, k: &Kit) -> El {
    let items = s.rows.iter().map(|session| {
        TabItem::new(session.id_text.as_str(), session.title.as_str())
            .status(tab_status(session.status))
            .closable(false)
    });
    let strip = TabStrip::new("session-tabs", TabOrientation::Horizontal)
        .tabs(items)
        .active(s.active_text.as_deref())
        .viewport_width(cx.layout_size("session-tabs").map(|size| size[0]))
        .drag_state(&s.tabs, |s: &mut AppState| &mut s.tabs)
        .label("Sessions")
        .on_select(|s: &mut AppState, id: &str, _| {
            if let Ok(id) = id.parse::<domain::SessionId>() {
                s.select(id);
            }
        })
        .build(k);
    row()
        .key("tabbar")
        .w_full()
        .h(44.0)
        .px(8.0)
        .center_y()
        .fill(k.c.bg_deep)
        .child(strip)
}

fn sidebar(s: &AppState, k: &Kit) -> El {
    let header = SidebarRow::new("New shell")
        .select_only()
        .icon(&PLUS)
        .height(32.0)
        .detail("⌘N")
        .on_select(|s: &mut AppState, _| s.request_create())
        .build(k)
        .key("sidebar-new-session");
    let body = Sidebar::new()
        .label("Sessions")
        .inset(0.0)
        .header_gap(4.0)
        .header_padding(0.0, 12.0)
        .row_gap(4.0)
        .header(header)
        .footer(status_row(s, k));
    let body = if s.rows.is_empty() {
        body.child(empty_row(k))
    } else {
        s.rows
            .iter()
            .enumerate()
            .fold(body, |body, (index, session)| {
                body.child(session_row(s, session, index, k))
            })
    };
    body.build(k)
}

fn session_row(s: &AppState, session: &SessionRow, index: usize, k: &Kit) -> El {
    let selected = s.active == Some(session.id);
    let id = session.id;
    let mut item = SidebarRow::new(session.title.as_str())
        .select_only()
        .height(46.0)
        .avatar(avatar(index, k))
        .subtitle(format!("{} · {}", session.workspace, session.detail).as_str())
        .selected(selected)
        .on_select(move |s: &mut AppState, _| s.select(id))
        .on_double_click(move |s: &mut AppState, _| {
            s.select(id);
            s.request_rename();
        });
    if let Some(status) = session.status {
        item = item.status(row_status(status));
    }
    item.build(k).key(format!("session-row-{index}"))
}

fn avatar(index: usize, k: &Kit) -> El {
    let fills = [
        k.c.bg_accent,
        k.c.info.icon,
        k.c.success.icon,
        k.c.warning.icon,
    ];
    let fill = fills[index % fills.len()];
    row()
        .size(18.0, 18.0)
        .radius(5.0)
        .center()
        .fill(fill)
        .child(
            text(format!("{}", index + 1))
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_inverse),
        )
}

fn empty_row(k: &Kit) -> El {
    row().key("empty-sessions").w_full().px(10.0).py(8.0).child(
        text("No sessions in this snapshot")
            .font(k.ty.small)
            .color(k.c.text_faint),
    )
}

fn status_row(s: &AppState, k: &Kit) -> El {
    let color = match s.status {
        LinkStatus::Connected => k.c.success.icon,
        LinkStatus::Connecting => k.c.warning.icon,
        LinkStatus::Reconnecting => k.c.danger.icon,
    };
    let detail = format!(
        "gen {} · {} deltas · {} ui sock",
        s.generation,
        s.metrics
            .terminal_deltas
            .load(std::sync::atomic::Ordering::Relaxed),
        s.metrics
            .ui_socket_ops
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    row()
        .key("connection-status")
        .w_full()
        .h(30.0)
        .px(8.0)
        .gap(7.0)
        .center_y()
        .child(el().size(8.0, 8.0).radius(4.0).fill(color))
        .child(
            text(s.status.label())
                .font(k.ty.small_medium)
                .color(k.c.text_muted),
        )
        .child(text(detail).font(k.ty.text_xsmall).color(k.c.text_faint))
}

fn center(s: &AppState, k: &Kit) -> El {
    let has_terminal = s.active_row().and_then(|row| row.terminal_id).is_some();
    let body = if has_terminal {
        terminal_surface(s, k)
    } else {
        placeholder(s, k)
    };
    col()
        .key("center")
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(k.c.bg_base)
        .child(body)
}

fn terminal_surface(s: &AppState, k: &Kit) -> El {
    let title = s
        .active_row()
        .map(|row| row.title.clone())
        .unwrap_or_else(|| "Terminal".to_string());
    let active = s.terminal.focus_terminal;
    let color = if active {
        k.c.success.icon
    } else {
        k.c.warning.icon
    };
    let header = row()
        .key("terminal-focus-indicator")
        .role("status")
        .label("Keyboard input: terminal")
        .description(if active {
            "Terminal has keyboard input"
        } else {
            "Click the terminal pane to focus it"
        })
        .w_full()
        .h(24.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .fill(k.c.bg_deep)
        .child(el().size(6.0, 6.0).radius(3.0).fill(color))
        .child(
            text(format!("Terminal · {title}"))
                .font(k.ty.small_medium)
                .color(k.c.text_muted),
        )
        .child(
            text(if active {
                "keyboard active"
            } else {
                "click to focus"
            })
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint),
        );
    col()
        .key("terminal-surface")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(header)
        .child(terminal_pane(s, k))
}

fn terminal_pane(s: &AppState, k: &Kit) -> El {
    let title = s
        .active_row()
        .map(|row| row.title.clone())
        .unwrap_or_else(|| "Terminal".to_string());
    let cursor = s.terminal.cursor().unwrap_or_default();
    let mut pane = col()
        .key("terminal-pane")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .role("log")
        .label(format!("Terminal · {title}"))
        .description(format!(
            "Cursor on row {}: {}",
            cursor.line + 1,
            s.terminal.mirror_cursor
        ))
        .a11y_value(s.terminal.mirror.clone())
        .live_region(rune::LiveRegion::Off)
        .focusable(true)
        .on_key(|s: &mut AppState, ev, tx| s.terminal_key(ev, tx))
        .on_wheel(|s: &mut AppState, ev, tx| s.terminal_wheel(&ev, tx))
        .on_drag(|s: &mut AppState, drag, tx| {
            let phase = match drag.phase {
                rune::Phase::Down => Press::Down,
                rune::Phase::Move => Press::Move,
                _ => Press::Up,
            };
            s.terminal_pointer(
                Pointer {
                    phase,
                    x: drag.x,
                    y: drag.y,
                    clicks: drag.clicks,
                    cancelled: drag.cancelled(),
                    shift: tx.mods().shift,
                },
                tx,
            );
        })
        .on_secondary_press_local(|s: &mut AppState, p, tx| {
            s.terminal_secondary(p.x, p.y, tx);
        })
        .child(effect(MARK_BASE).absolute(0.0, 0.0).w_full().h_full());
    if let Some(notice) = s.terminal.attach_notice() {
        pane = pane.child(
            row()
                .key("terminal-hold-notice")
                .role("status")
                .label(notice.clone())
                .absolute(8.0, 8.0)
                .px(8.0)
                .h(22.0)
                .radius(4.0)
                .fill(k.c.bg_deep)
                .border(1.0, k.c.warning.border)
                .center_y()
                .child(text(notice).font(k.ty.small).color(k.c.warning.fg)),
        );
    }
    if s.terminal.focus_terminal {
        let (x, y) = (
            cursor.col as f32 * CELL_PT.0,
            cursor.line as f32 * CELL_PT.1,
        );
        pane = pane.child(
            el().key("terminal-ime")
                .absolute(x, y)
                .size(CELL_PT.0, CELL_PT.1)
                .ime_cursor()
                .a11y_hidden(),
        );
    }
    pane
}

fn placeholder(s: &AppState, k: &Kit) -> El {
    let title = s
        .active_row()
        .map(|row| row.title.clone())
        .unwrap_or_else(|| "No active session".to_string());
    col()
        .key("center-placeholder")
        .w_full()
        .h_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(24.0)
        .gap(10.0)
        .center_y()
        .child(text(title).font(k.ty.body_medium).color(k.c.text_base))
        .child(
            text("This session has no terminal; the shell replica is still live.")
                .font(k.ty.small)
                .color(k.c.text_faint),
        )
        .child(divider(k))
        .child(section(k, "Client worker"))
        .children(status_stats(s, k))
        .child(divider(k))
        .child(section(k, "Last command"))
        .child(command_line(s.last_command.as_ref(), k))
}

fn section(k: &Kit, title: &str) -> El {
    text(title).font(k.ty.small_medium).color(k.c.text_muted)
}

fn divider(k: &Kit) -> El {
    el().w_full().h(1.0).fill(k.c.border_weak)
}

fn status_stats(s: &AppState, k: &Kit) -> Vec<El> {
    let mut out = vec![
        stat(
            "Socket",
            format!(
                "{} ops · {} on UI thread · thread {}",
                s.metrics
                    .socket_ops
                    .load(std::sync::atomic::Ordering::Relaxed),
                s.metrics
                    .ui_socket_ops
                    .load(std::sync::atomic::Ordering::Relaxed),
                s.metrics.socket_thread()
            ),
            k,
        ),
        stat(
            "Snapshot",
            match s.last_snapshot_us {
                Some(us) => format!("{:.2} ms · gen {}", us as f64 / 1000.0, s.generation),
                None => format!("waiting · gen {}", s.generation),
            },
            k,
        ),
        stat(
            "Status",
            format!("{} · {}", s.status.label(), s.status_detail),
            k,
        ),
        stat(
            "Terminal deltas",
            s.metrics
                .terminal_deltas
                .load(std::sync::atomic::Ordering::Relaxed)
                .to_string(),
            k,
        ),
        stat(
            "Stale events dropped",
            s.metrics
                .stale_events
                .load(std::sync::atomic::Ordering::Relaxed)
                .to_string(),
            k,
        ),
        stat(
            "UI view build",
            match s.view_ms.get() {
                Some(ms) => format!("{ms:.2} ms"),
                None => "not measured".to_string(),
            },
            k,
        ),
        stat(
            "Last switch (click → paint)",
            match s.switch_ms.get() {
                Some(ms) => format!("{ms:.2} ms"),
                None => "not measured".to_string(),
            },
            k,
        ),
    ];
    if let Some((bytes, summary)) = &s.last_input {
        out.push(stat("Last input", format!("{bytes} bytes · {summary}"), k));
    }
    if let Some(ms) = s.metrics.command_us() {
        out.push(stat(
            "Worker command",
            format!("{:.2} ms", ms as f64 / 1000.0),
            k,
        ));
    }
    out
}

fn stat(label: &str, value: String, k: &Kit) -> El {
    row()
        .w_full()
        .gap(8.0)
        .child(
            row().w(170.0).child(
                text(label.to_string())
                    .font(k.ty.small)
                    .color(k.c.text_faint),
            ),
        )
        .child(text(value).font(k.ty.mono).color(k.c.text_muted).truncate())
}

fn command_line(command: Option<&CommandStatus>, k: &Kit) -> El {
    let Some(command) = command else {
        return text("no command sent yet")
            .font(k.ty.small)
            .color(k.c.text_faint);
    };
    let color = if command.ok {
        k.c.success.fg
    } else {
        k.c.danger.fg
    };
    row()
        .w_full()
        .gap(8.0)
        .child(
            text(command.label)
                .font(k.ty.small_medium)
                .color(k.c.text_base),
        )
        .child(
            text(format!("{} · {} µs", command.message, command.took_us))
                .font(k.ty.small)
                .color(color)
                .truncate(),
        )
}

fn tab_status(status: Option<SessionStatus>) -> Option<TabItemStatus> {
    match status {
        Some(SessionStatus::Busy) => Some(TabItemStatus::Busy),
        Some(SessionStatus::Attention) => Some(TabItemStatus::Attention),
        None => None,
    }
}

fn row_status(status: SessionStatus) -> rune_kit::shell::RowStatus {
    match status {
        SessionStatus::Busy => rune_kit::shell::RowStatus::Busy,
        SessionStatus::Attention => rune_kit::shell::RowStatus::Attention,
    }
}
