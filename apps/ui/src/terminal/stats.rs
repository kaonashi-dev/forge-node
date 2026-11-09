use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Stats {
    pub tick_us: Vec<f64>,
    pub update_us: Vec<f64>,
    pub build_us: Vec<f64>,
    pub encode_us: Vec<f64>,
    pub total_us: Vec<f64>,
    pub frames: u64,
    pub draw_calls: u64,
    pub rows_uploaded: u64,
    pub instances_drawn: u64,
    pub glyph_misses: u64,
    pub build_allocs: u64,
    pub build_allocs_max: u64,
    pub hook_allocs: u64,
    pub frames_with_misses: u64,
    pub steady_build_allocs: u64,
    pub encode_allocs: u64,
    pub shapes: u64,
    pub rasters: u64,
    pub notdef_runs: u64,
    pub fallback_faces: usize,
    pub shape_us: f64,
    pub shape_us_max: f64,
    pub fallback_us: f64,
    pub steady_encode_allocs: u64,
}

pub type Shared = Arc<Mutex<Stats>>;

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

impl Stats {
    pub fn with_capacity(n: usize) -> Self {
        Stats {
            tick_us: Vec::with_capacity(n * 4),
            update_us: Vec::with_capacity(n),
            build_us: Vec::with_capacity(n),
            encode_us: Vec::with_capacity(n),
            total_us: Vec::with_capacity(n),
            ..Default::default()
        }
    }

    pub fn report(&mut self) -> String {
        let mut out = String::new();
        let rows: [(&str, &mut Vec<f64>); 5] = [
            ("synthetic tick (poll)", &mut self.tick_us),
            ("rune update (poll+view+layout)", &mut self.update_us),
            ("terminal build (diff+instances+upload)", &mut self.build_us),
            ("terminal encode (pass+draw)", &mut self.encode_us),
            ("TOTAL cpu/frame", &mut self.total_us),
        ];
        out.push_str(&format!(
            "frames={} draw_calls={} rows_uploaded={} instances_per_draw={} glyph_misses={}\n",
            self.frames,
            self.draw_calls,
            self.rows_uploaded,
            self.instances_drawn
                .checked_div(self.draw_calls)
                .unwrap_or(0),
            self.glyph_misses
        ));
        out.push_str(&format!(
            "allocations: cell encoding total={} (frames without new glyphs: {}), build incl. Rune write_buffer total={} (steady {}), build max/frame={}, draw hook total={}, frames with new glyphs={}\n",
            self.encode_allocs, self.steady_encode_allocs, self.build_allocs, self.steady_build_allocs, self.build_allocs_max, self.hook_allocs, self.frames_with_misses
        ));
        out.push_str(&format!(
            "shaping (one-time, first sight): {} graphemes shaped, {} glyphs rasterized, {:.3} ms total, {:.3} ms worst grapheme; fallback discovery {:.3} ms, {} fallback faces, {} runs with a missing glyph\n",
            self.shapes, self.rasters, self.shape_us / 1000.0, self.shape_us_max / 1000.0, self.fallback_us / 1000.0, self.fallback_faces, self.notdef_runs
        ));
        for (name, v) in rows {
            let n = v.len();
            let p50 = percentile(v, 0.5);
            let p95 = percentile(v, 0.95);
            let max = v.last().copied().unwrap_or(0.0);
            out.push_str(&format!(
                "{name:<42} n={n:<6} p50={:>7.3} ms  p95={:>7.3} ms  max={:>7.3} ms\n",
                p50 / 1000.0,
                p95 / 1000.0,
                max / 1000.0
            ));
        }
        out
    }
}
