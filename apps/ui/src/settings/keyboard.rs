//! Settings › Keyboard: the chords `keymap.rs` answers, read-only. Tauri
//! also rebinds them; this client's keymap is fixed for now.

use rune::{El, col, row, text};
use rune_kit::Kit;

use crate::keymap::BINDINGS;

pub fn section(k: &Kit) -> El {
    let header = row()
        .role("row")
        .w_full()
        .h(28.0)
        .px(8.0)
        .items_center()
        .child(
            text("Action")
                .role("columnheader")
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_faint)
                .grow(1.0),
        )
        .child(
            text("Context")
                .role("columnheader")
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_faint)
                .w(100.0),
        )
        .child(
            text("Chord")
                .role("columnheader")
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_faint)
                .w(120.0),
        );
    let mut table = col()
        .key("keyboard-table")
        .role("table")
        .label("Keyboard shortcuts")
        .w_full()
        .gap(1.0)
        .child(header);
    for (index, binding) in BINDINGS.iter().enumerate() {
        table = table.child(
            row()
                .key(format!("chord-{index}"))
                .role("row")
                .label(format!("{}: {}", binding.label, binding.chord))
                .w_full()
                .h(30.0)
                .px(8.0)
                .radius(5.0)
                .items_center()
                .fill(if index % 2 == 0 {
                    k.c.surface_hover
                } else {
                    rune::Color::TRANSPARENT
                })
                .child(
                    text(binding.label)
                        .role("cell")
                        .font(k.ty.small)
                        .color(k.c.text_base)
                        .grow(1.0),
                )
                .child(
                    text(binding.action.context())
                        .role("cell")
                        .font(k.ty.small)
                        .color(k.c.text_faint)
                        .w(100.0),
                )
                .child(
                    text(binding.chord)
                        .role("cell")
                        .font(k.ty.mono)
                        .color(k.c.text_muted)
                        .w(120.0),
                ),
        );
    }
    table
}
