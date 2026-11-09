//! The integrated shell: `SplitView` + `Sidebar` + `Toolbar`, a session tab
//! strip, and the active session in the center: a terminal drawn by the
//! terminal compositor, or an editor drawn from the daemon's windows of lines.

use std::time::Instant;

use rune::{Cx, El, col, effect, el, icon, row, text};
use rune_kit::shell::{SidebarRow, SidebarToggle, SplitView, Toolbar};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::{AppState, CommandStatus, LinkStatus, SessionRow, SessionStatus};
use crate::terminal::input::{Pointer, Press};
use crate::terminal::renderer::{CELL_PT, MARK_BASE};
use crate::workbench::CenterMode;

pub fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    s.metrics.note_ui_thread();
    let started = Instant::now();
    let mut k = crate::theme::kit(s.theme_base(cx.system_dark()), cx.system_dark());
    k.ty = crate::settings::prefs::scale_type(k.ty, s.chrome_font_size());
    let main = col()
        .key("main-pane")
        .w_full()
        .h_full()
        .min_h(0.0)
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
    let mut overlays = Vec::new();
    if s.palette.open {
        palette(s, &k, &mut overlays);
    }
    crate::projects::dialogs(s, &k, &mut overlays);
    crate::sync::dialog(s, &k, &mut overlays);
    rename_dialog(s, &k, &mut overlays);
    crate::pr_review::dialog(s, &k, &mut overlays);
    close_dialog(s, &k, &mut overlays);
    crate::history::delete_dialog(s, &k, &mut overlays);
    crate::settings::dialogs(s, &k, &mut overlays);
    let root = rune_kit::root(root, overlays);
    s.view_ms
        .set(Some(started.elapsed().as_secs_f64() * 1000.0));
    if let Some(switch) = s.switch_started.take() {
        s.switch_ms
            .set(Some(switch.elapsed().as_secs_f64() * 1000.0));
    }
    root
}

/// Tauri's card strip: the checkout's tabs, then its actions; the band the
/// window drags by.
fn toolbar(s: &AppState, k: &Kit) -> Toolbar {
    let mut bar = Toolbar::new().leading(crate::chrome::strip(s, k));
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

/// Tauri's sidebar: the view switcher, the chosen view, and the footer. Each
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
    let glyph = crate::glyphs::for_session(session.kind, session.provider.as_deref());
    let mut item = SidebarRow::new(session.title.as_str())
        .select_only()
        .height(30.0)
        .avatar(icon(glyph, 14.0).color(if selected {
            k.c.icon_base
        } else {
            k.c.icon_muted
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
        item = item.status(row_status(status));
    }
    if let Some(ended) = &session.ended {
        item = item.detail(ended.as_str());
    }
    item.build(k).key(format!("session-row-{index}"))
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

fn center(s: &AppState, k: &Kit) -> El {
    let has_terminal = s.active_row().and_then(|row| row.terminal_id).is_some();
    // A session of another checkout is not this window's to show.
    let in_checkout = s
        .active
        .is_some_and(|active| s.tab_rows().iter().any(|row| row.id == active));
    let body = if s.workbench.mode == CenterMode::Changes {
        crate::workbench::view::changes(s, k)
    } else if s.workbench.mode == CenterMode::Files {
        crate::workbench::view::preview(s, k)
    } else if s.workbench.mode == CenterMode::Settings {
        crate::settings::page(s, k)
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
    col()
        .key("terminal-surface")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(header)
        .child(
            // Tauri's terminalPadX / terminalPadY, painted in the terminal's
            // own ground; the pane inside keeps its coordinates.
            col()
                .key("terminal-inset")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .px(24.0)
                .py(16.0)
                .fill(terminal_ground(s, k))
                .child(terminal_pane(s, k)),
        )
}

/// The active theme's terminal background, as a Rune colour.
fn terminal_ground(s: &AppState, k: &Kit) -> rune::Color {
    let packed = crate::terminal::palette::Palette::for_base(s.theme_base(k.dark)).bg;
    let channel = |shift: u32| (packed >> shift) & 0xff;
    let srgb = format!("#{:02x}{:02x}{:02x}", channel(0), channel(8), channel(16));
    rune::color::hex(&srgb).unwrap_or(k.c.bg_base)
}

/// Tauri's `SessionHeader` crumbs: project / checkout / session.
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

fn row_status(status: SessionStatus) -> rune_kit::shell::RowStatus {
    match status {
        SessionStatus::Busy => rune_kit::shell::RowStatus::Busy,
        SessionStatus::Attention => rune_kit::shell::RowStatus::Attention,
    }
}

fn palette(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let entries = s.palette.entries();
    let field = rune_kit::TextInput::new(&s.palette.edit)
        .placeholder(s.palette.placeholder())
        .label("Command palette")
        .on_edit(|s: &mut AppState| &mut s.palette.edit)
        .on_change(|s: &mut AppState, _| s.palette.selected = 0)
        .on_key(|s: &mut AppState, ev, _| s.palette_key(ev))
        .build(k)
        .key("palette-input");
    let mut list = col()
        .key("palette-list")
        .gap(2.0)
        .role("listbox")
        .label("Results");
    for (index, entry) in entries.iter().enumerate().take(30) {
        let selected = index == s.palette.selected;
        let chosen = entry.clone();
        let kind = match entry {
            crate::palette::Entry::Command { command, .. } => {
                crate::palette::chord(command).unwrap_or_default()
            }
            crate::palette::Entry::File(_) => "file",
        };
        list = list.child(
            row()
                .key(format!("palette-{index}"))
                .role("option")
                .label(entry.label().to_string())
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
                .on_press(move |s: &mut AppState| s.run_palette(chosen.clone()))
                .child(
                    text(entry.label().to_string())
                        .font(k.ty.small)
                        .color(k.c.text_base)
                        .grow(1.0)
                        .truncate(),
                )
                .child(text(kind).font(k.ty.text_xsmall).color(k.c.text_faint)),
        );
    }
    if entries.is_empty() {
        list = list.child(
            text(if s.palette.searching() {
                "Searching files…"
            } else {
                "Nothing matches."
            })
            .font(k.ty.small)
            .color(k.c.text_faint),
        );
    }
    rune_kit::Dialog::new(
        "palette",
        if s.palette.files_only {
            "Open File…"
        } else if s.palette.launch {
            "New Session"
        } else {
            "Command Palette"
        },
    )
    .open(true)
    .click_outside(true)
    .width(560.0)
    .on_close(|s: &mut AppState| s.palette.close())
    .child(field)
    .child(list)
    .build(k, layer);
}

/// Tauri's empty centre (`EmptyCenter.tsx`): what can be started from here.
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

/// Tauri's confirmation before a close stops a live process.
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
