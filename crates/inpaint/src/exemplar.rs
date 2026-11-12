//! Exemplar-based inpainting that continues structures into the hole: A. Criminisi, P. Pérez,
//! K. Toyama, *Region Filling and Object Removal by Exemplar-Based Image Inpainting*, IEEE TIP 2004.
//!
//! The hole is filled from its border inwards, one patch at a time, in the order its *priority*
//! gives: confidence (how much of the patch is already known) times the data term (how strongly an
//! edge — an isophote — runs into the hole across its border). A pole, a cable or a horizon that
//! enters the hole is therefore extended first, along its own direction, before the flat
//! surroundings close over it; each step copies the unknown pixels of the most similar fully-known
//! patch (sum of squared differences over the known pixels).
//!
//! Used to seed [`crate::complete`] where thin structures cross the hole, which the smooth
//! membrane seed of the coarse levels would wash out.

use crate::Cancelled;

/// Exemplar-based fill of `hole` in the interleaved `w × h × ch` buffer `img` with `(2r+1)²`
/// patches. `Ok(None)` when no fully-known patch exists to copy from or the sizes do not match;
/// `Err(Cancelled)` once `cancelled` returned true (polled every step).
pub fn exemplar_fill(
    w: usize,
    h: usize,
    ch: usize,
    img: &[f32],
    hole: &[bool],
    r: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<f32>>, Cancelled> {
    if ch == 0 || Some(hole.len()) != w.checked_mul(h) || Some(img.len()) != hole.len().checked_mul(ch) {
        return Ok(None);
    }
    let r = r.max(1);
    if w < 2 * r + 1 || h < 2 * r + 1 {
        return Ok(None);
    }
    let mut out = img.to_vec();
    let mut filled: Vec<bool> = hole.iter().map(|b| !b).collect();
    if filled.iter().all(|f| *f) {
        return Ok(Some(out));
    }
    // sources: patch centres whose whole patch is known in the input
    let sum = integral(w, h, hole);
    let holes_in = |x0: usize, y0: usize, x1: usize, y1: usize| {
        let at = |x: usize, y: usize| sum[y * (w + 1) + x];
        at(x1, y1) + at(x0, y0) - at(x0, y1) - at(x1, y0)
    };
    let sources: Vec<(usize, usize)> =
        (r..h - r).flat_map(|y| (r..w - r).map(move |x| (x, y))).filter(|&(x, y)| holes_in(x - r, y - r, x + r + 1, y + r + 1) == 0).collect();
    if sources.is_empty() {
        return Ok(None);
    }
    // the shifts under which the known picture repeats (a stripe's period, a pole's own direction):
    // copies along them keep regular structures in step where fronts meet
    let shifts = dominant_offsets(w, h, ch, img, hole, 2 * r + 2);
    let mut conf: Vec<f32> = filled.iter().map(|f| if *f { 1.0 } else { 0.0 }).collect();
    // where each filled hole pixel was copied from (as a patch-centre offset), so neighbours can
    // keep copying the same region: coherent copies continue a structure instead of stitching
    // unrelated bits of it (stumps, stray ends)
    let mut offset: Vec<Option<(isize, isize)>> = vec![None; w * h];
    // luminance (channel mean) for the isophotes
    let lum = |o: &[f32], i: usize| o[i * ch..(i + 1) * ch].iter().sum::<f32>() / ch as f32;
    let mut remaining = hole.iter().filter(|b| **b).count();
    let area = ((2 * r + 1) * (2 * r + 1)) as f32;
    while remaining > 0 {
        if cancelled() {
            return Err(Cancelled);
        }
        // the fill front: unfilled pixels next to a filled one
        let mut best: Option<(f32, usize, usize)> = None;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if filled[i] || !neighbours4(w, h, x, y).any(|j| filled[j]) {
                    continue;
                }
                let (x0, y0, x1, y1) = (x.saturating_sub(r), y.saturating_sub(r), (x + r + 1).min(w), (y + r + 1).min(h));
                let mut c = 0.0f32;
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        if filled[yy * w + xx] {
                            c += conf[yy * w + xx];
                        }
                    }
                }
                let c = c / area;
                // normal to the front: gradient of the filled indicator
                let f = |xx: isize, yy: isize| -> f32 {
                    let (cx, cy) = (xx.clamp(0, w as isize - 1) as usize, yy.clamp(0, h as isize - 1) as usize);
                    if filled[cy * w + cx] { 1.0 } else { 0.0 }
                };
                let (xi, yi) = (x as isize, y as isize);
                let nx = f(xi + 1, yi) - f(xi - 1, yi);
                let ny = f(xi, yi + 1) - f(xi, yi - 1);
                let nl = (nx * nx + ny * ny).sqrt();
                // the strongest isophote among the known pixels of the patch (gradient turned 90°)
                let mut iso = (0.0f32, 0.0f32, 0.0f32);
                for yy in y0.max(1)..y1.min(h - 1) {
                    for xx in x0.max(1)..x1.min(w - 1) {
                        let j = yy * w + xx;
                        if !filled[j] || !filled[j - 1] || !filled[j + 1] || !filled[j - w] || !filled[j + w] {
                            continue;
                        }
                        let gx = (lum(&out, j + 1) - lum(&out, j - 1)) * 0.5;
                        let gy = (lum(&out, j + w) - lum(&out, j - w)) * 0.5;
                        let m = gx * gx + gy * gy;
                        if m > iso.2 {
                            iso = (-gy, gx, m);
                        }
                    }
                }
                let d = if nl > 0.0 { ((iso.0 * nx + iso.1 * ny) / nl).abs() } else { 0.0 };
                // a small floor so flat regions still progress in confidence order
                let p = c * (d + 1e-3);
                if best.is_none_or(|b| p > b.0) {
                    best = Some((p, x, y));
                }
            }
        }
        let Some((_, tx, ty)) = best else { break };
        let coherent: Vec<(usize, usize)> = {
            let mut v: Vec<(usize, usize)> = Vec::new();
            for &(ox, oy) in &shifts {
                let (cx, cy) = (tx as isize + ox, ty as isize + oy);
                if cx >= r as isize && cy >= r as isize && (cx as usize) < w - r && (cy as usize) < h - r {
                    let c = (cx as usize, cy as usize);
                    if holes_in(c.0 - r, c.1 - r, c.0 + r + 1, c.1 + r + 1) == 0 {
                        v.push(c);
                    }
                }
            }
            let (x0, y0, x1, y1) = (tx.saturating_sub(r), ty.saturating_sub(r), (tx + r + 1).min(w), (ty + r + 1).min(h));
            for yy in y0..y1 {
                for xx in x0..x1 {
                    if let Some((ox, oy)) = offset[yy * w + xx] {
                        let (cx, cy) = (tx as isize + ox, ty as isize + oy);
                        if cx >= r as isize && cy >= r as isize && (cx as usize) < w - r && (cy as usize) < h - r {
                            let c = (cx as usize, cy as usize);
                            if holes_in(c.0 - r, c.1 - r, c.0 + r + 1, c.1 + r + 1) == 0 && !v.contains(&c) {
                                v.push(c);
                            }
                        }
                    }
                }
            }
            v
        };
        let (sx, sy) = best_source(w, ch, &out, &filled, &sources, &coherent, tx, ty, r, h);
        let c = {
            let (x0, y0, x1, y1) = (tx.saturating_sub(r), ty.saturating_sub(r), (tx + r + 1).min(w), (ty + r + 1).min(h));
            let mut c = 0.0f32;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    if filled[yy * w + xx] {
                        c += conf[yy * w + xx];
                    }
                }
            }
            c / area
        };
        for dy in -(r as isize)..=r as isize {
            for dx in -(r as isize)..=r as isize {
                let (x, y) = (tx as isize + dx, ty as isize + dy);
                if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
                    continue;
                }
                let i = y as usize * w + x as usize;
                if filled[i] {
                    continue;
                }
                let s = (sy as isize + dy) as usize * w + (sx as isize + dx) as usize;
                let (d, s) = (i * ch, s * ch);
                for k in 0..ch {
                    out[d + k] = out[s + k];
                }
                filled[i] = true;
                conf[i] = c;
                offset[i] = Some((sx as isize - tx as isize, sy as isize - ty as isize));
                remaining -= 1;
            }
        }
    }
    Ok(Some(out))
}

