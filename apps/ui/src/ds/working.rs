use rune::{Color, Cycle, El, el};

/// The pulse is sampled this often: a turning loop (rune-kit's `Loader`)
/// asks for every vsync, and one busy agent in the sidebar then repaints the
/// whole window at the display's refresh rate for as long as it works.
pub const WORKING_FPS: f32 = 12.0;
const PERIOD: f32 = 1.2;
const DOTS: usize = 3;
const REST: f32 = 0.3;
const STOPS: &[(f32, f32)] = &[(0.0, REST), (0.25, 1.0), (0.6, REST), (1.0, REST)];

/// An agent at work: three dots lit in turn on a bounded wall-clock loop.
/// Under Reduce Motion a caller draws a still glyph instead.
#[must_use]
pub fn working(size: f32, ink: Color) -> El {
    let dot = (size * 0.22).round().max(2.0);
    let gap = (size - dot * DOTS as f32) / (DOTS as f32 + 1.0);
    let mut mark = el()
        .size(size, size)
        .a11y_hidden()
        .motion_intent(rune::MotionIntent::PULSE);
    for i in 0..DOTS {
        let x = gap + i as f32 * (dot + gap);
        mark = mark.child(
            el().key(format!("working-{i}"))
                .absolute(x, (size - dot) / 2.0)
                .size(dot, dot)
                .radius(dot / 2.0)
                .fill(ink)
                .cycle(Cycle {
                    period: PERIOD,
                    phase: i as f32 / DOTS as f32 * 0.35,
                    stops: STOPS,
                    still: REST,
                    fps: WORKING_FPS,
                }),
        );
    }
    mark
}

#[cfg(test)]
mod tests {
    use rune::{Cx, Env, Runtime};

    use super::*;

    fn runtime(view: fn(&(), &mut Cx<()>) -> El) -> Runtime<()> {
        let mut rt = Runtime::new((), view, (80.0, 40.0), Env::new(true, 1.0));
        for _ in 0..30 {
            rt.frame(1.0 / 120.0);
        }
        rt
    }

    #[test]
    fn a_working_agent_lets_the_window_sleep_between_pulse_frames() {
        let mut rt = runtime(|_, _| working(14.0, rune::Color::WHITE));
        assert!(!rt.frame(1.0 / 120.0), "no frame is asked for at once");
        let wake = rt
            .wake_deadline()
            .expect("the pulse schedules its next frame")
            - rt.clock;
        assert!(
            wake > 2.0 / 120.0 && wake <= 1.0 / f64::from(WORKING_FPS) + 1e-6,
            "the next pulse frame is {wake}s away"
        );
    }

    #[test]
    fn the_kits_turning_loader_asks_for_every_frame() {
        let mut rt = runtime(|_, _| {
            let k = crate::theme::kit(crate::theme::default_base(true), true);
            rune_kit::Loader::new().size(14.0).build(&k)
        });
        assert!(rt.frame(1.0 / 120.0), "why session marks do not use it");
    }
}
