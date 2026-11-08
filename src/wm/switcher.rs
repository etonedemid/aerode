//! Alt+Tab window switcher (Windows 7 style: glass panel, icons, title above).
use super::Wm;

const CELL: i32 = 110;
const PAD: i32 = 18;
const TITLE_H: i32 = 34;

pub struct Switcher {
    wins: Vec<u32>,
    sel: usize,
    pub w: i32,
    pub h: i32,
    pub pos: (i32, i32),
    /// the picture, redrawn on every step
    pub buf: Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>,
}

impl Switcher {
    pub fn new(wm: &Wm, wins: Vec<u32>) -> Option<Switcher> {
        let n = wins.len() as i32;
        let mon = wm.monitors().get(wm.pointer_monitor()).copied().unwrap_or_default();
        let per_row = ((mon.w - 120) / CELL).clamp(1, 12);
        let rows = (n + per_row - 1) / per_row;
        let w = per_row.min(n) * CELL + 2 * PAD;
        let h = rows * CELL + 2 * PAD + TITLE_H;
        let pos = (mon.x + (mon.w - w) / 2, mon.y + (mon.h - h) / 2);
        Some(Switcher { wins, sel: 0, w, h, pos, buf: None })
    }

    pub fn step(&mut self, fwd: bool) {
        let n = self.wins.len();
        if n == 0 { return; }
        self.sel = if fwd { (self.sel + 1) % n } else { (self.sel + n - 1) % n };
    }

    pub fn selected(&self) -> Option<u32> { self.wins.get(self.sel).copied() }

    pub fn draw(&mut self, wm: &Wm) {
        let Ok(mut s) = cairo::ImageSurface::create(cairo::Format::ARgb32, self.w, self.h) else { return };
        {
            let cr = cairo::Context::new(&s).unwrap();
            let (w, h) = (self.w as f64, self.h as f64);
            // glass panel like the frame (active glass)
            crate::glass::rounded(&cr, 0.5, 0.5, w - 1.0, h - 1.0, 7.0);
            cr.save().ok();
            cr.clip_preserve();
            wm.glass.fill(&cr, self.pos.0 as f64, self.pos.1 as f64, true, false, true);
            cr.restore().ok();
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.7);
            cr.set_line_width(1.0);
            cr.stroke().ok();
            crate::glass::rounded(&cr, 1.5, 1.5, w - 3.0, h - 3.0, 6.0);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.45);
            cr.stroke().ok();
            let g = cairo::LinearGradient::new(0.0, 0.0, 0.0, h * 0.5);
            g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.25);
            g.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
            cr.set_source(&g).ok();
            crate::glass::rounded(&cr, 2.0, 2.0, w - 4.0, h * 0.5, 6.0);
            cr.fill().ok();
            // title of the selected window
            if let Some(c) = self.selected().and_then(|w| wm.wins.get(&w)) {
                let layout = pangocairo::functions::create_layout(&cr);
                let fd = pangocairo::pango::FontDescription::from_string("Segoe UI 11");
                layout.set_font_description(Some(&fd));
                layout.set_text(&c.title);
                layout.set_width(((w - 40.0) * pangocairo::pango::SCALE as f64) as i32);
                layout.set_ellipsize(pangocairo::pango::EllipsizeMode::End);
                layout.set_alignment(pangocairo::pango::Alignment::Center);
                cr.move_to(20.0, 12.0);
                cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
                pangocairo::functions::show_layout(&cr, &layout);
            }
            let per_row = ((self.w - 2 * PAD) / CELL).max(1);
            for (i, wid) in self.wins.iter().enumerate() {
                let (col, row) = (i as i32 % per_row, i as i32 / per_row);
                let (x, y) = ((PAD + col * CELL) as f64, (PAD + TITLE_H + row * CELL) as f64);
                if i == self.sel {
                    crate::glass::rounded(&cr, x + 4.0, y + 4.0, CELL as f64 - 8.0, CELL as f64 - 8.0, 4.0);
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.28);
                    cr.fill_preserve().ok();
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.75);
                    cr.stroke().ok();
                }
                if let Some(icon) = wm.wins.get(wid).and_then(|c| c.icon.as_ref()) {
                    let sz = 48.0;
                    cr.save().ok();
                    cr.translate(x + (CELL as f64 - sz) / 2.0, y + (CELL as f64 - sz) / 2.0);
                    let k = sz / icon.width().max(1) as f64;
                    cr.scale(k, k);
                    cr.set_source_surface(icon, 0.0, 0.0).ok();
                    cr.source().set_filter(cairo::Filter::Good);
                    cr.paint().ok();
                    cr.restore().ok();
                } else {
                    crate::glass::rounded(&cr, x + 31.0, y + 31.0, 48.0, 48.0, 4.0);
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.6);
                    cr.fill().ok();
                }
            }
        }
        self.buf = super::render::surface_to_buffer(&mut s);
    }
}
