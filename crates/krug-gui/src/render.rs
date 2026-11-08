//! Свой программный рендер: прямоугольники, скругления, круги и текст.
//!
//! Никакого WebView, GPU-контекста и HTML. Кадр — это `Vec<u32>` в формате
//! `0x00RRGGBB`, ровно такой, какой ждёт `softbuffer`. Этот же буфер можно
//! сохранить в PNG — так проверяется вёрстка без запуска окна.

use std::collections::HashMap;
use std::io::{self, Write};

use fontdue::{Font, FontSettings};

const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

/// Прямоугольник в логических пикселях.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }

    pub fn inset(&self, by: f32) -> Rect {
        Rect::new(self.x + by, self.y + by, self.w - by * 2.0, self.h - by * 2.0)
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

#[derive(Clone)]
struct Glyph {
    width: usize,
    height: usize,
    xmin: i32,
    ymin: i32,
    advance: f32,
    bitmap: Vec<u8>,
}

pub struct Renderer {
    pub width: usize,
    pub height: usize,
    pub scale: f32,
    pub buf: Vec<u32>,
    regular: Font,
    bold: Font,
    glyphs: HashMap<(u8, char, u32), Glyph>,
    clip: Option<(i32, i32, i32, i32)>,
    ascent: HashMap<(u8, u32), f32>,
    bg: Vec<u32>,
    blur: Vec<u32>,
    bg_ready: bool,
}

impl Renderer {
    pub fn new(width: usize, height: usize, scale: f32) -> Renderer {
        let regular = Font::from_bytes(REGULAR, FontSettings::default()).expect("шрифт Inter");
        let bold = Font::from_bytes(BOLD, FontSettings::default()).expect("Inter SemiBold");
        let mut r = Renderer {
            width: 0,
            height: 0,
            scale,
            buf: Vec::new(),
            regular,
            bold,
            glyphs: HashMap::new(),
            clip: None,
            ascent: HashMap::new(),
            bg: Vec::new(),
            blur: Vec::new(),
            bg_ready: false,
        };
        r.resize(width, height);
        r
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.buf = vec![0u32; self.width * self.height];
        self.bg_ready = false;
    }

    pub fn set_scale(&mut self, scale: f32) {
        if (self.scale - scale).abs() > 0.01 {
            self.scale = scale;
            self.glyphs.clear();
            self.ascent.clear();
            self.bg_ready = false;
        }
    }

    /// Рисует фон и запоминает размытую копию для стеклянных панелей.
    pub fn background(&mut self) {
        if !self.bg_ready || self.bg.len() != self.buf.len() {
            self.paint_background();
            self.bg.clear();
            self.bg.extend_from_slice(&self.buf);
            let (w, h) = (self.width, self.height);
            self.blur.resize(self.buf.len(), 0);
            let radius = ((46.0 * self.scale) as usize).clamp(2, 160);
            let mut tmp = vec![0u32; self.buf.len()];
            blur_h(&self.bg, &mut tmp, w, h, radius);
            blur_v(&tmp, &mut self.blur, w, h, radius);
            blur_h(&self.blur, &mut tmp, w, h, radius);
            blur_v(&tmp, &mut self.blur, w, h, radius);
            self.bg_ready = true;
        } else {
            self.buf.copy_from_slice(&self.bg);
        }
    }

    fn paint_background(&mut self) {
        let w = self.width as f32 / self.scale;
        let h = self.height as f32 / self.scale;
        self.gradient(0xe9eff3, 0xdce8ee);
        self.glow(w * 0.78, h * 0.12, h * 0.9, 0xffffff, 70);
        self.glow(w * 0.1, h * 0.9, h * 0.8, 0xcfe2ec, 60);
    }

    /// Стеклянная панель: тень, размытый фон, тонировка, блик и рамка.
    #[allow(dead_code)]
    pub fn glass(&mut self, rect: Rect, radius: f32, tint: u32, alpha: u8, border: u8) {
        self.shadow(rect, radius, 34.0, 30);
        self.glass_fill(rect, radius, tint, alpha);
        let clip = self.clip;
        let (cx, cy, cw, ch) = self.px_rect(rect);
        self.clip = Some((cx, cy, cw, ch));
        self.glow(
            rect.x + rect.w * 0.22,
            rect.y + rect.h * 0.02,
            rect.w * 0.55,
            0xffffff,
            22,
        );
        self.clip = clip;
        self.stroke_rounded(rect, radius, 1.0, 0x000000, 14);
        self.stroke_rounded(rect.inset(1.0), radius - 1.0, 1.0, 0xffffff, border);
    }

    /// Мягкая тень по контуру: считается только снаружи панели.
    pub fn shadow(&mut self, rect: Rect, radius: f32, spread: f32, alpha: u8) {
        let s = self.scale;
        let (x, y, w, h) = self.px_rect(rect);
        if w <= 0 || h <= 0 {
            return;
        }
        let r = (radius * s).min(w as f32 / 2.0).min(h as f32 / 2.0).max(0.0);
        let sp = (spread * s).max(1.0);
        let shift = 6.0 * s;
        let x0 = (x as f32 - sp).max(0.0) as i32;
        let y0 = (y as f32 - sp).max(0.0) as i32;
        let x1 = (x as f32 + w as f32 + sp).min(self.width as f32) as i32;
        let y1 = (y as f32 + h as f32 + sp).min(self.height as f32) as i32;
        for py in y0..y1 {
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                let fy = py as f32 + 0.5;
                let sdf = sdf_round_rect(
                    fx,
                    fy,
                    x as f32,
                    y as f32 + shift,
                    w as f32,
                    h as f32,
                    r,
                );
                if sdf > 0.0 && sdf < sp {
                    let t = 1.0 - sdf / sp;
                    self.blend(px, py, 0x000000, (alpha as f32 * t * t) as u8);
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn glass_fill(&mut self, rect: Rect, radius: f32, tint: u32, alpha: u8) {
        let s = self.scale;
        let (x, y, w, h) = self.px_rect(rect);
        if w <= 0 || h <= 0 {
            return;
        }
        let r = (radius * s).min(w as f32 / 2.0).min(h as f32 / 2.0).max(0.0);
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + w).min(self.width as i32), (y + h).min(self.height as i32));
        let a = alpha as f32 / 255.0;
        for py in y0..y1 {
            let rel = (py - y) as f32 / h as f32;
            for px in x0..x1 {
                let cov = coverage(
                    px as f32 + 0.5,
                    py as f32 + 0.5,
                    x as f32,
                    y as f32,
                    w as f32,
                    h as f32,
                    r,
                );
                if cov <= 0.0 {
                    continue;
                }
                let idx = py as usize * self.width + px as usize;
                let base = if self.blur.len() == self.buf.len() {
                    self.blur[idx]
                } else {
                    self.buf[idx]
                };
                let mut color = mix(base, tint, a);
                let sheen = (1.0 - (rel / 0.55).min(1.0)) * 0.17;
                color = mix(color, 0xffffff, sheen);
                if rel > 0.8 {
                    color = mix(color, 0x000000, (rel - 0.8) * 0.22);
                }
                self.blend(px, py, color, (255.0 * cov) as u8);
            }
        }
    }

    pub fn stroke_rounded(&mut self, rect: Rect, radius: f32, width: f32, color: u32, alpha: u8) {
        let s = self.scale;
        let (x, y, w, h) = self.px_rect(rect);
        if w <= 0 || h <= 0 {
            return;
        }
        let r = (radius * s).min(w as f32 / 2.0).min(h as f32 / 2.0).max(0.0);
        let iw = (width * s).max(1.0);
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + w).min(self.width as i32), (y + h).min(self.height as i32));
        for py in y0..y1 {
            let t = (py - y) as f32 / h as f32;
            let a = alpha as f32 * (1.0 - 0.55 * t);
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                let fy = py as f32 + 0.5;
                let outer = coverage(fx, fy, x as f32, y as f32, w as f32, h as f32, r);
                let inner = coverage(
                    fx,
                    fy,
                    x as f32 + iw,
                    y as f32 + iw,
                    w as f32 - iw * 2.0,
                    h as f32 - iw * 2.0,
                    (r - iw).max(0.0),
                );
                let cov = (outer - inner).max(0.0);
                if cov > 0.0 {
                    self.blend(px, py, color, (a * cov) as u8);
                }
            }
        }
    }

    fn font(&self, bold: bool) -> &Font {
        if bold {
            &self.bold
        } else {
            &self.regular
        }
    }

    pub fn line_height(&self, size: f32) -> f32 {
        size * 1.42
    }

    pub fn ascent(&mut self, size: f32, bold: bool) -> f32 {
        let key = (bold as u8, (size * 16.0) as u32);
        if let Some(a) = self.ascent.get(&key) {
            return *a;
        }
        let metrics = self.font(bold).horizontal_line_metrics(size);
        let a = metrics.map(|m| m.ascent).unwrap_or(size * 0.8);
        self.ascent.insert(key, a);
        a
    }

    fn glyph(&mut self, ch: char, size: f32, bold: bool) -> &Glyph {
        let key = (bold as u8, ch, (size * 16.0) as u32);
        if !self.glyphs.contains_key(&key) {
            let px = (size * self.scale).max(1.0);
            let (metrics, bitmap) = self.font(bold).rasterize(ch, px);
            self.glyphs.insert(
                key,
                Glyph {
                    width: metrics.width,
                    height: metrics.height,
                    xmin: metrics.xmin,
                    ymin: metrics.ymin,
                    advance: metrics.advance_width / self.scale,
                    bitmap,
                },
            );
        }
        self.glyphs.get(&key).expect("глиф только что вставили")
    }

    // ------------------------------------------------------------------
    // Геометрия

    fn px_rect(&self, rect: Rect) -> (i32, i32, i32, i32) {
        let s = self.scale;
        (
            (rect.x * s).round() as i32,
            (rect.y * s).round() as i32,
            (rect.w * s).round() as i32,
            (rect.h * s).round() as i32,
        )
    }

    pub fn blend(&mut self, x: i32, y: i32, color: u32, alpha: u8) {
        if alpha == 0 || x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        if let Some((cx, cy, cw, ch)) = self.clip {
            if x < cx || y < cy || x >= cx + cw || y >= cy + ch {
                return;
            }
        }
        let idx = y as usize * self.width + x as usize;
        let dst = self.buf[idx];
        if alpha == 255 {
            self.buf[idx] = color;
            return;
        }
        let a = alpha as u32;
        let inv = 255 - a;
        let r = (((color >> 16) & 0xff) * a + ((dst >> 16) & 0xff) * inv) / 255;
        let g = (((color >> 8) & 0xff) * a + ((dst >> 8) & 0xff) * inv) / 255;
        let b = ((color & 0xff) * a + (dst & 0xff) * inv) / 255;
        self.buf[idx] = (r << 16) | (g << 8) | b;
    }

    pub fn rect(&mut self, rect: Rect, color: u32) {
        self.rect_alpha(rect, color, 255);
    }

    pub fn rect_alpha(&mut self, rect: Rect, color: u32, alpha: u8) {
        let (x, y, w, h) = self.px_rect(rect);
        if w <= 0 || h <= 0 {
            return;
        }
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + w).min(self.width as i32), (y + h).min(self.height as i32));
        for py in y0..y1 {
            for px in x0..x1 {
                self.blend(px, py, color, alpha);
            }
        }
    }

    pub fn rounded(&mut self, rect: Rect, radius: f32, color: u32) {
        self.rounded_alpha(rect, radius, color, 255);
    }

    pub fn rounded_alpha(&mut self, rect: Rect, radius: f32, color: u32, alpha: u8) {
        let s = self.scale;
        let (x, y, w, h) = self.px_rect(rect);
        if w <= 0 || h <= 0 {
            return;
        }
        let r = (radius * s).min(w as f32 / 2.0).min(h as f32 / 2.0).max(0.0);
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + w).min(self.width as i32), (y + h).min(self.height as i32));
        for py in y0..y1 {
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                let fy = py as f32 + 0.5;
                let cx = fx.clamp(x as f32 + r, x as f32 + w as f32 - r);
                let cy = fy.clamp(y as f32 + r, y as f32 + h as f32 - r);
                let dist = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                let cov = (r - dist + 0.5).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(px, py, color, (alpha as f32 * cov) as u8);
                }
            }
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, radius: f32, color: u32) {
        let s = self.scale;
        let pcx = cx * s;
        let pcy = cy * s;
        let pr = radius * s;
        let x0 = (pcx - pr - 1.0).floor().max(0.0) as i32;
        let y0 = (pcy - pr - 1.0).floor().max(0.0) as i32;
        let x1 = (pcx + pr + 1.0).ceil().min(self.width as f32) as i32;
        let y1 = (pcy + pr + 1.0).ceil().min(self.height as f32) as i32;
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - pcx;
                let dy = py as f32 + 0.5 - pcy;
                let dist = (dx * dx + dy * dy).sqrt();
                let cov = (pr - dist + 0.5).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(px, py, color, (255.0 * cov) as u8);
                }
            }
        }
    }

    /// Мягкое радиальное свечение — для фона экрана входа.
    pub fn glow(&mut self, cx: f32, cy: f32, radius: f32, color: u32, strength: u8) {
        let s = self.scale;
        let pcx = cx * s;
        let pcy = cy * s;
        let pr = radius * s;
        let x0 = (pcx - pr).floor().max(0.0) as i32;
        let y0 = (pcy - pr).floor().max(0.0) as i32;
        let x1 = (pcx + pr).ceil().min(self.width as f32) as i32;
        let y1 = (pcy + pr).ceil().min(self.height as f32) as i32;
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - pcx;
                let dy = py as f32 + 0.5 - pcy;
                let t = (dx * dx + dy * dy).sqrt() / pr;
                if t < 1.0 {
                    let falloff = (1.0 - t) * (1.0 - t);
                    self.blend(px, py, color, (strength as f32 * falloff) as u8);
                }
            }
        }
    }

    /// Вертикальный градиент на весь кадр.
    pub fn gradient(&mut self, top: u32, bottom: u32) {
        for y in 0..self.height {
            let t = y as f32 / (self.height.max(2) - 1) as f32;
            let color = mix(top, bottom, t);
            for x in 0..self.width {
                self.buf[y * self.width + x] = color;
            }
        }
    }

    /// Заливка треугольника (иконка «отправить»).
    pub fn triangle(&mut self, a: (f32, f32), b: (f32, f32), c: (f32, f32), color: u32) {
        let s = self.scale;
        let a = (a.0 * s, a.1 * s);
        let b = (b.0 * s, b.1 * s);
        let c = (c.0 * s, c.1 * s);
        let min_x = a.0.min(b.0).min(c.0).floor().max(0.0) as i32;
        let max_x = a.0.max(b.0).max(c.0).ceil().min(self.width as f32) as i32;
        let min_y = a.1.min(b.1).min(c.1).floor().max(0.0) as i32;
        let max_y = a.1.max(b.1).max(c.1).ceil().min(self.height as f32) as i32;
        let edge = |p1: (f32, f32), p2: (f32, f32), px: f32, py: f32| {
            (px - p1.0) * (p2.1 - p1.1) - (py - p1.1) * (p2.0 - p1.0)
        };
        for py in min_y..max_y {
            for px in min_x..max_x {
                let x = px as f32 + 0.5;
                let y = py as f32 + 0.5;
                let e0 = edge(a, b, x, y);
                let e1 = edge(b, c, x, y);
                let e2 = edge(c, a, x, y);
                if (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0)
                {
                    self.blend(px, py, color, 255);
                }
            }
        }
    }

    /// Мелкий узор обоев: точки с шагом.
    pub fn pattern(&mut self, rect: Rect, step: f32, color: u32, alpha: u8) {
        let mut y = rect.y + 10.0;
        while y < rect.bottom() {
            let mut x = rect.x + 10.0;
            while x < rect.right() {
                self.rounded_alpha(
                    Rect::new(x - 1.2, y - 1.2, 2.4, 2.4),
                    1.2,
                    color,
                    alpha,
                );
                x += step;
            }
            y += step;
        }
    }

    pub fn set_clip(&mut self, rect: Option<Rect>) {
        self.clip = rect.map(|r| {
            let (x, y, w, h) = self.px_rect(r);
            (x, y, w, h)
        });
    }

    // ------------------------------------------------------------------
    // Текст

    pub fn text_width(&mut self, text: &str, size: f32, bold: bool) -> f32 {
        let mut width = 0.0;
        for ch in text.chars() {
            if ch == '\n' {
                continue;
            }
            width += self.glyph(ch, size, bold).advance;
        }
        width
    }

    /// Рисует строку, `y` — верх строки. Возвращает ширину.
    pub fn text(&mut self, x: f32, y: f32, text: &str, size: f32, color: u32, bold: bool) -> f32 {
        let s = self.scale;
        let baseline = y * s + self.ascent(size, bold) * s;
        let mut pen = x * s;
        let start = pen;
        for ch in text.chars() {
            if ch == '\n' {
                break;
            }
            let key = (bold as u8, ch, (size * 16.0) as u32);
            if !self.glyphs.contains_key(&key) {
                let _ = self.glyph(ch, size, bold);
            }
            let glyph = self.glyphs.remove(&key).expect("глиф");
            if glyph.width > 0 && glyph.height > 0 {
                let gx = (pen + glyph.xmin as f32).round() as i32;
                let gy = (baseline - glyph.ymin as f32 - glyph.height as f32).round() as i32;
                for row in 0..glyph.height {
                    for col in 0..glyph.width {
                        let a = glyph.bitmap[row * glyph.width + col];
                        if a > 0 {
                            self.blend(gx + col as i32, gy + row as i32, color, a);
                        }
                    }
                }
            }
            pen += glyph.advance * s;
            self.glyphs.insert(key, glyph);
        }
        (pen - start) / s
    }

    /// Рисует текст с центрированием по вертикали в прямоугольнике.
    pub fn text_centered(&mut self, rect: Rect, text: &str, size: f32, color: u32, bold: bool) {
        let tw = self.text_width(text, size, bold);
        let th = self.line_height(size);
        self.text(
            rect.x + (rect.w - tw) / 2.0,
            rect.y + (rect.h - th) / 2.0,
            text,
            size,
            color,
            bold,
        );
    }

    /// Перенос по словам. Возвращает строки без изменения исходного текста.
    pub fn wrap(&mut self, text: &str, size: f32, bold: bool, max_width: f32) -> Vec<String> {
        let mut out = Vec::new();
        for paragraph in text.split('\n') {
            if paragraph.is_empty() {
                out.push(String::new());
                continue;
            }
            let mut line = String::new();
            for word in paragraph.split(' ') {
                let candidate = if line.is_empty() {
                    word.to_string()
                } else {
                    format!("{line} {word}")
                };
                if self.text_width(&candidate, size, bold) <= max_width || line.is_empty() {
                    if self.text_width(&candidate, size, bold) > max_width && line.is_empty() {
                        // Очень длинное слово — режем по символам.
                        let mut piece = String::new();
                        for ch in word.chars() {
                            let test = format!("{piece}{ch}");
                            if self.text_width(&test, size, bold) > max_width && !piece.is_empty() {
                                out.push(piece.clone());
                                piece.clear();
                            }
                            piece.push(ch);
                        }
                        line = piece;
                    } else {
                        line = candidate;
                    }
                } else {
                    out.push(line.clone());
                    line = word.to_string();
                }
            }
            out.push(line);
        }
        if out.is_empty() {
            out.push(String::new());
        }
        out
    }
}

