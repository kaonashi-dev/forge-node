//! The integrated shell: `SplitView` + `Sidebar` + `Toolbar`, a session tab
//! strip, and the active session in the center: a terminal drawn by the
//! terminal compositor, or an editor drawn from the daemon's windows of lines.

use std::time::Instant;

use rune::{Cx, El, col, effect, el, icon, row, text};
use rune_kit::shell::{SplitView, Toolbar};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, Loader};

use crate::app::{AppState, CommandStatus, LinkStatus, SessionRow, SessionStatus};
use crate::terminal::input::{Pointer, Press};
use crate::terminal::renderer::MARK_BASE;
use crate::workbench::CenterMode;

/// The focusable ancestor a sidebar or tab press walks up to; keys it hears
/// belong to whatever surface the window shows.
pub const FOCUS_PROXY_KEY: &str = "terminal-focus-proxy";

const PALETTE_ROW_H: f32 = 30.0;
const PALETTE_HINT_W: f32 = 90.0;

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

/// The mark of a session at work: `for_row` names it `LOADER`, a glyph that
/// cannot turn, so it is drawn as the kit's spinner unless motion is reduced.
#[must_use]
pub fn session_mark(
    s: &AppState,
    k: &Kit,
    glyph: &'static rune::Icon,
    size: f32,
    ink: rune::Color,
) -> El {
    if std::ptr::eq(glyph, &crate::glyphs::LOADER) && !s.reduce_motion() {
        Loader::new().size(size).color(ink).build(k).a11y_hidden()
    } else {
        icon(glyph, size).color(ink)
    }
}

/// The title row the traffic lights centre in; the kit's own band is taller.
pub const TITLE_H: f32 = crate::ds::CONTROL;
const TOGGLE_SIDE: f32 = 20.0;

/// The sidebar's toggle, beside the traffic lights in the title row whether
/// the sidebar is open or not, as in Rex; the kit's own would sit in its
/// taller band.
fn sidebar_toggle(s: &AppState, k: &Kit, x: f32) -> El {
    let open = s.sidebar.is_open();
    crate::ds::icon_button(
        if open {
            &crate::glyphs::PANEL_LEFT_CLOSE
        } else {
            &crate::glyphs::PANEL_LEFT_OPEN
        },
        if open { "Hide sidebar" } else { "Show sidebar" },
        crate::ds::IconButtonSize::Mini,
    )
    .shortcut("⌘B")
    .on_press(|s: &mut AppState| {
        let open = s.sidebar.is_open();
        s.sidebar.set_open(!open);
    })
    .build(k)
    .key("sidebar-toggle")
    .absolute(x, (TITLE_H - TOGGLE_SIDE) / 2.0)
}

