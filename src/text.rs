//! Text engine: font lookup, measuring and drawing into a tiny-skia pixmap.
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use ab_glyph::{Font, FontArc, FontRef, FontVec, PxScale};

static BUNDLED: &[u8] = include_bytes!("../assets/JetBrainsMono-Bold.ttf");
/// Roboto Bold (Clean theme) cut to Latin, unhinted, no layout tables:
/// `pyftsubset Roboto-Bold.ttf --unicodes=U+0020-007E,U+00A0-00FF,U+0131,
/// U+0152-0153,U+0160-0161,U+0178,U+017D-017E,U+2013-2014,U+2018-201E,
/// U+2022,U+2026,U+2039-203A,U+20AC,U+2122,U+2190-2193,U+23F8,U+25A0,U+25B6
/// --no-hinting --layout-features='' --name-IDs='*' --notdef-outline
/// --drop-tables+=DSIG`.
static BUNDLED_ROBOTO: &[u8] = include_bytes!("../assets/Roboto-Bold.ttf");
const GENERIC_ALIASES: [&str; 4] = ["monospace", "sans", "sans-serif", "serif"];
/// Upper bound on cached glyph bitmaps (one frame at the largest font size
/// and 2x scale needs ~2 MB); least recently used glyphs are
/// evicted past it (font-size changes and pulse animations add sizes).
const CACHE_CAP: usize = 4 << 20;
/// Bookkeeping bytes charged per cached glyph on top of its bitmap.
const GLYPH_OVERHEAD: usize = 64;

/// A rasterized glyph: coverage bitmap and its offset from the pen
/// position on the baseline (y down).
struct Glyph {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    coverage: Vec<u8>,
    /// `FontSet::clock` value of the last use (LRU eviction).
    used: u64,
}

impl Glyph {
    fn bytes(&self) -> usize {
        self.coverage.len() + GLYPH_OVERHEAD
    }
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
    cache_bytes: usize,
    cache_cap: usize,
    clock: u64,
}

/// The bundled font, parsed once and shared by every `FontSet` (HUD,
/// menus, tooltip).
fn bundled_font() -> FontArc {
    static FONT: OnceLock<FontArc> = OnceLock::new();
    parse_once(&FONT, BUNDLED)
}

/// The bundled Roboto, parsed only if Roboto is asked for and not installed.
fn bundled_roboto() -> FontArc {
    static FONT: OnceLock<FontArc> = OnceLock::new();
    parse_once(&FONT, BUNDLED_ROBOTO)
}

fn parse_once(cell: &OnceLock<FontArc>, bytes: &'static [u8]) -> FontArc {
    cell.get_or_init(|| {
        FontArc::new(FontRef::try_from_slice(bytes).expect("bundled font is valid"))
    })
    .clone()
}

