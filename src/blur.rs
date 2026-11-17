//! Lyric lines drawn out of focus. Slint cannot blur, so the line is drawn
//! here with swash, blurred, and shown as an image the window stretches over
//! where its own text would be.

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::sync::OnceLock;
use swash::scale::{Render, ScaleContext, Source};
use swash::shape::ShapeContext;
use swash::zeno::Format;
use swash::FontRef;

/// The fonts lines are drawn in, first choice first. Segoe UI Bold is close
/// to the window's font. (Not the variable Segoe UI the window uses: swash
/// gives some of its letters, such as r, far too wide an advance. The image
/// is stretched to the window's own width of the line.) Segoe UI has no
/// Korean, Japanese or Chinese, so those come from the fonts Windows itself
/// falls back to for them, in bold.
fn fonts() -> &'static [Vec<u8>] {
    static DATA: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    DATA.get_or_init(|| {
        let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()) + r"\Fonts\";
        let mut fonts: Vec<Vec<u8>> = ["segoeuib.ttf", "SegUIVar.ttf"]
            .iter()
            .find_map(|name| std::fs::read(dir.clone() + name).ok())
            .into_iter()
            .collect();
        for name in ["malgunbd.ttf", "YuGothB.ttc", "msyhbd.ttc", "msjhbd.ttc", "seguisym.ttf"] {
            if let Ok(data) = std::fs::read(dir.clone() + name) {
                fonts.push(data);
            }
        }
        fonts
    })
}

/// Splits `text` into runs, each with the index of the first font in
/// `fonts` that has all its letters. Spaces and marks stay with the run
/// they are in, so a word is never split between fonts.
fn runs<'a>(text: &'a str, fonts: &[FontRef]) -> Vec<(usize, &'a str)> {
    let pick = |c: char| fonts.iter().position(|f| f.charmap().map(c) != 0);
    let mut runs: Vec<(usize, &str)> = Vec::new();
    let mut start = 0;
    let mut current: Option<usize> = None;
    for (at, c) in text.char_indices() {
        let neutral = c.is_whitespace() || (c.is_ascii_punctuation() && current.is_some());
        let font = if neutral { current } else { pick(c).or(current) };
        if let (Some(old), Some(new)) = (current, font) {
            if old != new {
                runs.push((old, &text[start..at]));
                start = at;
            }
        }
        if font.is_some() {
            current = font;
        }
    }
    runs.push((current.unwrap_or(0), &text[start..]));
    runs.retain(|(_, run)| !run.is_empty());
    runs
}

/// Draws `text` white on clear at `px` physical pixels in bold, blurred by
/// about `radius` pixels. The image is the line's advance plus `margin` on
/// each side, and the line's ascent plus descent plus `margin` above and
/// below. `margin` should be about three times the radius, so the blur fits.
pub fn render(text: &str, px: f32, radius: f32, margin: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let fonts: Vec<FontRef> = fonts()
        .iter()
        .filter_map(|data| FontRef::from_index(data, 0))
        .collect();
    if fonts.is_empty() {
        return None;
    }
    let weight = [("wght", 700.0f32)];

    // Shape each run in its own font, one after another along the line.
    let mut shape = ShapeContext::new();
    let mut glyphs = Vec::new();
    let mut pen = 0.0f32;
    let (mut ascent, mut descent) = (0f32, 0f32);
    for (index, run) in runs(text, &fonts) {
        let mut shaper = shape.builder(fonts[index]).size(px).variations(weight).build();
        let metrics = shaper.metrics();
        ascent = ascent.max(metrics.ascent);
        descent = descent.max(metrics.descent);
        shaper.add_str(run);
        shaper.shape_with(|cluster| {
            for glyph in cluster.glyphs {
                glyphs.push((index, glyph.id, pen + glyph.x, glyph.y));
                pen += glyph.advance;
            }
        });
    }

    let width = pen.ceil() as u32 + margin * 2;
    let line = (ascent + descent).ceil() as u32;
    let height = line + margin * 2;
    let baseline = margin as f32 + ascent;
    let mut alpha = vec![0f32; (width * height) as usize];

    let mut contexts: Vec<ScaleContext> = fonts.iter().map(|_| ScaleContext::new()).collect();
    let mut scalers: Vec<_> = contexts
        .iter_mut()
        .zip(&fonts)
        .map(|(context, font)| context.builder(*font).size(px).variations(weight).hint(false).build())
        .collect();
    let sources = [Source::Outline];
    for (index, id, x, y) in glyphs {
        let scaler = &mut scalers[index];
        let origin_x = margin as f32 + x;
        let origin_y = baseline - y;
        let mut render = Render::new(&sources);
        render
            .format(Format::Alpha)
            .offset(swash::zeno::Vector::new(origin_x.fract(), 0.0));
        let Some(image) = render.render(scaler, id) else { continue };
        let p = image.placement;
        let left = origin_x.floor() as i32 + p.left;
        let top = origin_y.round() as i32 - p.top;
        for row in 0..p.height as i32 {
            for col in 0..p.width as i32 {
                let (tx, ty) = (left + col, top + row);
                if tx < 0 || ty < 0 || tx >= width as i32 || ty >= height as i32 {
                    continue;
                }
                let a = image.data[(row * p.width as i32 + col) as usize] as f32 / 255.0;
                let cell = &mut alpha[(ty as u32 * width + tx as u32) as usize];
                *cell = (*cell + a).min(1.0);
            }
        }
    }

    gaussian(&mut alpha, width as usize, height as usize, radius);

    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    for (out, a) in pixels.make_mut_slice().iter_mut().zip(&alpha) {
        let a = (a * 255.0).round().clamp(0.0, 255.0) as u8;
        // Premultiplied is not asked for: Slint takes plain RGBA here.
        *out = Rgba8Pixel { r: 255, g: 255, b: 255, a };
    }
    Some(pixels)
}