/// The shell's kit: the card around the centre is flush with the window, so
/// its corners are square.
fn flush_kit(k: &Kit) -> Kit {
    let mut flush = k.clone();
    flush.radius.lg = 0.0;
    flush
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
    let inset = cx.titlebar_inset();
    let toggle_x = if inset > 0.0 {
        inset + crate::ds::SPACE_4
    } else {
        crate::ds::SPACE_4
    };
    s.title_clear
        .set(toggle_x + TOGGLE_SIDE + crate::ds::SPACE_4);
    // The kit's band is 48 points; Rex's title row is one control tall. With no
    // crumbs to show, the centre and the sidebar rise into the band so the
    // lights, the toggle and the pane headers share that one row.
    let lifted = crate::chrome::strip(s, &k).is_none() && !s.debug;
    let mut main = col()
        .key("main-pane")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(center(s, &k, &mut overlays));
    if lifted {
        main = main.mt(-rune_kit::shell::TOOLBAR_HEIGHT);
    }
    overlays.push(sidebar_toggle(s, &k, toggle_x));
    let shell = SplitView::new(main)
        .sidebar(
            &s.sidebar,
            |s: &mut AppState| &mut s.sidebar,
            sidebar(s, &k).mt(TITLE_H - rune_kit::shell::TOOLBAR_HEIGHT),
        )
        .toolbar(toolbar(s, &k))
        .titlebar_inset(cx.titlebar_inset())
        // Flush, as Rex: the card's own edge is the line between the
        // sidebar and the terminal, and no margin frames it.
        .frame_inset(0.0)
        .toolbar_in_panel()
        .sidebar_padding(metrics.sidebar_pad[0], metrics.sidebar_pad[1])
        .hide_sidebar_toggle()
        .reduced_motion(cx.is_reduced_motion() || s.reduce_motion())
        .build(&flush_kit(&k));
    // A focusable ancestor is the only public way to move focus on a sidebar
    // press in this Rune pin (`Runtime::set_focus` is crate-private). Sidebar
    // rows are select-only, so the press walks up to this proxy; the proxy's
    // key handler is the same terminal route as `AppSpec::on_key`.
    let root = col()
        .key(FOCUS_PROXY_KEY)
        .w_full()
        .h_full()
        .min_h(0.0)
        .focusable(true)
        .no_focus_ring()
        .on_key(|s: &mut AppState, ev, tx| s.terminal_key(ev, tx))
        .on_layout(|s: &mut AppState, rect| s.window.set([rect[2], rect[3]]))
        .child(shell);
    if s.palette.open {
        palette(s, &k, &mut overlays);
    }
    crate::projects::dialogs(s, &k, &mut overlays);
    rename_dialog(s, &k, &mut overlays);
    crate::pr_review::dialog(s, &k, &mut overlays);
    crate::spec_run::dialog(s, &k, &mut overlays);
    crate::pr_link::dialog(s, &k, &mut overlays);
    crate::agent_ops::dialogs(s, &k, &mut overlays);
    close_dialog(s, &k, &mut overlays);
    crate::history::delete_dialog(s, &k, &mut overlays);
    crate::settings::dialogs(s, &k, &mut overlays);
    crate::storage::dialogs(s, &k, &mut overlays);
    crate::menus::layer(s, &k, &mut overlays);
    crate::native_menus::dialog(s, &k, &mut overlays);
    tab_switcher(s, &k, &mut overlays);
    overlays.extend(crate::toasts::layer(s, &k));
    let root = rune_kit::root(root, overlays).on_paste_content(
        |s: &mut AppState, content: &rune::ClipboardContent, tx: &mut rune::Tx| {
            s.paste_content(content, tx);
        },
    );
    s.view_ms
        .set(Some(started.elapsed().as_secs_f64() * 1000.0));
    if let Some(switch) = s.switch_started.take() {
        s.switch_ms
            .set(Some(switch.elapsed().as_secs_f64() * 1000.0));
    }
    root
}

/// The card strip: the checkout's tabs, then its actions; the band the
/// window drags by.
fn toolbar(s: &AppState, k: &Kit) -> Toolbar {
    let mut bar = Toolbar::new();
    if let Some(crumbs) = crate::chrome::strip(s, k) {
        bar = bar.leading(crumbs);
    }
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
    bar
}

/// The sidebar: the view switcher, the chosen view, and the footer. Each
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
        .child(crate::chrome::jump_to(s, k))
        .child(crate::chrome::views(s, k))
        .child(
            col()
                .key("sidebar-panel")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                // Hard edges, so a page sliding out never draws over the switcher.
                .clip_fade(0.0, 0.0, 0.0, 0.0)
                .child(
                    col()
                        .key(format!("sidebar-page-{:?}", s.workbench.panel))
                        .w_full()
                        .h_full()
                        .min_h(0.0)
                        .transition(page_turn(s))
                        .offset(
                            s.page_swipe
                                .map_or(0.0, |dx| crate::pages::drag_offset(s.workbench.panel, dx)),
                            0.0,
                        )
                        .on_wheel(crate::pages::wheel)
                        .child(panel),
                ),
        )
        .child(status_row(s, k))
}

/// The sidebar's views swap like pages: the next view pushes in from the
/// side it sits on in the switcher. Reduce Motion keeps only a fade.
fn page_turn(s: &AppState) -> rune::Transition {
    if s.reduce_motion() {
        rune::Transition::opacity()
    } else if s.workbench.panel_back {
        rune::Transition::push(rune::Edge::Leading)
    } else {
        rune::Transition::push(rune::Edge::Trailing)
    }
}

