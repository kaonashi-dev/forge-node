//! The integrated shell: `SplitView` + `Sidebar` + `Toolbar`, a session tab
//! strip, and the active session in the center: a terminal drawn by the
//! terminal compositor, or an editor drawn from the daemon's windows of lines.

use std::time::Instant;

use rune::{Cx, El, col, effect, el, icon, row, text};
use rune_kit::shell::{SidebarRow, SidebarToggle, SplitView, Toolbar};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, Loader};

use crate::app::{AppState, CommandStatus, LinkStatus, SessionRow, SessionStatus};
use crate::terminal::input::{Pointer, Press};
use crate::terminal::renderer::MARK_BASE;
use crate::workbench::CenterMode;

/// A spinner while work is running, or a still ellipsis when ui.reduce_motion
/// (or the system setting) says not to animate.
#[must_use]
pub fn busy(s: &AppState, k: &Kit) -> El {
    if s.reduce_motion() {
        text("…")
            .role("progressbar")
            .label("Loading")
            .font(k.ty.small)
            .color(k.c.icon_muted)
    } else {
        Loader::new().size(14.0).color(k.c.icon_muted).build(k)
    }
}

pub fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    s.metrics.note_ui_thread();
    let started = Instant::now();
    let mut k = crate::theme::kit(s.theme_base(cx.system_dark()), cx.system_dark());
    k.ty = crate::settings::prefs::scale_type(k.ty, s.chrome_font_size());
    crate::settings::prefs::apply_kit(
        &mut k,
        s.density(),
        s.reduce_motion(),
        s.glass(),
        s.trimmed(),
    );
    let metrics = s.chrome_metrics();
    let mut overlays = Vec::new();
    let main = col()
        .key("main-pane")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(center(s, &k, &mut overlays));
    let shell = SplitView::new(main)
        .sidebar(
            &s.sidebar,
            |s: &mut AppState| &mut s.sidebar,
            sidebar(s, &k),
        )
        .toolbar(toolbar(s, &k))
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(metrics.frame)
        .toolbar_in_panel()
        .sidebar_padding(metrics.sidebar_pad[0], metrics.sidebar_pad[1])
        .sidebar_toggle(
            SidebarToggle::new()
                .size(ControlSize::Large)
                .past_lights(16.0)
                .edge(18.0)
                .gap(4.0)
                .in_header(),
        )
        .reduced_motion(cx.is_reduced_motion() || s.reduce_motion())
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
            s.paste(text, tx);
        });
    overlays.extend(crate::chrome::drop_marker(s, &k));
    if s.palette.open {
        palette(s, &k, &mut overlays);
    }
    crate::projects::dialogs(s, &k, &mut overlays);
    crate::sync::dialog(s, &k, &mut overlays);
    rename_dialog(s, &k, &mut overlays);
    crate::pr_review::dialog(s, &k, &mut overlays);
    crate::agent_ops::dialogs(s, &k, &mut overlays);
    close_dialog(s, &k, &mut overlays);
    crate::history::delete_dialog(s, &k, &mut overlays);
    crate::settings::dialogs(s, &k, &mut overlays);
    crate::storage::dialogs(s, &k, &mut overlays);
    crate::menus::layer(s, &k, &mut overlays);
    crate::native_menus::dialog(s, &k, &mut overlays);
    tab_switcher(s, &k, &mut overlays);
    overlays.extend(crate::toasts::layer(s, &k));
    let root = rune_kit::root(root, overlays);
    s.view_ms
        .set(Some(started.elapsed().as_secs_f64() * 1000.0));
    if let Some(switch) = s.switch_started.take() {
        s.switch_ms
            .set(Some(switch.elapsed().as_secs_f64() * 1000.0));
    }
    root
}

