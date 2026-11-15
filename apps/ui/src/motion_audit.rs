//! The window's motion gate: each cell drives the fixture app headless on the
//! virtual clock and `rune::motion_gate` lists what a person would see go
//! wrong (a pop, a jump, a blink). `MOTION_GATE=<cell>` runs one cell.

use std::sync::Arc;

use rune::motion_gate::{self, AllowRow, Job, Recording, Run};
use rune::{Env, Runtime};

use crate::app::AppState;
use crate::connection::{Metrics, fixture_store};
use crate::terminal::SharedTerminal;
use crate::workbench::{CenterMode, Panel};

const HZ: f64 = 60.0;

const ALLOW: &[AllowRow] = &[
    AllowRow::new(
        "jerk",
        "sidebar-page-",
        "switch-view|settings|changes-view",
        "a page pushed the sidebar's whole width starts from rest",
    )
    .max(3),
    AllowRow::new(
        "jerk",
        "split/",
        "toggle-sidebar",
        "rune-kit's SplitView opening a 300 pt pane from rest",
    )
    .max(6),
];

fn app_in(panel: Panel) -> Run<AppState> {
    let store = fixture_store(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
    )
    .expect("fixture");
    let mut state = AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
    state.generation = 1;
    state.debug = false;
    state.show_panel(panel);
    let mut rt = Runtime::new(
        state,
        crate::ui::view,
        (1180.0, 760.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.poll = Some(crate::app::poll);
    let mut run = Run::new(rt, HZ);
    run.idle(1.0);
    run.step();
    run
}

fn app() -> Run<AppState> {
    app_in(Panel::Projects)
}

fn there_and_back(first: &str, second: &str) -> Recording {
    let mut r = app();
    r.act(first, |rt| Run::tap(rt, first));
    r.play(1.0);
    r.act(second, |rt| Run::tap(rt, second));
    r.play(1.0);
    r.rec
}

fn edit_store(rt: &mut Runtime<AppState>, f: impl FnOnce(&mut client::Store)) {
    rt.update(|s, _| {
        let mut store = s.store.clone().expect("a store");
        f(&mut store);
        s.set_store(store);
    });
}

fn first_agent(store: &mut client::Store) -> &mut domain::Session {
    store
        .sessions
        .iter_mut()
        .find(|session| session.agent_provider_id.is_some() && session.state.is_active())
        .expect("a live agent in the fixture")
}

/// Data, not input: an agent stops to ask, then goes back to work.
fn agent_waits(panel: Panel) -> Recording {
    let mut r = app_in(panel);
    edit_store(&mut r.rt, |store| {
        first_agent(store).activity.state = domain::ActivityState::Waiting;
    });
    r.play(1.0);
    if panel == Panel::Projects {
        assert!(r.rt.find("needs-you").is_some(), "the agent is pinned");
    }
    edit_store(&mut r.rt, |store| {
        first_agent(store).activity.state = domain::ActivityState::Working;
    });
    r.play(1.0);
    r.rec
}

fn session_added_and_removed() -> Recording {
    let mut r = app();
    let fresh: domain::SessionId = "00000000-0000-7000-8000-00000000ffff"
        .parse()
        .expect("an id");
    edit_store(&mut r.rt, |store| {
        let mut session = store.sessions[0].clone();
        session.id = fresh;
        session.created_at = domain::Timestamp::now();
        store.sessions.push(session);
    });
    r.play(1.0);
    edit_store(&mut r.rt, |store| {
        store.sessions.retain(|session| session.id != fresh);
    });
    r.play(1.0);
    r.rec
}

fn overlay(open: fn(&mut AppState), close: fn(&mut AppState)) -> Recording {
    let mut r = app();
    r.act("open", |rt| rt.update(|s, _| open(s)));
    r.play(1.0);
    r.act("close", |rt| rt.update(|s, _| close(s)));
    r.play(1.0);
    r.rec
}

fn ctrl_tab() -> Recording {
    let tab = rune::KeyEvent {
        key: rune::Key::Tab,
        mods: rune::Mods {
            ctrl: true,
            ..rune::Mods::default()
        },
    };
    let mut r = app();
    r.act("hold ctrl-tab", |rt| {
        rt.update(|s, _| {
            s.global_key(&tab);
            s.global_key(&tab);
        });
    });
    r.play(1.0);
    r.act("release", |rt| {
        rt.update(|s, _| {
            let _ = s.tabs.commit_gesture();
        });
    });
    r.play(1.0);
    r.rec
}

fn toasts() -> Recording {
    let mut r = app();
    r.act("success", |rt| {
        rt.update(|s, _| s.toasts.success("Committed", None));
    });
    r.play(1.0);
    r.act("failure", |rt| {
        rt.update(|s, _| s.toasts.danger("Push failed", "rejected"));
    });
    r.play(1.0);
    r.act("dismiss", |rt| {
        rt.update(|s, _| {
            let ids: Vec<u64> = s.toasts.visible().iter().map(|t| t.id).collect();
            for id in ids {
                s.toasts.dismiss(id);
            }
        });
    });
    r.play(1.0);
    r.rec
}

fn cells() -> Vec<(String, Job)> {
    vec![
        (
            "switch-view".into(),
            Box::new(|| there_and_back("view-git", "view-projects")) as Job,
        ),
        (
            "select-session".into(),
            Box::new(|| there_and_back("session-row-2", "session-row-5")) as Job,
        ),
        (
            "toggle-sidebar".into(),
            Box::new(|| there_and_back("sidebar-toggle", "sidebar-toggle")) as Job,
        ),
        (
            "settings".into(),
            Box::new(|| there_and_back("open-settings", "open-settings")) as Job,
        ),
        (
            "palette".into(),
            Box::new(|| {
                overlay(
                    |s| s.open_palette(crate::palette::Scope::Everything),
                    |s| s.palette.open = false,
                )
            }) as Job,
        ),
        (
            "changes-view".into(),
            Box::new(|| {
                overlay(
                    |s| s.show_mode(CenterMode::Changes),
                    |s| s.show_mode(CenterMode::Session),
                )
            }) as Job,
        ),
        (
            "rename".into(),
            Box::new(|| overlay(AppState::request_rename, |s| s.rename = None)) as Job,
        ),
        ("ctrl-tab".into(), Box::new(ctrl_tab) as Job),
        ("toasts".into(), Box::new(toasts) as Job),
        (
            "agent-waits/projects".into(),
            Box::new(|| agent_waits(Panel::Projects)) as Job,
        ),
        (
            "agent-waits/history".into(),
            Box::new(|| agent_waits(Panel::History)) as Job,
        ),
        (
            "session-added-removed".into(),
            Box::new(session_added_and_removed) as Job,
        ),
    ]
}

#[test]
fn the_window_moves_without_pops_jumps_or_blinks() {
    motion_gate::gate_rows(cells(), ALLOW);
}