fn sdf_round_rect(fx: f32, fy: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let qx = (fx - (x + w / 2.0)).abs() - (w / 2.0 - r);
    let qy = (fy - (y + h / 2.0)).abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn coverage(fx: f32, fy: f32, xf: f32, yf: f32, wf: f32, hf: f32, r: f32) -> f32 {
    if fx < xf || fx > xf + wf || fy < yf || fy > yf + hf {
        return 0.0;
    }
    let cx = fx.clamp(xf + r, xf + wf - r);
    let cy = fy.clamp(yf + r, yf + hf - r);
    let dist = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
    (r - dist + 0.5).clamp(0.0, 1.0)
}

fn blur_h(src: &[u32], dst: &mut [u32], w: usize, h: usize, r: usize) {
    if w == 0 || h == 0 {
        return;
    }
    let r = r.min(w);
    let mut pr = vec![0u64; w + 1];
    let mut pg = vec![0u64; w + 1];
    let mut pb = vec![0u64; w + 1];
    for y in 0..h {
        for x in 0..w {
            let p = src[y * w + x];
            pr[x + 1] = pr[x] + ((p >> 16) & 0xff) as u64;
            pg[x + 1] = pg[x] + ((p >> 8) & 0xff) as u64;
            pb[x + 1] = pb[x] + (p & 0xff) as u64;
        }
        for x in 0..w {
            let x0 = x.saturating_sub(r);
            let x1 = (x + r).min(w - 1);
            let n = (x1 - x0 + 1) as u64;
            let rr = ((pr[x1 + 1] - pr[x0]) / n) as u32;
            let gg = ((pg[x1 + 1] - pg[x0]) / n) as u32;
            let bb = ((pb[x1 + 1] - pb[x0]) / n) as u32;
            dst[y * w + x] = (rr << 16) | (gg << 8) | bb;
        }
    }
}

fn blur_v(src: &[u32], dst: &mut [u32], w: usize, h: usize, r: usize) {
    if w == 0 || h == 0 {
        return;
    }
    let r = r.min(h);
    for x in 0..w {
        let mut pr = vec![0u64; h + 1];
        let mut pg = vec![0u64; h + 1];
        let mut pb = vec![0u64; h + 1];
        for py in 0..h {
            let p = src[py * w + x];
            pr[py + 1] = pr[py] + ((p >> 16) & 0xff) as u64;
            pg[py + 1] = pg[py] + ((p >> 8) & 0xff) as u64;
            pb[py + 1] = pb[py] + (p & 0xff) as u64;
        }
        for py in 0..h {
            let y0 = py.saturating_sub(r);
            let y1 = (py + r).min(h - 1);
            let n = (y1 - y0 + 1) as u64;
            let rr = ((pr[y1 + 1] - pr[y0]) / n) as u32;
            let gg = ((pg[y1 + 1] - pg[y0]) / n) as u32;
            let bb = ((pb[y1 + 1] - pb[y0]) / n) as u32;
            dst[py * w + x] = (rr << 16) | (gg << 8) | bb;
        }
    }
}

/// Смешивает два цвета: `t = 0` — `a`, `t = 1` — `b`.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u32, y: u32| -> u32 {
        let x = x as f32;
        let y = y as f32;
        (x + (y - x) * t) as u32
    };
    (lerp((a >> 16) & 0xff, (b >> 16) & 0xff) << 16)
        | (lerp((a >> 8) & 0xff, (b >> 8) & 0xff) << 8)
        | lerp(a & 0xff, b & 0xff)
}