/// the original client's card strip: the checkout's tabs, then its actions; the band the
/// window drags by.
fn toolbar(s: &AppState, k: &Kit) -> Toolbar {
    let mut bar = Toolbar::new().leading(crate::chrome::strip(s, k));
    if s.glass() && k.dark {
        bar = bar.material(rune::SystemMaterial::Glass);
    }
    // Refresh and Reconnect are probes of the client, not things a person does.
    if s.debug {
        let probe = |label: &'static str, key: &'static str, press: fn(&mut AppState)| {
            Button::new(label)
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(press)
                .build(k)
                .key(key)
        };
        bar = bar
            .trailing(probe(
                "Refresh",
                "refresh-snapshot",
                AppState::request_refresh,
            ))
            .trailing(probe(
                "Reconnect",
                "force-reconnect",
                AppState::request_reconnect,
            ));
    }
    let [create, jump] = crate::chrome::actions(k);
    bar.trailing(create).trailing(jump)
}

/// the original client's sidebar: the view switcher, the chosen view, and the footer. Each
/// view scrolls itself, so Git can keep its commit box in reach.
fn sidebar(s: &AppState, k: &Kit) -> El {
    use crate::workbench::Panel;
    if s.workbench.mode == CenterMode::Settings {
        return col()
            .key("sidebar")
            .role("complementary")
            .label("Sidebar")
            .w_full()
            .h_full()
            .min_h(0.0)
            .gap(10.0)
            .child(
                col()
                    .key("sidebar-panel")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .child(crate::settings::nav(s, k)),
            )
            .child(status_row(s, k));
    }
    let panel = match s.workbench.panel {
        Panel::Projects => crate::projects::view(s, k),
        Panel::Files => crate::workbench::view::files_panel(s, k),
        Panel::History => crate::history::view(s, k),
        Panel::PullRequests => crate::pr_review::view(s, k),
        Panel::Git => crate::workbench::view::git_panel(s, k),
        Panel::Processes => crate::processes::view(s, k),
    };
    col()
        .key("sidebar")
        .role("complementary")
        .label("Sidebar")
        .w_full()
        .h_full()
        .min_h(0.0)
        .gap(10.0)
        .child(crate::chrome::views(s, k))
        .child(
            col()
                .key("sidebar-panel")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(panel),
        )
        .child(status_row(s, k))
}

/// A session under its checkout in Projects: select-only, so a press walks
/// up to the terminal's focus proxy and typing reaches the terminal.
pub fn session_row(s: &AppState, session: &SessionRow, index: usize, k: &Kit) -> El {
    let selected = s.active == Some(session.id) && s.workbench.mode == CenterMode::Session;
    let id = session.id;
    let glyph = crate::glyphs::for_row(session, selected, !k.dark);
    let roles = crate::theme::roles(k);
    let mut item = SidebarRow::new(session.title.as_str())
        .select_only()
        .height(s.chrome_metrics().row)
        .avatar(icon(glyph, 14.0).color(match session.status {
            _ if selected => k.c.icon_base,
            Some(SessionStatus::Attention) => roles.needs_you.solid,
            Some(SessionStatus::Busy) => k.c.icon_accent,
            None => k.c.icon_muted,
        }))
        .selected(selected)
        .on_select(move |s: &mut AppState, _| {
            s.select(id);
            s.show_mode(CenterMode::Session);
        })
        .on_double_click(move |s: &mut AppState, _| {
            s.select(id);
            s.request_rename();
        });
    if let Some(status) = session.status {
        item = item.status(match status {
            SessionStatus::Attention => rune_kit::shell::RowStatus::Other(
                el().size(7.0, 7.0).radius(3.5).fill(roles.needs_you.solid),
            ),
            SessionStatus::Busy => rune_kit::shell::RowStatus::Busy,
        });
    }
    if let Some(ended) = &session.ended {
        item = item.detail(ended.as_str());
    }
    item.build(k)
        .key(format!("session-row-{index}"))
        .on_secondary_press(move |s: &mut AppState, point, _| {
            crate::menus::open(s, crate::menus::Target::Session(id), point);
        })
}

