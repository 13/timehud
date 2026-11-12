//! Text engine: font lookup, measuring and drawing into a tiny-skia pixmap.
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use ab_glyph::{Font, FontArc, FontRef, FontVec, PxScale};

static BUNDLED: &[u8] = include_bytes!("../assets/JetBrainsMono-Bold.ttf");
const GENERIC_ALIASES: [&str; 4] = ["monospace", "sans", "sans-serif", "serif"];

/// A rasterized glyph: coverage bitmap and its offset from the pen
/// position on the baseline (y down).
struct Glyph {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    coverage: Vec<u8>,
}

/// A font plus its bitmap cache. Outlines are parsed lazily per glyph
/// (ab_glyph), so loading a font with thousands of glyphs costs only the
/// file read and table directory, not every outline.
pub struct FontSet {
    font: FontArc,
    bundled: bool,
    units_per_em: f32,
    /// Unscaled ink height of '8'.
    eight_height: f32,
    /// Unscaled advance of the widest ASCII digit.
    digit_advance: f32,
    cache: HashMap<(char, u32), Glyph>,
}

/// The bundled font, parsed once and shared by every `FontSet` (HUD,
/// menus, tooltip).
fn bundled_font() -> FontArc {
    static FONT: OnceLock<FontArc> = OnceLock::new();
    FONT.get_or_init(|| {
        FontArc::new(FontRef::try_from_slice(BUNDLED).expect("bundled font is valid"))
    })
    .clone()
}