impl FontSet {
    /// Resolve `family` (Bold) through fontconfig, falling back to a bundled
    /// font ([`FontSet::fallback`]).
    pub fn load(family: &str) -> FontSet {
        resolve_font_file(family)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| FontVec::try_from_vec(b).ok())
            .and_then(|f| FontSet::new(FontArc::new(f), false))
            .unwrap_or_else(|| FontSet::fallback(family))
    }

    /// The bundled Roboto for "Roboto" (so the Clean theme looks the same
    /// without it installed), else the bundled JetBrains Mono.
    pub fn fallback(family: &str) -> FontSet {
        if family.trim().eq_ignore_ascii_case("roboto") {
            FontSet::new(bundled_roboto(), true).expect("bundled font is valid")
        } else {
            FontSet::bundled()
        }
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
            cache_bytes: 0,
            cache_cap: CACHE_CAP,
            clock: 0,
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
                used: 0,
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
            used: 0,
        }
    }

    /// The cached bitmap of `c` at `px`, rasterizing (and evicting the least
    /// recently used glyphs past `cache_cap`) on a miss.
    fn glyph(&mut self, c: char, px: f32) -> &Glyph {
        self.clock += 1;
        let key = (c, px as u32);
        if !self.cache.contains_key(&key) {
            let g = self.rasterize(c, px);
            self.cache_bytes += g.bytes();
            self.cache.insert(key, g);
            if self.cache_bytes > self.cache_cap {
                self.evict(key);
            }
        }
        let g = self.cache.get_mut(&key).expect("glyph cached");
        g.used = self.clock;
        g
    }

    /// Drops least recently used glyphs (never `keep`) down to 3/4 of the cap,
    /// so eviction does not run again on the next miss.
    fn evict(&mut self, keep: (char, u32)) {
        let low = self.cache_cap / 4 * 3;
        let mut by_age: Vec<((char, u32), u64)> = self
            .cache
            .iter()
            .filter(|(k, _)| **k != keep)
            .map(|(k, g)| (*k, g.used))
            .collect();
        by_age.sort_unstable_by_key(|(_, used)| *used);
        for (k, _) in by_age {
            if self.cache_bytes <= low {
                break;
            }
            if let Some(g) = self.cache.remove(&k) {
                self.cache_bytes -= g.bytes();
            }
        }
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
    fn roboto_falls_back_to_bundled_roboto() {
        for name in ["Roboto", "roboto", " ROBOTO "] {
            let f = FontSet::fallback(name);
            assert!(f.is_bundled());
            assert_eq!(f.font.font_data().as_ptr(), BUNDLED_ROBOTO.as_ptr());
        }
        let f = FontSet::fallback("Roboto Condensed");
        assert_eq!(f.font.font_data().as_ptr(), BUNDLED.as_ptr());
    }

    /// The bundled Roboto is a Latin subset: it must still cover everything
    /// the HUD draws, with equal-width digits (ab_glyph applies no `tnum`).
    #[test]
    fn bundled_roboto_covers_hud_text() {
        let mut f = FontSet::fallback("Roboto");
        for c in (' '..='~').chain("äöüßéèàçñ€…–—•".chars()) {
            assert_ne!(f.font.glyph_id(c).0, 0, "missing {c:?}");
        }
        assert_eq!(
            f.measure("11:11", 40., false),
            f.measure("88:88", 40., false)
        );
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

    fn draw_sizes(f: &mut FontSet, sizes: impl Iterator<Item = u32>) {
        let mut pm = tiny_skia::Pixmap::new(8, 8).unwrap();
        for px in sizes {
            f.draw(
                &mut pm,
                "0123456789:",
                0.,
                0.,
                px as f32,
                tiny_skia::Color::WHITE,
                true,
                0.,
            );
        }
    }

    /// A font with a small cache cap, so tests reach it quickly.
    fn small_cache() -> FontSet {
        let mut f = FontSet::bundled();
        f.cache_cap = 64 << 10;
        f
    }

    #[test]
    fn glyph_cache_is_bounded() {
        let mut f = small_cache();
        draw_sizes(&mut f, 20..160);
        assert!(f.cache_bytes <= f.cache_cap, "{}", f.cache_bytes);
        let total: usize = f.cache.values().map(Glyph::bytes).sum();
        assert_eq!(total, f.cache_bytes, "byte count stays in sync");
        // The most recent size survived eviction; the oldest did not.
        assert!(f.cache.contains_key(&('8', 159)));
        assert!(!f.cache.contains_key(&('8', 20)));
    }

    #[test]
    fn glyph_cache_keeps_recently_used_sizes() {
        let mut f = small_cache();
        draw_sizes(&mut f, std::iter::once(48));
        // Keep touching 48 px while other sizes push past the cap.
        for px in 60..160 {
            draw_sizes(&mut f, [px, 48].into_iter());
        }
        assert!(f.cache.contains_key(&('8', 48)));
        assert!(f.cache_bytes <= f.cache_cap);
    }

    #[test]
    fn default_cap_holds_a_frame_at_the_largest_size() {
        // font_size 120 x timer scale x 2x HiDPI stays well inside the cap.
        let mut f = FontSet::bundled();
        draw_sizes(&mut f, std::iter::once(480));
        assert!(f.cache_bytes * 2 < CACHE_CAP, "{}", f.cache_bytes);
    }

    #[test]
    fn generic_alias_accepted_by_resolver_does_not_panic() {
        let _ = resolve_font_file("Monospace");
    }
}