/// A session under its checkout in Projects: select-only, so a press walks
/// up to the terminal's focus proxy and typing reaches the terminal.
pub fn session_row(s: &AppState, session: &SessionRow, index: usize, depth: u8, k: &Kit) -> El {
    use crate::ds::{self, Emphasis, Tone};
    let selected = s.active == Some(session.id) && s.workbench.mode == CenterMode::Session;
    let id = session.id;
    let glyph = crate::glyphs::for_row(session, selected, !k.dark);
    let roles = crate::theme::roles(k);
    let mut item = ds::Row::new(format!("session-row-{index}"), session.title.as_str())
        .indent(depth)
        .height(s.chrome_metrics().row)
        .emphasis(Emphasis::Quiet)
        .selected(selected)
        .leading(session_mark(
            s,
            k,
            glyph,
            ds::ICON_ROW,
            match session.status {
                _ if selected => k.c.icon_base,
                Some(SessionStatus::Attention) => roles.needs_you.solid,
                Some(SessionStatus::Busy) => k.c.icon_accent,
                None => k.c.icon_muted,
            },
        ));
    item = match (session.status, &session.ended) {
        (Some(SessionStatus::Attention), _) => item
            .meta("Needs you", Tone::NeedsYou)
            .label(format!("{}, waiting on you", session.title)),
        (Some(SessionStatus::Busy), _) => item.label(format!("{}, working", session.title)),
        (None, Some(ended)) => item.meta(ended.as_str(), Tone::Neutral),
        (None, None) => item,
    };
    item.build(k, &roles)
        .on_press(move |s: &mut AppState| {
            s.select(id);
            s.show_mode(CenterMode::Session);
        })
        .on_double_press(move |s: &mut AppState| {
            s.select(id);
            s.request_rename();
        })
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
        .gap(crate::ds::SPACE_4)
        .center_y()
        .child(el().size(8.0, 8.0).radius(k.radius.sm).fill(color))
        .child(
            text(s.status.label())
                .font(k.ty.small_medium)
                .color(k.c.text_muted),
        )
        .child(
            text(detail)
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .truncate()
                .min_w(0.0),
        )
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
    } else if s.workbench.mode == CenterMode::OpenPr {
        crate::open_pr::view(s, k)
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
    let column = |idx: usize| {
        col()
            .key(if idx == 0 {
                "terminal-column"
            } else {
                "terminal-split-extra"
            })
            .w(0.0)
            .min_w(0.0)
            .h_full()
            .transition(rune::Transition::none())
            .child(pane_header(s, k, idx))
            .child(pane_inset(s, k, idx).w_full().grow(1.0))
    };
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
            .child(column(0).grow(ratio))
            .child(split_handle(k))
            .child(column(1).grow(1.0 - ratio))
    } else {
        row()
            .key("terminal-split")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(column(0).grow(1.0))
    };
    col()
        .key("terminal-surface")
        .w_full()
        .h_full()
        .min_h(0.0)
        // A terminal is text, not a card: fading it in puts the effect slot in an
        // Rgba16Float layer, and the glyphs are unreadable while it fades.
        .transition(rune::Transition::none())
        .child(body)
}

/// The terminal's padding, painted in the terminal's own ground;
/// the pane inside keeps its coordinates.
fn pane_inset(s: &AppState, k: &Kit, idx: usize) -> El {
    col()
        .key(if idx == 0 {
            "terminal-inset"
        } else {
            "terminal-inset-split"
        })
        .min_h(0.0)
        .px(crate::ds::SPACE_5)
        .pb(crate::ds::SPACE_3)
        .fill(terminal_ground(s, k))
        .child(terminal_pane(s, k, idx))
}

/// Wide enough to grab; it draws a hairline.
const SPLIT_HANDLE_W: f32 = 5.0;

