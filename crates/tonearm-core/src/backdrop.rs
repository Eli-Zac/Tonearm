//! The background of expanded Now playing: the cover blurred to a soft
//! wash of its colours, flowing slowly like ink in water. Pixels are RGBA,
//! four bytes each, row by row. The results are small; the app stretches
//! them smoothly over the whole view.

/// How many pixels a side the still backdrop is worked out at.
pub const BASE: usize = 32;
/// How many pixels a side each frame of the flow is.
pub const FRAME: usize = 64;

/// A very blurred copy of the `width` by `height` picture `rgba`: shrunk to
/// `BASE` pixels a side by averaging, then softened twice more so no block
/// edges show when it is stretched. `BASE * BASE * 4` bytes.
pub fn backdrop(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    let (w, h, n) = (width, height, BASE);
    let mut small = vec![[0f32; 3]; n * n];
    if w > 0 && h > 0 && rgba.len() >= w * h * 4 {
        for y in 0..n {
            let (y0, y1) = (y * h / n, ((y + 1) * h / n).max(y * h / n + 1).min(h));
            for x in 0..n {
                let (x0, x1) = (x * w / n, ((x + 1) * w / n).max(x * w / n + 1).min(w));
                let mut sum = [0f32; 3];
                for yy in y0..y1 {
                    for p in rgba[(yy * w + x0) * 4..(yy * w + x1) * 4].chunks_exact(4) {
                        sum[0] += p[0] as f32;
                        sum[1] += p[1] as f32;
                        sum[2] += p[2] as f32;
                    }
                }
                let count = ((y1 - y0) * (x1 - x0)).max(1) as f32;
                small[y * n + x] = [sum[0] / count, sum[1] / count, sum[2] / count];
            }
        }
    }
    // Two passes of a 5 by 5 box, edges held.
    for _ in 0..2 {
        let before = small.clone();
        for y in 0..n {
            for x in 0..n {
                let mut sum = [0f32; 3];
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let yy = (y as i32 + dy).clamp(0, n as i32 - 1) as usize;
                        let xx = (x as i32 + dx).clamp(0, n as i32 - 1) as usize;
                        let p = before[yy * n + xx];
                        sum[0] += p[0];
                        sum[1] += p[1];
                        sum[2] += p[2];
                    }
                }
                small[y * n + x] = [sum[0] / 25.0, sum[1] / 25.0, sum[2] / 25.0];
            }
        }
    }
    // A light cover would make a light background, and white words on it
    // hard to read. Each pixel is held to a brightness the words stand out
    // on, keeping its colour; darker covers are left as they are.
    const BRIGHTEST: f32 = 96.0;
    let mut out = Vec::with_capacity(n * n * 4);
    for s in &small {
        let luma = 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
        let k = if luma > BRIGHTEST { BRIGHTEST / luma } else { 1.0 };
        out.extend_from_slice(&[(s[0] * k) as u8, (s[1] * k) as u8, (s[2] * k) as u8, 255]);
    }
    out
}

/// `base`, a `width` by `height` backdrop, flowing like liquid at `t`
/// seconds. Each point reads its colour from somewhere else, and where it
/// reads from is itself pushed around by a slower current, twice over, so
/// the colours curl, fold and swirl into one another like ink in water,
/// and never repeat quite the same. Zoomed in, so the edges never show,
/// and a little more saturated, so the movement reads. `FRAME * FRAME * 4`
/// bytes.
///
/// Made many times a second, so the slow current, which only varies along
/// one axis at a time, is worked out once a row and once a column.
pub fn wave(base: &[u8], width: usize, height: usize, t: f32) -> Vec<u8> {
    let n = FRAME;
    let (w, h) = (width, height);
    let mut out = vec![0u8; n * n * 4];
    if w < 2 || h < 2 || base.len() < w * h * 4 {
        return out;
    }
    let at = |x: usize, y: usize| {
        let i = (y.min(h - 1) * w + x.min(w - 1)) * 4;
        [base[i] as f32, base[i + 1] as f32, base[i + 2] as f32]
    };
    // -1 to 1 across the image.
    let coord = |i: usize| i as f32 / (n - 1) as f32 * 2.0 - 1.0;
    // The slow current's parts: those along x, and those along y.
    let mut along_x = [[0f32; 2]; FRAME];
    let mut along_y = [[0f32; 2]; FRAME];
    for i in 0..n {
        let c = coord(i);
        along_x[i] = [(c * 1.7 + t * 0.21).sin(), 0.5 * (c * 2.3 + t * 0.23).cos()];
        along_y[i] = [0.5 * (c * 2.9 - t * 0.17).sin(), (c * 1.9 - t * 0.19).cos()];
    }
    for y in 0..n {
        let v = coord(y);
        for x in 0..n {
            let u = coord(x);
            // The slow current.
            let qx = along_x[x][0] + along_y[y][0];
            let qy = along_y[y][1] + along_x[x][1];
            // The current it carries, which curls the colours.
            let rx = ((u + 0.9 * qx) * 2.1 + t * 0.29).sin();
            let ry = ((v + 0.9 * qy) * 2.4 - t * 0.25).cos();
            let (su, sv) = (u * 0.55 + 0.32 * rx, v * 0.55 + 0.32 * ry);
            let fx = (su * 0.5 + 0.5).clamp(0.0, 1.0) * (w - 1) as f32;
            let fy = (sv * 0.5 + 0.5).clamp(0.0, 1.0) * (h - 1) as f32;
            let (x0, y0) = (fx as usize, fy as usize);
            let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
            let (p00, p10, p01, p11) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
            let mut c = [0f32; 3];
            for k in 0..3 {
                let top = p00[k] + (p10[k] - p00[k]) * ax;
                let bottom = p01[k] + (p11[k] - p01[k]) * ax;
                c[k] = top + (bottom - top) * ay;
            }
            // Pushed away from grey, so the colours stand apart as they flow.
            let grey = (c[0] + c[1] + c[2]) / 3.0;
            let o = (y * n + x) * 4;
            for k in 0..3 {
                out[o + k] = (grey + (c[k] - grey) * 1.35).clamp(0.0, 255.0) as u8;
            }
            out[o + 3] = 255;
        }
    }
    // Where the flow folds over itself it leaves a crease. Two light
    // softenings melt those into the rest.
    let mut copy = vec![0u8; out.len()];
    for _ in 0..2 {
        copy.copy_from_slice(&out);
        for y in 0..n {
            for x in 0..n {
                let mut sum = [0u32; 3];
                let mut count = 0;
                for yy in y.saturating_sub(1)..(y + 2).min(n) {
                    for xx in x.saturating_sub(1)..(x + 2).min(n) {
                        let p = (yy * n + xx) * 4;
                        sum[0] += copy[p] as u32;
                        sum[1] += copy[p + 1] as u32;
                        sum[2] += copy[p + 2] as u32;
                        count += 1;
                    }
                }
                let o = (y * n + x) * 4;
                out[o] = (sum[0] / count) as u8;
                out[o + 1] = (sum[1] / count) as u8;
                out[o + 2] = (sum[2] / count) as u8;
            }
        }
    }
    out
}