fn status_row(s: &AppState, k: &Kit) -> El {
    if !s.debug {
        return crate::chrome::footer(s, k);
    }
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

fn center(s: &AppState, k: &Kit, layer: &mut Vec<El>) -> El {
    let has_terminal = s.active_row().and_then(|row| row.terminal_id).is_some();
    // A session of another checkout is not this window's to show.
    let in_checkout = s
        .active
        .is_some_and(|active| s.tab_rows().iter().any(|row| row.id == active));
    let body = if s.workbench.mode == CenterMode::Changes {
        crate::workbench::view::changes(s, k)
    } else if s.workbench.mode == CenterMode::Files {
        crate::workbench::view::preview(s, k)
    } else if s.workbench.mode == CenterMode::Search {
        crate::workbench::search_view::view(s, k)
    } else if s.workbench.mode == CenterMode::Settings {
        crate::settings::page(s, k)
    } else if s.workbench.mode == CenterMode::SessionChanges {
        crate::session_changes::view(s, k)
    } else if s.workbench.mode == CenterMode::Review {
        crate::code_review::view(s, k)
    } else if s.workbench.mode == CenterMode::Compare {
        crate::workbench::compare::view(s, k, layer)
    } else if s.workbench.mode == CenterMode::PrDetail {
        crate::pr_detail::view(s, k)
    } else if s.workbench.mode == CenterMode::PrCompose {
        crate::pr_compose::view(s, k)
    } else if !in_checkout {
        empty_center(s, k)
    } else if s.active_line_editor().is_some() {
        crate::editor::view::pane(s, k)
    } else if has_terminal {
        terminal_surface(s, k)
    } else {
        placeholder(s, k)
    };
    col()
        .key("center")
        .w_full()
        // What the tab strip leaves, not the column's whole height.
        .grow(1.0)
        .min_h(0.0)
        .fill(k.c.bg_base)
        .child(body)
}

fn terminal_surface(s: &AppState, k: &Kit) -> El {
    let active = s.terminal.focus_terminal;
    let color = if active {
        k.c.success.icon
    } else {
        k.c.warning.icon
    };
    let focus = row()
        .key("terminal-focus-indicator")
        .role("status")
        .label("Keyboard input: terminal")
        .description(if active {
            "Terminal has keyboard input"
        } else {
            "Click the terminal pane to focus it"
        })
        .gap(6.0)
        .center_y()
        .child(el().size(6.0, 6.0).radius(3.0).fill(color))
        .child(
            text(if active {
                "keyboard active"
            } else {
                "click to focus"
            })
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint),
        );
    let header = row()
        .key("session-header")
        .w_full()
        .h(28.0)
        .px(10.0)
        .gap(10.0)
        .center_y()
        .fill(k.c.bg_deep)
        .child(session_crumbs(s, k))
        .children(provider_chip(s, k))
        .children(s.debug.then_some(focus));
    let body = if s.pane_count() == 2 {
        let ratio = s.split_ratio();
        row()
            .key("terminal-split")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .on_layout_tx(|s: &mut AppState, rect, tx| {
                s.split.width = rect[2];
                tx.live_only();
            })
            .child(pane_inset(s, k, 0).w(0.0).grow(ratio).min_w(0.0))
            .child(split_handle(k))
            .child(
                col()
                    .key("terminal-split-extra")
                    .w(0.0)
                    .grow(1.0 - ratio)
                    .min_w(0.0)
                    .h_full()
                    .child(split_header(s, k))
                    .child(pane_inset(s, k, 1).w_full().grow(1.0)),
            )
    } else {
        pane_inset(s, k, 0).w_full().grow(1.0)
    };
    col()
        .key("terminal-surface")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(header)
        .child(body)
}

/// the original client's terminalPadX / terminalPadY, painted in the terminal's own ground;
/// the pane inside keeps its coordinates.
fn pane_inset(s: &AppState, k: &Kit, idx: usize) -> El {
    col()
        .key(if idx == 0 {
            "terminal-inset"
        } else {
            "terminal-inset-split"
        })
        .min_h(0.0)
        .px(24.0)
        .py(16.0)
        .fill(terminal_ground(s, k))
        .child(terminal_pane(s, k, idx))
}