/// The source patch centre most similar to the target patch at `(tx, ty)` over its filled pixels
/// (sum of squared differences, each candidate abandoned once it can't win), among the `coherent`
/// ones (along a dominant shift, or continuing a neighbour's copy) when there are any.
#[allow(clippy::too_many_arguments)]
fn best_source(
    w: usize,
    ch: usize,
    out: &[f32],
    filled: &[bool],
    sources: &[(usize, usize)],
    coherent: &[(usize, usize)],
    tx: usize,
    ty: usize,
    r: usize,
    h: usize,
) -> (usize, usize) {
    // the target's known offsets (clipped to the image)
    let mut known: Vec<(isize, isize)> = Vec::new();
    for dy in -(r as isize)..=r as isize {
        for dx in -(r as isize)..=r as isize {
            let (x, y) = (tx as isize + dx, ty as isize + dy);
            if x >= 0 && y >= 0 && x < w as isize && y < h as isize && filled[y as usize * w + x as usize] {
                known.push((dx, dy));
            }
        }
    }
    let score = |&(sx, sy): &(usize, usize), bound: f32| -> f32 {
        let mut e = 0.0f32;
        for &(dx, dy) in &known {
            let t = ((ty as isize + dy) as usize * w + (tx as isize + dx) as usize) * ch;
            let s = ((sy as isize + dy) as usize * w + (sx as isize + dx) as usize) * ch;
            for k in 0..ch {
                let d = out[t + k] - out[s + k];
                e += d * d;
            }
            if e >= bound {
                return e;
            }
        }
        // ties go to the nearer source (keeps the copy local)
        e + 1e-9 * ((sx.abs_diff(tx) + sy.abs_diff(ty)) as f32)
    };
    let search = |cands: &[(usize, usize)]| -> (f32, (usize, usize)) {
        let mut best = (f32::INFINITY, cands.first().copied().unwrap_or((tx, ty)));
        for c in cands {
            let e = score(c, best.0);
            if e < best.0 {
                best = (e, *c);
            }
        }
        best
    };
    let global = |cands: &[(usize, usize)]| -> (f32, (usize, usize)) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            if cands.len() >= 4096 {
                let chunk = cands.len().div_ceil(rayon::current_num_threads().max(1) * 4).max(1);
                return cands
                    .par_chunks(chunk)
                    .map(search)
                    .reduce(|| (f32::INFINITY, (tx, ty)), |a, b| if b.0 < a.0 || (b.0 == a.0 && b.1 < a.1) { b } else { a });
            }
        }
        search(cands)
    };
    // a copy along a dominant shift or continuing a neighbour's always wins: the best match anywhere
    // else is, on a noisy picture, mostly the luckiest noise (a stripe's flat white copied from a
    // label, a pole's stump into the sea); it is looked for only when no such copy is possible
    if coherent.is_empty() { global(sources).1 } else { search(coherent).1 }
}