// ---------------------------------------------------------------------------
// PNG без зависимостей: RGB, zlib «stored», CRC32 и Adler32.

pub fn write_png(path: &std::path::Path, width: usize, height: usize, pixels: &[u32]) -> io::Result<()> {
    let mut raw = Vec::with_capacity(height * (width * 3 + 1));
    for y in 0..height {
        raw.push(0u8);
        for x in 0..width {
            let p = pixels[y * width + x];
            raw.push(((p >> 16) & 0xff) as u8);
            raw.push(((p >> 8) & 0xff) as u8);
            raw.push((p & 0xff) as u8);
        }
    }

    let mut zlib = Vec::with_capacity(raw.len() + 32);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let mut chunks = raw.chunks(65535);
    loop {
        match chunks.next() {
            Some(chunk) => {
                let last = chunks.len() == 0;
                zlib.push(if last { 1 } else { 0 });
                let len = chunk.len() as u16;
                zlib.extend_from_slice(&len.to_le_bytes());
                zlib.extend_from_slice(&(!len).to_le_bytes());
                zlib.extend_from_slice(chunk);
                if last {
                    break;
                }
            }
            None => break,
        }
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::with_capacity(zlib.len() + 64);
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    write_chunk(&mut png, b"IHDR", &ihdr);
    write_chunk(&mut png, b"IDAT", &zlib);
    write_chunk(&mut png, b"IEND", &[]);

    let mut file = std::fs::File::create(path)?;
    file.write_all(&png)?;
    file.flush()
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in data {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_bytes_are_valid_header() {
        let path = std::env::temp_dir().join("krug-render-test.png");
        let px = vec![0x112233u32; 4];
        write_png(&path, 2, 2, &px).unwrap();
        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(&data[12..16], b"IHDR");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn text_has_width() {
        let mut r = Renderer::new(100, 100, 1.0);
        assert!(r.text_width("привет", 16.0, false) > 20.0);
        assert_eq!(r.text_width("", 16.0, false), 0.0);
    }

    #[test]
    fn wrapping_respects_width() {
        let mut r = Renderer::new(100, 100, 1.0);
        let lines = r.wrap("привет мир как дела", 14.0, false, 70.0);
        assert!(lines.len() > 1);
        for line in lines {
            assert!(r.text_width(&line, 14.0, false) <= 71.0);
        }
    }
}