/// A column divider that stores a fraction, as the original client's `SplitHandle`: a
/// remembered width of one column would starve the other after a resize.
fn split_handle(k: &Kit) -> El {
    el().key("terminal-split-handle")
        .role("separator")
        .label("Resize the split")
        .w(5.0)
        .h_full()
        .fill(k.c.border_base)
        .cursor(rune::Cursor::ResizeColumn)
        .on_drag(|s: &mut AppState, drag, tx| {
            let from = match drag.phase {
                rune::Phase::Down => {
                    let ratio = s.split_ratio();
                    s.split.drag_from = Some((drag.dx, ratio));
                    return;
                }
                _ => s.split.drag_from,
            };
            let Some((x0, ratio)) = from else {
                return;
            };
            if drag.phase == rune::Phase::Move {
                let width = s.split.width.max(1.0);
                s.split.dragging = Some(crate::terminal::split::clamp_ratio(
                    ratio + (drag.dx - x0) / width,
                ));
                tx.instant();
            } else if drag.cancelled() {
                s.split.dragging = None;
                s.split.drag_from = None;
            } else {
                s.commit_split_ratio();
            }
        })
        .on_double_press(|s: &mut AppState| {
            s.split.dragging = Some(crate::terminal::split::RATIO_FALLBACK);
            s.commit_split_ratio();
        })
}