/// Three box blurs one way and then the other come close to a Gaussian of
/// standard deviation `sigma`.
fn gaussian(buf: &mut [f32], width: usize, height: usize, sigma: f32) {
    if sigma <= 0.0 {
        return;
    }
    // Box sizes for three passes, from "Fast Almost-Gaussian Filtering".
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut low = ideal.floor() as i32;
    if low % 2 == 0 {
        low -= 1;
    }
    let high = low + 2;
    let m = ((12.0 * sigma * sigma - 3.0 * (low * low) as f32 - 12.0 * low as f32 - 9.0)
        / (-4.0 * low as f32 - 4.0))
        .round() as i32;
    let mut tmp = vec![0f32; buf.len()];
    for pass in 0..3 {
        let size = if pass < m { low } else { high };
        let r = ((size - 1) / 2).max(0) as usize;
        if r == 0 {
            continue;
        }
        box_pass(buf, &mut tmp, width, height, r, true);
        box_pass(&tmp, buf, width, height, r, false);
    }
}

/// A running-sum box blur of radius `r` along rows or columns, clear past
/// the edges.
fn box_pass(src: &[f32], dst: &mut [f32], width: usize, height: usize, r: usize, rows: bool) {
    let (lines, len) = if rows { (height, width) } else { (width, height) };
    let at = |line: usize, i: usize| if rows { line * width + i } else { i * width + line };
    let scale = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let mut sum = 0.0f32;
        for i in 0..=r.min(len - 1) {
            sum += src[at(line, i)];
        }
        for i in 0..len {
            dst[at(line, i)] = sum * scale;
            if i + r + 1 < len {
                sum += src[at(line, i + r + 1)];
            }
            if i >= r {
                sum -= src[at(line, i - r)];
            }
        }
    }
}


#[cfg(test)]
mod tests {
    /// Each glyph sits right after the one before: no column of a word is
    /// left clear for more than a letter's gap.
    #[test]
    fn draws_words_without_gaps() {
        for text in ["harder", "really", "wrong"] {
            let img = super::render(text, 24.0, 0.0, 2).expect("Segoe UI is installed");
            let (w, h) = (img.width() as usize, img.height() as usize);
            let px = img.as_slice();
            let inked: Vec<bool> = (0..w).map(|x| (0..h).any(|y| px[y * w + x].a > 60)).collect();
            let first = inked.iter().position(|&i| i).unwrap();
            let last = inked.iter().rposition(|&i| i).unwrap();
            let mut gap = 0;
            let mut widest = 0;
            for &i in &inked[first..=last] {
                gap = if i { 0 } else { gap + 1 };
                widest = widest.max(gap);
            }
            assert!(widest <= 4, "{text:?} has a {widest}px gap");
            assert!(last - first < 90, "{text:?} is {}px wide", last - first);
        }
    }

    /// Korean, Japanese and Chinese come from fallback fonts rather than
    /// Segoe UI's empty box, which would draw every letter the same.
    #[test]
    fn draws_korean_japanese_and_chinese() {
        let inked = |text: &str| {
            let img = super::render(text, 24.0, 0.0, 2).expect("fonts are installed");
            img.as_slice().iter().filter(|p| p.a > 60).count()
        };
        // Two different letters ink differently; two empty boxes do not.
        assert_ne!(inked("바"), inked("람"));
        assert_ne!(inked("あ"), inked("ぬ"));
        assert_ne!(inked("一"), inked("龍"));
        assert!(inked("Ah-oh 비로소") > inked("Ah-oh"));
    }
}