/// A column divider that stores a fraction: a
/// remembered width of one column would starve the other after a resize.
fn split_handle(k: &Kit) -> El {
    el().key("terminal-split-handle")
        .role("separator")
        .label("Resize the split")
        .w(SPLIT_HANDLE_W)
        .h_full()
        .center()
        .cursor(rune::Cursor::ResizeColumn)
        .child(el().w(1.0).h_full().fill(k.c.border_base))
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

/// A pane's title bar, as Rex draws one: the session's mark and title, and the
/// pane's own actions at the end — split beside the first, close the second
/// (its session keeps running in the sidebar).
fn pane_header(s: &AppState, k: &Kit, idx: usize) -> El {
    use crate::ds::{self, IconButtonSize};
    let session = s
        .pane_session(idx)
        .and_then(|session| s.rows.iter().find(|row| row.id == session));
    let title = session.map_or_else(|| "Terminal".to_string(), |row| row.title.clone());
    let focused = s.focused_pane() == idx;
    let mark = session.map_or_else(
        || icon(&crate::glyphs::TERMINAL, ds::ICON_ROW).color(k.c.icon_muted),
        |row| {
            session_mark(
                s,
                k,
                crate::glyphs::for_row(row, true, !k.dark),
                ds::ICON_ROW,
                k.c.icon_muted,
            )
        },
    );
    let action = if idx == 1 {
        ds::icon_button(
            &crate::glyphs::CLOSE,
            "Close this pane",
            IconButtonSize::Mini,
        )
        .on_press(|s: &mut AppState| s.join_panes())
        .build(k)
        .key("terminal-split-join")
    } else if s.pane_count() == 1 {
        ds::icon_button(&crate::glyphs::SPLIT, "Split right", IconButtonSize::Mini)
            .shortcut("⌘D")
            .on_press(AppState::split_pane)
            .build(k)
            .key("terminal-split-open")
    } else {
        el()
    };
    row()
        .key(if idx == 0 {
            "terminal-header"
        } else {
            "terminal-split-header"
        })
        .w_full()
        .h(TITLE_H)
        .pl(if idx == 0 && !s.sidebar.is_open() {
            s.title_clear.get()
        } else {
            ds::SPACE_5
        })
        .pr(ds::SPACE_2)
        .gap(ds::SPACE_3)
        .center_y()
        .fill(terminal_ground(s, k))
        .window_drag()
        .child(mark)
        .child(
            text(title)
                .role("heading")
                .font(k.ty.ui)
                .color(if focused {
                    k.c.text_base
                } else {
                    k.c.text_muted
                })
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(action)
}

/// The active theme's terminal background, as a Rune colour.
fn terminal_ground(s: &AppState, k: &Kit) -> rune::Color {
    let packed = crate::terminal::palette::Palette::bg_for_base(s.theme_base(k.dark));
    let channel = |shift: u32| (packed >> shift) & 0xff;
    let srgb = format!("#{:02x}{:02x}{:02x}", channel(0), channel(8), channel(16));
    rune::color::hex(&srgb).unwrap_or(k.c.bg_base)
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
        .no_focus_ring()
        .on_key(move |s: &mut AppState, ev, tx| {
            if !ev.mods.cmd {
                s.focus_pane(idx);
            }
            s.terminal_key(ev, tx)
        })
        .on_wheel(move |s: &mut AppState, ev, tx| {
            crate::pages::sidebar_swipe(s, &ev) || s.terminal_wheel(idx, &ev, tx)
        })
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
                .radius(k.radius.sm)
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
    crate::ds::section_header(k, title, None, None)
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
        .gap(crate::ds::SPACE_4)
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
        .gap(crate::ds::SPACE_4)
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
    let entries = s.palette.entries_cached();
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
        .on_key(|s: &mut AppState, ev, tx| s.palette_key(ev, tx))
        .build(k)
        .key("palette-input")
        // `width(None)` is the row's whole width; beside the scope chip it
        // must take only what is left.
        .grow(1.0)
        .min_w(0.0);
    let mut heading = row()
        .key("palette-scope")
        .w_full()
        .gap(crate::ds::SPACE_4)
        .items_center();
    if let Some((prefix, label)) = s.palette.chip() {
        heading = heading.child(
            row()
                .px(7.0)
                .py(2.0)
                .radius(k.radius.sm)
                .fill(k.c.surface_hover)
                .gap(crate::ds::SPACE_2)
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
    let list_h = (s.modal_body_max_h() - 72.0).max(PALETTE_ROW_H);
    let list = crate::modal::ModalList::new("palette", entries.len(), PALETTE_ROW_H, list_h)
        .selected(Some(s.palette.selected))
        .role("listbox")
        .label("Results")
        .row({
            let entries = std::rc::Rc::clone(&entries);
            let k = k.clone();
            let selected = s.palette.selected;
            move |s: &AppState, index| {
                entries.get(index).map_or_else(el, |entry| {
                    palette_row(s, &k, index, entry, index == selected)
                })
            }
        })
        .build();
    let mut results = col().key("palette-results").w_full().child(list);
    if entries.is_empty() {
        let mut empty = row()
            .key("palette-empty")
            .gap(crate::ds::SPACE_4)
            .items_center();
        if s.palette.searching() {
            empty = empty.child(busy(s, k));
        }
        results = results.child(
            empty.child(
                text(s.palette.empty_message())
                    .font(k.ty.small)
                    .color(k.c.text_faint),
            ),
        );
    }
    let mut body = col()
        .key("palette-body")
        .w_full()
        .gap(crate::ds::SPACE_4)
        .child(heading)
        .child(results);
    if let Some(error) = &s.palette.error {
        body = body.child(
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
    crate::modal::Modal::new("palette", title)
        .width(crate::modal::WIDTH_STANDARD)
        .anchor_top()
        .click_outside(true)
        .on_close(|s: &mut AppState| s.palette.close())
        .child(body)
        .build(s, k, layer);
}

fn palette_row(
    s: &AppState,
    k: &Kit,
    index: usize,
    entry: &crate::palette::Entry,
    selected: bool,
) -> El {
    let chosen = entry.clone();
    let mut row = row()
        .key(format!("palette-{index}"))
        .role("option")
        .label(entry.label().to_string())
        .focus_selected(selected)
        .w_full()
        .h(PALETTE_ROW_H)
        .px(10.0)
        .gap(crate::ds::SPACE_4)
        .items_center()
        .radius(k.radius.md)
        .fill(if selected {
            k.c.surface_hover
        } else {
            rune::Color::TRANSPARENT
        })
        .on_press(move |s: &mut AppState| s.run_palette(chosen.clone()));
    let (mark, hint, note) = match entry {
        crate::palette::Entry::Command { command, .. } => (
            Some(command.icon()),
            s.palette.chord(command).unwrap_or_default().to_string(),
            None,
        ),
        crate::palette::Entry::File(path) => (
            Some(crate::file_icons::for_path(path).icon(!k.dark)),
            "file".to_string(),
            None,
        ),
        crate::palette::Entry::Session {
            id,
            agent,
            kind,
            waiting,
            note,
            ..
        } => {
            // An open file is known by its language mark, as in the sidebar;
            // the plain code sheet would make every editor look alike.
            let file = s
                .rows
                .iter()
                .find(|row| row.id == *id)
                .and_then(|row| row.file)
                .map(|file| file.icon(!k.dark));
            let glyph = if *waiting {
                &crate::glyphs::NEEDS_YOU
            } else {
                file.unwrap_or_else(|| crate::glyphs::for_session(*kind, agent.as_deref()))
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
        row = row.child(
            rune::row().w(PALETTE_HINT_W).justify_end().child(
                text(hint)
                    .key(format!("palette-hint-{index}"))
                    .font(k.ty.mono)
                    .color(k.c.text_faint)
                    .truncate()
                    .min_w(0.0),
            ),
        );
    }
    row
}

/// Hold-Control tab list. It never takes focus: the gesture is keyboard-only,
/// Tab steps it, and releasing Control commits the highlighted row.
fn tab_switcher(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    const ROW_H: f32 = 30.0;
    let Some((targets, index, foreign_at)) = s.tabs.shown_gesture() else {
        return;
    };
    let ids: Vec<domain::SessionId> = targets.to_vec();
    let rows: Vec<(String, String, &'static rune::Icon)> = ids
        .iter()
        .enumerate()
        .map(|(at, id)| {
            let session = s.rows.iter().find(|row| row.id == *id);
            let title = session.map_or_else(|| id.to_string(), |row| row.title.clone());
            let glyph = session.map_or(&crate::glyphs::TERMINAL, |row| {
                crate::glyphs::for_row(row, at == index, !k.dark)
            });
            let note = if at >= foreign_at {
                session
                    .and_then(|session| {
                        let store = s.store.as_ref()?;
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
            (title, note, glyph)
        })
        .collect();
    let mut list = col()
        .key("tab-switcher-list")
        .role("listbox")
        .label("Switch Tab")
        .gap(crate::ds::SPACE_1)
        .reveal_selected(true);
    for (at, (title, note, glyph)) in rows.iter().enumerate() {
        let selected = at == index;
        let mut item = row()
            .key(format!("tab-switcher-{at}"))
            .role("option")
            .label(title.clone())
            .focus_selected(selected)
            .w_full()
            .h(ROW_H)
            .px(8.0)
            .gap(crate::ds::SPACE_4)
            .items_center()
            .radius(k.radius.md)
            .fill(if selected {
                k.c.surface_hover
            } else {
                rune::Color::TRANSPARENT
            })
            .child(icon(glyph, 14.0).color(k.c.icon_muted))
            .child(
                text(title.clone())
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
                text(note.clone())
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint)
                    .truncate(),
            );
        }
        list = list.child(item);
    }
    let list = col()
        .key("tab-switcher-scroll")
        .w_full()
        .h((rows.len() as f32 * ROW_H).min(s.modal_body_max_h().min(360.0)))
        .shrink()
        .min_h(if rows.is_empty() { 0.0 } else { ROW_H })
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .scroll_edge_fade(12.0)
        .scroll_margin(12.0)
        .child(list);
    let card = col()
        .gap(crate::ds::SPACE_1)
        .w(420.0)
        .padding(10.0)
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_base)
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
            .child(k.elevate(card, &k.elevation.overlay)),
    );
}

/// The empty centre: what can be started from here.
fn empty_center(s: &AppState, k: &Kit) -> El {
    use crate::ds;
    const ENTRY_W: f32 = 360.0;
    const ENTRY_LABEL_W: f32 = 120.0;
    let has_projects = s
        .store
        .as_ref()
        .is_some_and(|store| !store.projects.is_empty());
    let entry =
        |key: &'static str, label: &'static str, note: &'static str, run: fn(&mut AppState)| {
            k.interactive(
                row()
                    .key(key)
                    .role("button")
                    .label(label)
                    .w(ENTRY_W)
                    .h(ds::CONTROL_FIELD)
                    .px(ds::SPACE_5)
                    .gap(ds::SPACE_5)
                    .radius(k.radius.md)
                    .items_center()
                    .on_press(run),
            )
            .child(
                text(label)
                    .font(k.ty.ui_medium)
                    .color(k.c.text_base)
                    .w(ENTRY_LABEL_W),
            )
            .child(
                text(note)
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint)
                    .grow(1.0),
            )
        };
    let add = |note| {
        entry("empty-add-project", "Add a project", note, |s| {
            s.show_mode(CenterMode::Projects);
        })
    };
    let empty = if has_projects {
        ds::Empty::new("empty-center", "No sessions open")
            .why("Everything here is closed. Start one to get back to work.")
            .action(entry(
                "empty-new-terminal",
                "New terminal",
                "A shell in the current checkout",
                |s| s.request_create(),
            ))
            .action(entry(
                "empty-new-agent",
                "New agent",
                "Start a coding agent in a session",
                |s| {
                    s.open_launch_palette();
                },
            ))
            .action(entry(
                "empty-new-worktree",
                "New worktree",
                "Checkout a branch in a new folder",
                |s| {
                    s.show_mode(CenterMode::Projects);
                },
            ))
            .action(add("Work in another repository"))
    } else {
        ds::Empty::new("empty-center", "No projects yet")
            .why("Point Forge Node at a repository to work in.")
            .action(add("Choose a folder with a git repository"))
    };
    col()
        .key("center-placeholder")
        .w_full()
        .h_full()
        .grow(1.0)
        .min_h(0.0)
        .center()
        .child(empty.build(k))
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
    crate::modal::Modal::new("rename", "Rename session")
        .width(crate::modal::WIDTH_PROMPT)
        .click_outside(true)
        .on_close(|s: &mut AppState| s.rename = None)
        .child(field)
        .build(s, k, layer);
}

/// The confirmation before a close stops a live process.
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
    crate::modal::Modal::new("close-session", format!("{title} is still running."))
        .description("Closing it stops the process. Anything it has not written is lost.")
        .width(crate::modal::WIDTH_PROMPT)
        .on_close(|s: &mut AppState| s.confirm_close = None)
        .child(
            row()
                .gap(crate::ds::SPACE_4)
                .child(el().grow(1.0))
                .child(cancel)
                .child(close),
        )
        .build(s, k, layer);
}