/// The split column's own header: its session, and the join that closes it.
fn split_header(s: &AppState, k: &Kit) -> El {
    let title = s
        .split
        .pane
        .as_ref()
        .and_then(|pane| s.rows.iter().find(|row| row.id == pane.session))
        .map_or_else(|| "Terminal".to_string(), |row| row.title.clone());
    row()
        .key("terminal-split-header")
        .w_full()
        .h(28.0)
        .pl(10.0)
        .pr(4.0)
        .gap(6.0)
        .center_y()
        .fill(k.c.bg_deep)
        .child(
            text(title)
                .role("heading")
                .font(k.ty.small_medium)
                .color(if s.focused_pane() == 1 {
                    k.c.text_base
                } else {
                    k.c.text_muted
                })
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(
            row()
                .key("terminal-split-join")
                .role("button")
                .label("Join panes")
                .size(22.0, 22.0)
                .radius(5.0)
                .center()
                .hover(k.c.hover)
                .on_press(|s: &mut AppState| s.join_panes())
                .child(icon(&crate::glyphs::CLOSE, 14.0).color(k.c.icon_muted)),
        )
}

/// The active theme's terminal background, as a Rune colour.
fn terminal_ground(s: &AppState, k: &Kit) -> rune::Color {
    let packed = crate::terminal::palette::Palette::for_base(s.theme_base(k.dark)).bg;
    let channel = |shift: u32| (packed >> shift) & 0xff;
    let srgb = format!("#{:02x}{:02x}{:02x}", channel(0), channel(8), channel(16));
    rune::color::hex(&srgb).unwrap_or(k.c.bg_base)
}

/// the original client's `SessionHeader` crumbs: project / checkout / session.
fn session_crumbs(s: &AppState, k: &Kit) -> El {
    let mut crumbs = row()
        .key("session-crumbs")
        .role("navigation")
        .label("Session location")
        .gap(6.0)
        .grow(1.0)
        .min_w(0.0)
        .center_y();
    let session = s.active.and_then(|id| {
        s.store
            .as_ref()?
            .sessions
            .iter()
            .find(|session| session.id == id)
    });
    let store = s.store.as_ref();
    let workspace = session.and_then(|session| {
        store?
            .workspaces
            .iter()
            .find(|workspace| workspace.id == session.workspace_id)
    });
    let project = workspace.and_then(|workspace| {
        store?
            .projects
            .iter()
            .find(|project| project.id == workspace.project_id)
    });
    let sep = || {
        text("/")
            .font(k.ty.small)
            .color(k.c.text_faint)
            .a11y_hidden()
    };
    if let Some(project) = project {
        crumbs = crumbs
            .child(
                text(project.name.clone())
                    .font(k.ty.small)
                    .color(k.c.text_muted)
                    .truncate(),
            )
            .child(sep());
    }
    if let Some(workspace) = workspace {
        let place = workspace
            .display_name
            .clone()
            .or_else(|| workspace.branch.clone())
            .unwrap_or_else(|| workspace.path.display().to_string());
        crumbs = crumbs
            .child(text(place).font(k.ty.mono).color(k.c.text_muted).truncate())
            .child(sep());
    }
    let title = s
        .active_row()
        .map_or_else(|| "Terminal".to_string(), |row| row.title.clone());
    crumbs.child(
        text(title)
            .role("heading")
            .font(k.ty.small_medium)
            .color(k.c.text_base)
            .truncate(),
    )
}

/// The agent's provider by its display name, for an agent session.
fn provider_chip(s: &AppState, k: &Kit) -> Option<El> {
    let store = s.store.as_ref()?;
    let provider = store
        .sessions
        .iter()
        .find(|session| Some(session.id) == s.active)?
        .agent_provider_id
        .as_ref()?;
    let name = store
        .providers
        .iter()
        .find(|info| &info.descriptor.id == provider)
        .map_or_else(
            || provider.to_string(),
            |info| info.descriptor.display_name.clone(),
        );
    Some(
        row()
            .key("session-chip")
            .px(8.0)
            .py(2.0)
            .radius(10.0)
            .fill(k.c.surface_hover)
            .child(text(name).font(k.ty.text_xsmall).color(k.c.text_muted)),
    )
}

fn terminal_pane(s: &AppState, k: &Kit, idx: usize) -> El {
    let ui = s.pane(idx);
    let title = s
        .pane_session(idx)
        .and_then(|session| s.rows.iter().find(|row| row.id == session))
        .map(|row| row.title.clone())
        .unwrap_or_else(|| "Terminal".to_string());
    let cursor = ui.cursor().unwrap_or_default();
    let mut pane = col()
        .key(if idx == 0 {
            "terminal-pane"
        } else {
            "terminal-pane-split"
        })
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .role("log")
        .label(format!("Terminal · {title}"))
        .description(format!(
            "Cursor on row {}: {}",
            cursor.line + 1,
            ui.mirror_cursor
        ))
        .a11y_value(ui.mirror.clone())
        .live_region(rune::LiveRegion::Off)
        .focusable(true)
        .on_key(move |s: &mut AppState, ev, tx| {
            if !ev.mods.cmd {
                s.focus_pane(idx);
            }
            s.terminal_key(ev, tx)
        })
        .on_wheel(move |s: &mut AppState, ev, tx| s.terminal_wheel(idx, &ev, tx))
        .on_drag(move |s: &mut AppState, drag, tx| {
            let phase = match drag.phase {
                rune::Phase::Down => Press::Down,
                rune::Phase::Move => Press::Move,
                _ => Press::Up,
            };
            let mods = tx.mods();
            s.terminal_pointer(
                idx,
                Pointer {
                    phase,
                    x: drag.x,
                    y: drag.y,
                    clicks: drag.clicks,
                    cancelled: drag.cancelled(),
                    shift: mods.shift,
                    plain: !(mods.shift || mods.cmd || mods.alt || mods.ctrl),
                },
                tx,
            );
        })
        .on_secondary_press_local(move |s: &mut AppState, p, tx| {
            s.terminal_secondary(idx, p.x, p.y, tx);
        })
        .on_pointer_move_local(move |s: &mut AppState, p, tx| {
            s.terminal_hover(idx, Some((p.x, p.y)), tx);
        })
        .on_hover_change(move |s: &mut AppState, over, tx| {
            if !over {
                s.terminal_hover(idx, None, tx);
            }
        })
        .on_drop_files(move |s: &mut AppState, files, tx| s.terminal_drop(idx, files, tx))
        .child(
            effect(MARK_BASE + idx as u32)
                .absolute(0.0, 0.0)
                .w_full()
                .h_full(),
        );
    if ui.link.is_some() {
        pane = pane.cursor(rune::Cursor::Pointer);
    }
    if let Some(notice) = ui.attach_notice() {
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
    if s.terminal.focus_terminal && s.focused_pane() == idx {
        let cell = s.cell_pt();
        let (x, y) = (cursor.col as f32 * cell.0, cursor.line as f32 * cell.1);
        pane = pane.child(
            el().key("terminal-ime")
                .absolute(x, y)
                .size(cell.0, cell.1)
                .ime_cursor()
                .a11y_hidden(),
        );
    }
    pane
}

fn placeholder(s: &AppState, k: &Kit) -> El {
    if !s.debug {
        return empty_center(s, k);
    }
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

fn palette(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let entries = s.palette.entries();
    let (scope, _) = s.palette.parsed();
    let field = rune_kit::TextInput::new(&s.palette.edit)
        .placeholder(s.palette.placeholder())
        .label("Command palette")
        .width(None)
        .on_edit(|s: &mut AppState| &mut s.palette.edit)
        .on_change(|s: &mut AppState, _| {
            s.palette.selected = 0;
            s.palette.clear_error();
        })
        .on_key(|s: &mut AppState, ev, _| s.palette_key(ev))
        .build(k)
        .key("palette-input");
    let mut heading = row().key("palette-scope").w_full().gap(8.0).items_center();
    if let Some((prefix, label)) = s.palette.chip() {
        heading = heading.child(
            row()
                .px(7.0)
                .py(2.0)
                .radius(5.0)
                .fill(k.c.surface_hover)
                .gap(4.0)
                .child(
                    text(prefix.to_string())
                        .font(k.ty.mono)
                        .color(k.c.text_faint)
                        .a11y_hidden(),
                )
                .child(
                    text(label)
                        .font(k.ty.text_xsmall_medium)
                        .color(k.c.text_muted),
                ),
        );
    }
    heading = heading.child(field);
    let mut list = col()
        .key("palette-list")
        .gap(2.0)
        .role("listbox")
        .label("Results");
    for (index, entry) in entries.iter().enumerate().take(30) {
        let selected = index == s.palette.selected;
        let chosen = entry.clone();
        let mut row = row()
            .key(format!("palette-{index}"))
            .role("option")
            .label(entry.label().to_string())
            .focus_selected(selected)
            .w_full()
            .h(30.0)
            .px(10.0)
            .gap(8.0)
            .items_center()
            .radius(6.0)
            .fill(if selected {
                k.c.surface_hover
            } else {
                rune::Color::TRANSPARENT
            })
            .on_press(move |s: &mut AppState| s.run_palette(chosen.clone()));
        let (mark, hint, note) = match entry {
            crate::palette::Entry::Command { command, .. } => (
                None,
                s.palette.chord(command).unwrap_or_default().to_string(),
                None,
            ),
            crate::palette::Entry::File(path) => (
                Some(crate::file_icons::for_path(path).icon(!k.dark)),
                "file".to_string(),
                None,
            ),
            crate::palette::Entry::Session {
                agent,
                kind,
                waiting,
                note,
                ..
            } => {
                let glyph = if *waiting {
                    &crate::glyphs::NEEDS_YOU
                } else {
                    crate::glyphs::for_session(*kind, agent.as_deref())
                };
                (
                    Some(glyph),
                    if *waiting { "waiting" } else { "" }.to_string(),
                    Some(note.as_str()),
                )
            }
            crate::palette::Entry::Checkout { note, .. } => (
                Some(&crate::glyphs::WORKTREE),
                "checkout".to_string(),
                Some(note.as_str()),
            ),
            crate::palette::Entry::Symbol(hit) => (
                Some(crate::file_icons::for_path(&hit.path).icon(!k.dark)),
                format!("{}:{}", hit.path, hit.line),
                Some(hit.text.as_str()),
            ),
        };
        if let Some(mark) = mark {
            row = row.child(icon(mark, 14.0));
        }
        row = row.child(
            text(entry.label().to_string())
                .font(k.ty.small)
                .color(k.c.text_base)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        );
        if let Some(note) = note.filter(|note| !note.is_empty()) {
            row = row.child(
                text(note.to_string())
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint)
                    .truncate()
                    .min_w(0.0)
                    .shrink(),
            );
        }
        if !hint.is_empty() {
            row = row.child(text(hint).font(k.ty.mono).color(k.c.text_faint).w(90.0));
        }
        list = list.child(row);
    }
    if entries.is_empty() {
        let mut empty = row().gap(8.0).items_center();
        if s.palette.searching() {
            empty = empty.child(busy(s, k));
        }
        list = list.child(
            empty.child(
                text(s.palette.empty_message())
                    .font(k.ty.small)
                    .color(k.c.text_faint),
            ),
        );
    }
    if let Some(error) = &s.palette.error {
        list = list.child(
            text(error.clone())
                .key("palette-error")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg),
        );
    }
    let title = match scope {
        crate::palette::Scope::Everything => "Command Palette".to_string(),
        crate::palette::Scope::Commands => "Commands".to_string(),
        crate::palette::Scope::Files => "Open File".to_string(),
        crate::palette::Scope::Places => "Go to".to_string(),
        crate::palette::Scope::Symbols => "Go to Symbol".to_string(),
        crate::palette::Scope::Launch => "New Session".to_string(),
    };
    rune_kit::Dialog::new("palette", title)
        .open(true)
        .click_outside(true)
        .width(640.0)
        .on_close(|s: &mut AppState| s.palette.close())
        .child(heading)
        .child(list)
        .build(k, layer);
}

/// Hold-Control tab list. It never takes focus: the gesture is keyboard-only,
/// Tab steps it, and releasing Control commits the highlighted row.
fn tab_switcher(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some((targets, index, foreign_at)) = s.tabs.shown_gesture() else {
        return;
    };
    let ids: Vec<domain::SessionId> = targets.to_vec();
    let mut card = col()
        .gap(2.0)
        .w(420.0)
        .padding(10.0)
        .radius(8.0)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_base);
    let mut list = col()
        .key("tab-switcher-list")
        .role("listbox")
        .label("Switch Tab")
        .gap(2.0);
    for (at, id) in ids.iter().enumerate() {
        let Some(session) = s.rows.iter().find(|row| row.id == *id) else {
            continue;
        };
        let selected = at == index;
        let glyph = crate::glyphs::for_row(session, selected, !k.dark);
        let note = if at >= foreign_at {
            s.store
                .as_ref()
                .and_then(|store| {
                    let workspace = store
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.id == session.workspace_id)?;
                    store
                        .projects
                        .iter()
                        .find(|project| project.id == workspace.project_id)
                        .map(|project| project.name.clone())
                        .or_else(|| workspace.branch.clone())
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        let mut item = row()
            .key(format!("tab-switcher-{at}"))
            .role("option")
            .label(session.title.clone())
            .focus_selected(selected)
            .w_full()
            .h(30.0)
            .px(8.0)
            .gap(8.0)
            .items_center()
            .radius(6.0)
            .fill(if selected {
                k.c.surface_hover
            } else {
                rune::Color::TRANSPARENT
            })
            .child(icon(glyph, 14.0).color(k.c.icon_muted))
            .child(
                text(session.title.clone())
                    .font(if selected {
                        k.ty.small_medium
                    } else {
                        k.ty.small
                    })
                    .color(if selected {
                        k.c.text_base
                    } else {
                        k.c.text_muted
                    })
                    .grow(1.0)
                    .min_w(0.0)
                    .truncate(),
            );
        if !note.is_empty() {
            item = item.child(
                text(note)
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint)
                    .truncate(),
            );
        }
        list = list.child(item);
    }
    card = card
        .child(
            text("Switch Tab")
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_faint)
                .px(8.0)
                .pb(4.0),
        )
        .child(list)
        .child(
            text("hold ctrl · tab steps · release commits · esc cancels")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .px(8.0)
                .pt(6.0),
        );
    layer.push(
        el().key("tab-switcher-layer")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .pt(72.0)
            .items_center()
            .child(card),
    );
}