/// Dominant shifts kept by [`dominant_offsets`].
const SHIFTS: usize = 16;
/// Kept shifts differ by more than this (px, along either axis).
const SHIFT_SEPARATION: isize = 3;
/// Known pixels sampled to score a shift.
const SHIFT_SAMPLES: usize = 2048;

/// The shifts `(dx, dy)` (at least `min_shift` long) under which the known part of the picture
/// best matches itself: the lowest local minima of the mean squared difference between a pixel and
/// the pixel one shift away, over a fixed sample of known pixels whose shifted partner is known too.
/// A regular structure (stripes, a fence, tiles) shows its period; a straight pole or cable, its own
/// direction. (After K. He, J. Sun, *Statistics of Patch Offsets for Image Completion*, ECCV 2012,
/// with the offsets scored directly instead of from patch matches.)
pub fn dominant_offsets(w: usize, h: usize, ch: usize, img: &[f32], hole: &[bool], min_shift: usize) -> Vec<(isize, isize)> {
    if ch == 0 || hole.len() != w * h || img.len() != hole.len() * ch {
        return Vec::new();
    }
    // the hole's surroundings decide: known pixels within half the hole's extent of its bounding box
    let (mut bx0, mut by0, mut bx1, mut by1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, _) in hole.iter().enumerate().filter(|(_, b)| **b) {
        let (x, y) = (i % w, i / w);
        (bx0, by0, bx1, by1) = (bx0.min(x), by0.min(y), bx1.max(x), by1.max(y));
    }
    if bx0 > bx1 {
        return Vec::new();
    }
    let reach = ((bx1 - bx0).max(by1 - by0) / 2).max(16);
    let near = |x: usize, y: usize| x + reach >= bx0 && x <= bx1 + reach && y + reach >= by0 && y <= by1 + reach;
    let known: Vec<usize> = (0..w * h).filter(|i| !hole[*i] && near(i % w, i / w)).collect();
    if known.len() < 64 {
        return Vec::new();
    }
    let step = (known.len() / SHIFT_SAMPLES).max(1);
    let sample: Vec<(isize, isize)> = known.iter().step_by(step).map(|i| ((i % w) as isize, (i / w) as isize)).collect();
    let lum = |x: isize, y: isize| {
        let i = (y as usize * w + x as usize) * ch;
        img[i..i + ch].iter().sum::<f32>() / ch as f32
    };
    let (mx, my) = ((w / 2) as isize, (h / 2) as isize);
    let (gw, gh) = (2 * mx as usize + 1, 2 * my as usize + 1);
    // the mean squared difference for every shift (NaN: too few pairs to tell)
    let row = |dy: isize| -> Vec<f32> {
        (-mx..=mx)
            .map(|dx| {
                let (mut e, mut n) = (0.0f32, 0usize);
                for &(x, y) in &sample {
                    let (sx, sy) = (x + dx, y + dy);
                    if sx < 0 || sy < 0 || sx >= w as isize || sy >= h as isize || hole[sy as usize * w + sx as usize] {
                        continue;
                    }
                    let d = lum(x, y) - lum(sx, sy);
                    e += d * d;
                    n += 1;
                }
                if n * 8 >= sample.len() { e / n as f32 } else { f32::NAN }
            })
            .collect()
    };
    #[cfg(not(target_arch = "wasm32"))]
    let cost: Vec<f32> = {
        use rayon::prelude::*;
        (-my..=my).into_par_iter().flat_map_iter(row).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let cost: Vec<f32> = (-my..=my).flat_map(row).collect();
    let at = |gx: usize, gy: usize| cost[gy * gw + gx];
    let mut minima: Vec<(f32, isize, isize)> = Vec::new();
    for gy in 1..gh.saturating_sub(1) {
        for gx in 1..gw.saturating_sub(1) {
            let c = at(gx, gy);
            let (dx, dy) = (gx as isize - mx, gy as isize - my);
            if c.is_nan() || ((dx * dx + dy * dy) as f32).sqrt() < min_shift as f32 {
                continue;
            }
            let is_min = (-1..=1isize).all(|j| {
                (-1..=1isize).all(|i| {
                    let v = at((gx as isize + i) as usize, (gy as isize + j) as usize);
                    (i == 0 && j == 0) || v.is_nan() || c <= v
                })
            });
            if is_min {
                minima.push((c, dx, dy));
            }
        }
    }
    minima.sort_by(|a, b| a.0.total_cmp(&b.0).then((a.1, a.2).cmp(&(b.1, b.2))));
    // one shift per neighbourhood: noise makes shallow minima next to a true one
    let mut kept: Vec<(isize, isize)> = Vec::new();
    for (_, dx, dy) in minima {
        if kept.iter().all(|&(kx, ky)| (kx - dx).abs().max((ky - dy).abs()) > SHIFT_SEPARATION) {
            kept.push((dx, dy));
            if kept.len() == SHIFTS {
                break;
            }
        }
    }
    kept
}

fn neighbours4(w: usize, h: usize, x: usize, y: usize) -> impl Iterator<Item = usize> {
    [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)]
        .into_iter()
        .filter(move |&(a, b)| a < w && b < h)
        .map(move |(a, b)| b * w + a)
}