impl FontSet {
    /// Resolve `family` (Bold) through fontconfig, falling back to the bundled font.
    pub fn load(family: &str) -> FontSet {
        resolve_font_file(family)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| FontVec::try_from_vec(b).ok())
            .and_then(|f| FontSet::new(FontArc::new(f), false))
            .unwrap_or_else(FontSet::bundled)
    }

    pub fn bundled() -> FontSet {
        FontSet::new(bundled_font(), true).expect("bundled font is valid")
    }

    /// None for fonts the HUD cannot use (no units-per-em or no '8').
    fn new(font: FontArc, bundled: bool) -> Option<FontSet> {
        let units_per_em = font.units_per_em().filter(|u| *u > 0.0)?;
        let eight = font.outline(font.glyph_id('8'))?.bounds;
        let digit_advance = ('0'..='9')
            .map(|d| font.h_advance_unscaled(font.glyph_id(d)))
            .fold(0.0, f32::max);
        Some(FontSet {
            font,
            bundled,
            units_per_em,
            eight_height: (eight.max.y - eight.min.y).abs(),
            digit_advance,
            cache: HashMap::new(),
        })
    }

    pub fn is_bundled(&self) -> bool {
        self.bundled
    }

    /// Pixels per font unit at an em size of `px`.
    fn factor(&self, px: f32) -> f32 {
        px / self.units_per_em
    }

    fn advance(&self, c: char, px: f32) -> f32 {
        self.font.h_advance_unscaled(self.font.glyph_id(c)) * self.factor(px)
    }

    fn rasterize(&self, c: char, px: f32) -> Glyph {
        // ab_glyph's PxScale is the ascent-to-descent height; `px` is the em size.
        let scale = PxScale::from(px * self.font.height_unscaled() / self.units_per_em);
        let glyph = self.font.glyph_id(c).with_scale(scale);
        let Some(outline) = self.font.outline_glyph(glyph) else {
            // Whitespace: nothing to draw.
            return Glyph {
                left: 0,
                top: 0,
                width: 0,
                height: 0,
                coverage: Vec::new(),
            };
        };
        let b = outline.px_bounds();
        let (width, height) = (b.width() as usize, b.height() as usize);
        let mut coverage = vec![0u8; width * height];
        outline.draw(|x, y, cov| {
            if let Some(v) = coverage.get_mut(y as usize * width + x as usize) {
                *v = (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
        Glyph {
            left: b.min.x as i32,
            top: b.min.y as i32,
            width,
            height,
            coverage,
        }
    }

    fn glyph(&mut self, c: char, px: f32) -> &Glyph {
        if !self.cache.contains_key(&(c, px as u32)) {
            let g = self.rasterize(c, px);
            self.cache.insert((c, px as u32), g);
        }
        &self.cache[&(c, px as u32)]
    }

    /// Height of the '8' glyph's bounding box (Qt `tightBoundingRect("8")`).
    pub fn ink_height(&mut self, px: f32) -> f32 {
        self.eight_height * self.factor(px)
    }

    fn digit_cell(&self, px: f32) -> f32 {
        self.digit_advance * self.factor(px)
    }

    /// Advance width of `text`; with `tabular` every ASCII digit takes the widest digit's cell.
    pub fn measure(&mut self, text: &str, px: f32, tabular: bool) -> f32 {
        let cell = self.digit_cell(px);
        text.chars()
            .map(|c| {
                if tabular && c.is_ascii_digit() {
                    cell
                } else {
                    self.advance(c, px)
                }
            })
            .sum()
    }

    /// Draw `text` with its origin at (`x`, `baseline`), compositing coverage x color
    /// into the premultiplied pixmap.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pm: &mut tiny_skia::Pixmap,
        text: &str,
        x: f32,
        baseline: f32,
        px: f32,
        color: tiny_skia::Color,
        tabular: bool,
        letter_spacing: f32,
    ) {
        let cell = self.digit_cell(px);
        let (w, h) = (pm.width() as i32, pm.height() as i32);
        let mut pen = x;
        for c in text.chars() {
            let advance = self.advance(c, px);
            let (adv, off) = if tabular && c.is_ascii_digit() {
                (cell, (cell - advance) / 2.0)
            } else {
                (advance, 0.0)
            };
            let g = self.glyph(c, px);
            let gx = (pen + off).round() as i32 + g.left;
            let gy = baseline.round() as i32 + g.top;
            let data = pm.data_mut();
            for row in 0..g.height as i32 {
                let py = gy + row;
                if py < 0 || py >= h {
                    continue;
                }
                for col in 0..g.width as i32 {
                    let pxx = gx + col;
                    if pxx < 0 || pxx >= w {
                        continue;
                    }
                    let cov = g.coverage[(row as usize) * g.width + col as usize] as f32 / 255.0;
                    if cov == 0.0 {
                        continue;
                    }
                    let a = cov * color.alpha();
                    let i = ((py * w + pxx) * 4) as usize;
                    let src = [color.red() * a, color.green() * a, color.blue() * a, a];
                    for (k, s) in src.iter().enumerate() {
                        let d = data[i + k] as f32 / 255.0;
                        data[i + k] = ((s + d * (1.0 - a)) * 255.0).round().min(255.0) as u8;
                    }
                }
            }
            pen += adv + letter_spacing;
        }
    }
}

/// Path of the Bold font file for `family` per `fc-match`, or None if fontconfig is
/// missing or only returned an unrelated fallback font.
pub fn resolve_font_file(family: &str) -> Option<PathBuf> {
    let out = Command::new("fc-match")
        .args(["-f", "%{family}|%{file}", &format!("{family}:style=Bold")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let (fam, file) = s.split_once('|')?;
    let want = family.trim().to_lowercase();
    if want.is_empty() || file.is_empty() {
        return None;
    }
    let ok = GENERIC_ALIASES.contains(&want.as_str()) || fam.to_lowercase().contains(&want);
    ok.then(|| PathBuf::from(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_family_uses_bundled() {
        assert!(FontSet::load("Definitely Not A Font 123").is_bundled());
    }

    #[test]
    fn bundled_font_is_parsed_once_and_shared() {
        let (a, b) = (FontSet::bundled(), FontSet::bundled());
        let (pa, pb) = (a.font.font_data().as_ptr(), b.font.font_data().as_ptr());
        assert_eq!(pa, pb);
        assert_eq!(pa, BUNDLED.as_ptr(), "no copy of the embedded bytes");
    }

    #[test]
    fn tabular_digits_equal_width() {
        let mut f = FontSet::bundled();
        assert_eq!(f.measure("11:11", 40., true), f.measure("88:88", 40., true));
        assert!(f.measure("88:88", 40., true) > 0.0);
    }

    #[test]
    fn monospace_advance_is_em_scaled() {
        // JetBrains Mono: 600/1000 em advance.
        let mut f = FontSet::bundled();
        assert!((f.measure("0", 40., false) - 24.0).abs() < 0.01);
        assert!((f.measure("ab", 40., false) - 48.0).abs() < 0.01);
    }

    #[test]
    fn ink_height_scales() {
        let mut f = FontSet::bundled();
        let (a, b) = (f.ink_height(40.), f.ink_height(80.));
        assert!(a > 10.0);
        assert!((b - 2. * a).abs() <= 1.0, "{a} {b}");
    }

    #[test]
    fn draw_puts_ink_in_pixmap() {
        let mut f = FontSet::bundled();
        let mut pm = tiny_skia::Pixmap::new(64, 64).unwrap();
        f.draw(
            &mut pm,
            "8",
            8.,
            50.,
            40.,
            tiny_skia::Color::WHITE,
            false,
            0.,
        );
        assert!(pm.pixels().iter().any(|p| p.alpha() > 200));
        // Ink sits above the baseline, about as tall as `ink_height`.
        let rows: Vec<u32> = (0..64)
            .filter(|y| (0..64).any(|x| pm.pixel(x, *y).unwrap().alpha() > 0))
            .collect();
        let (top, bottom) = (rows[0], *rows.last().unwrap());
        assert!(bottom <= 50, "bottom {bottom}");
        let ink = f.ink_height(40.);
        assert!(
            ((bottom - top + 1) as f32 - ink).abs() <= 2.0,
            "{top}..{bottom} vs {ink}"
        );
    }

    #[test]
    fn generic_alias_accepted_by_resolver_does_not_panic() {
        let _ = resolve_font_file("Monospace");
    }
}