/// the original client's empty centre (the equivalent): what can be started from here.
fn empty_center(s: &AppState, k: &Kit) -> El {
    let has_projects = s
        .store
        .as_ref()
        .is_some_and(|store| !store.projects.is_empty());
    let entry =
        |key: &'static str, label: &'static str, note: &'static str, run: fn(&mut AppState)| {
            row()
                .key(key)
                .role("button")
                .label(label)
                .w(360.0)
                .px(12.0)
                .py(8.0)
                .gap(12.0)
                .radius(6.0)
                .items_center()
                .on_press(run)
                .child(
                    text(label)
                        .font(k.ty.small_medium)
                        .color(k.c.text_base)
                        .w(120.0),
                )
                .child(
                    text(note)
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint)
                        .grow(1.0),
                )
        };
    let add = entry(
        "empty-add-project",
        "Add a project",
        "Point Forge Node at a repository to work in",
        |s| {
            s.show_mode(CenterMode::Projects);
        },
    );
    let mut view = col()
        .key("center-placeholder")
        .w_full()
        .h_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(6.0)
        .center()
        .child(
            text("No sessions open")
                .font(k.ty.body_medium)
                .color(k.c.text_base),
        )
        .child(
            text(if has_projects {
                "Everything here is closed. Start one to get back to work."
            } else {
                "Nothing is set up yet."
            })
            .font(k.ty.small)
            .color(k.c.text_faint)
            .pb(10.0),
        );
    if !has_projects {
        return view.child(add);
    }
    view = view
        .child(entry(
            "empty-new-terminal",
            "New terminal",
            "A shell in the current checkout",
            |s| s.request_create(),
        ))
        .child(entry(
            "empty-new-agent",
            "New agent",
            "Start a coding agent in a session",
            |s| {
                s.open_launch_palette();
            },
        ))
        .child(entry(
            "empty-new-worktree",
            "New worktree",
            "Checkout a branch in a new folder",
            |s| {
                s.show_mode(CenterMode::Projects);
            },
        ))
        .child(entry(
            "empty-add-project",
            "Add a project",
            "Work in another repository",
            |s| {
                s.show_mode(CenterMode::Projects);
            },
        ));
    view
}