/// Summed-area table of a boolean grid (`(w+1) × (h+1)`).
fn integral(w: usize, h: usize, m: &[bool]) -> Vec<u32> {
    let mut s = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += u32::from(m[y * w + x]);
            s[(y + 1) * (w + 1) + x + 1] = s[y * (w + 1) + x + 1] + row;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thin dark vertical pole on a flat grey field, with a square hole across it: the pole
    /// continues through the hole, the field stays flat.
    #[test]
    fn a_thin_pole_crossing_the_hole_is_continued() {
        let (w, h) = (80usize, 80usize);
        let pole = |x: usize| (39..=40).contains(&x);
        let img: Vec<f32> = (0..w * h).map(|i| if pole(i % w) { 0.1 } else { 0.7 }).collect();
        let hole: Vec<bool> = (0..w * h).map(|i| (i % w).abs_diff(40) <= 10 && (i / w).abs_diff(40) <= 10).collect();
        let mut damaged = img.clone();
        for (v, hl) in damaged.iter_mut().zip(&hole) {
            if *hl {
                *v = 0.5;
            }
        }
        let out = exemplar_fill(w, h, 1, &damaged, &hole, 3, &|| false).unwrap().unwrap();
        let err: f32 = out.iter().zip(&img).map(|(a, b)| (a - b).abs()).sum::<f32>() / hole.iter().filter(|b| **b).count() as f32;
        assert!(err < 0.02, "mean error in the hole {err}");
        // the pole's pixels in the middle of the hole are dark
        assert!(out[40 * w + 39] < 0.2 && out[40 * w + 40] < 0.2, "{} {}", out[40 * w + 39], out[40 * w + 40]);
    }

    /// Vertical stripes (period 13) with a round hole over several of them: the stripes run on
    /// through the hole in step, without the dark bridges where the fronts meet.
    #[test]
    fn stripes_crossing_the_hole_stay_in_step() {
        let (w, h) = (120usize, 120usize);
        let stripe = |x: usize| if (x % 13) < 6 { 0.15 } else { 0.8 };
        let img: Vec<f32> = (0..w * h).map(|i| stripe(i % w)).collect();
        let hole: Vec<bool> = (0..w * h).map(|i| ((i % w) as f32 - 60.0).hypot((i / w) as f32 - 60.0) < 22.0).collect();
        let mut damaged = img.clone();
        for (v, hl) in damaged.iter_mut().zip(&hole) {
            if *hl {
                *v = 0.5;
            }
        }
        // every dominant shift repeats the stripes exactly (a whole number of periods across)
        let shifts = dominant_offsets(w, h, 1, &damaged, &hole, 8);
        assert!(!shifts.is_empty() && shifts.iter().all(|(dx, _)| dx % 13 == 0), "{shifts:?}");
        let out = exemplar_fill(w, h, 1, &damaged, &hole, 3, &|| false).unwrap().unwrap();
        let bad = out.iter().zip(&img).zip(&hole).filter(|((a, b), hl)| **hl && (*a - *b).abs() > 0.1).count();
        assert!(bad == 0, "{bad} pixels of the hole off the stripes");
    }

    #[test]
    fn hostile_inputs_do_not_panic() {
        assert_eq!(exemplar_fill(4, 4, 1, &[0.0; 5], &[true; 16], 3, &|| false), Ok(None));
        assert_eq!(exemplar_fill(16, 16, 1, &[0.5; 256], &[true; 256], 3, &|| false), Ok(None));
        assert_eq!(exemplar_fill(3, 3, 1, &[0.5; 9], &[false; 9], 3, &|| false), Ok(None));
        assert_eq!(exemplar_fill(32, 32, 1, &[0.5; 1024], &[false; 1024], 3, &|| false), Ok(Some(vec![0.5; 1024])));
        let centre: Vec<bool> = (0..1024usize).map(|i| (i % 32).abs_diff(16) < 4 && (i / 32).abs_diff(16) < 4).collect();
        assert_eq!(exemplar_fill(32, 32, 1, &[0.5; 1024], &centre, 3, &|| true), Err(Cancelled));
    }
}
