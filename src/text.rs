//! Text engine: font lookup, measuring and drawing into a tiny-skia pixmap.
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

const BUNDLED: &[u8] = include_bytes!("../assets/JetBrainsMono-Bold.ttf");
const GENERIC_ALIASES: [&str; 4] = ["monospace", "sans", "sans-serif", "serif"];

type Glyph = (fontdue::Metrics, Vec<u8>);

pub struct FontSet {
    font: fontdue::Font,
    bundled: bool,
    cache: HashMap<(char, u32), Glyph>,
}

fn parse(bytes: &[u8]) -> Option<fontdue::Font> {
    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
}

impl FontSet {
    /// Resolve `family` (Bold) through fontconfig, falling back to the bundled font.
    pub fn load(family: &str) -> FontSet {
        let system = resolve_font_file(family)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| parse(&b));
        match system {
            Some(font) => FontSet {
                font,
                bundled: false,
                cache: HashMap::new(),
            },
            None => FontSet::bundled(),
        }
    }

    pub fn bundled() -> FontSet {
        FontSet {
            font: parse(BUNDLED).expect("bundled font is valid"),
            bundled: true,
            cache: HashMap::new(),
        }
    }

    pub fn is_bundled(&self) -> bool {
        self.bundled
    }

    fn glyph(&mut self, c: char, px: f32) -> &Glyph {
        let font = &self.font;
        self.cache
            .entry((c, px as u32))
            .or_insert_with(|| font.rasterize(c, px))
    }

    /// Height of the '8' glyph's bounding box (Qt `tightBoundingRect("8")`).
    pub fn ink_height(&mut self, px: f32) -> f32 {
        self.font.metrics('8', px).bounds.height
    }

    fn digit_cell(&self, px: f32) -> f32 {
        ('0'..='9')
            .map(|d| self.font.metrics(d, px).advance_width)
            .fold(0.0, f32::max)
    }

    /// Advance width of `text`; with `tabular` every ASCII digit takes the widest digit's cell.
    pub fn measure(&mut self, text: &str, px: f32, tabular: bool) -> f32 {
        let cell = self.digit_cell(px);
        text.chars()
            .map(|c| {
                if tabular && c.is_ascii_digit() {
                    cell
                } else {
                    self.font.metrics(c, px).advance_width
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
            let (m, bitmap) = self.glyph(c, px);
            let (adv, off) = if tabular && c.is_ascii_digit() {
                (cell, (cell - m.advance_width) / 2.0)
            } else {
                (m.advance_width, 0.0)
            };
            let gx = (pen + off).round() as i32 + m.xmin;
            let gy = (baseline - m.height as f32).round() as i32 - m.ymin;
            let data = pm.data_mut();
            for row in 0..m.height as i32 {
                let py = gy + row;
                if py < 0 || py >= h {
                    continue;
                }
                for col in 0..m.width as i32 {
                    let pxx = gx + col;
                    if pxx < 0 || pxx >= w {
                        continue;
                    }
                    let cov = bitmap[(row as usize) * m.width + col as usize] as f32 / 255.0;
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
    fn tabular_digits_equal_width() {
        let mut f = FontSet::bundled();
        assert_eq!(f.measure("11:11", 40., true), f.measure("88:88", 40., true));
        assert!(f.measure("88:88", 40., true) > 0.0);
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
    }

    #[test]
    fn generic_alias_accepted_by_resolver_does_not_panic() {
        let _ = resolve_font_file("Monospace");
    }
}