/// The rename prompt, as a dialog layer.
fn rename_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some((_, edit)) = &s.rename else {
        return;
    };
    let field = rune_kit::TextInput::new(edit)
        .label("Session name")
        .on_edit(|s: &mut AppState| match s.rename.as_mut() {
            Some((_, edit)) => edit,
            None => &mut s.scratch_edit,
        })
        .on_key(|s: &mut AppState, ev, _| match ev.key {
            rune::Key::Enter => {
                s.finish_rename();
                true
            }
            rune::Key::Escape => {
                s.rename = None;
                true
            }
            _ => false,
        })
        .build(k)
        .key("rename-input");
    rune_kit::Dialog::new("rename", "Rename session")
        .open(true)
        .fit()
        .width(420.0)
        .click_outside(true)
        .on_close(|s: &mut AppState| s.rename = None)
        .child(field)
        .build(k, layer);
}

/// the original client's confirmation before a close stops a live process.
fn close_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(session) = s.confirm_close else {
        return;
    };
    let title = s
        .rows
        .iter()
        .find(|row| row.id == session)
        .map_or_else(|| "This session".to_string(), |row| row.title.clone());
    let cancel = Button::new("Cancel")
        .variant(ButtonVariant::Ghost)
        .on_press(|s: &mut AppState| s.confirm_close = None)
        .build(k)
        .key("close-cancel");
    let close = Button::new("Close session")
        .variant(ButtonVariant::Danger)
        .on_press(|s: &mut AppState| s.confirm_close_session())
        .build(k)
        .key("close-confirm");
    rune_kit::Dialog::new("close-session", format!("{title} is still running."))
        .description("Closing it stops the process. Anything it has not written is lost.")
        .open(true)
        .fit()
        .width(420.0)
        .on_close(|s: &mut AppState| s.confirm_close = None)
        .child(
            row()
                .gap(8.0)
                .child(el().grow(1.0))
                .child(cancel)
                .child(close),
        )
        .build(k, layer);
}
