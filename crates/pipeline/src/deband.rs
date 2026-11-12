//! LED banding removal (Detail ▸ LED Banding).
//!
//! A light that flickers with the mains (LEDs, fluorescent tubes: 100 or 120 Hz) photographed with
//! a rolling shutter (the electronic shutter of a mirrorless camera, or video) leaves dark and light
//! bands across the frame, one per flicker cycle, along the sensor's rows. They are periodic and
//! multiplicative: every row is lit a little more or less, by the same factor across the frame.
//!
//! [`estimate`] finds them on a small render of the whole frame: the log of each row's (and each
//! column's) mean light, its residual against a moving average one period long (which removes a
//! periodic component of exactly that period), folded modulo the period. The period that explains
//! most of the residual wins, among those a flicker can have (2.5 to 24 cycles per frame: shorter
//! scene patterns such as a curtain's folds are left alone); the axis is the one with the stronger
//! bands. The folded mean is the bands' waveform; [`apply`] divides it out, at the Amount given.
//! Nothing is changed when no periodic banding stands out of the scene.

use lightcraft_raster::Rgb32f;

use crate::geometry::Frame;

/// Long edge of the whole-frame render the bands are measured on.
const PROXY_EDGE: usize = 1024;
/// Fewest and most flicker cycles across the frame.
const MIN_CYCLES: f64 = 2.5;
const MAX_CYCLES: f64 = 24.0;
/// Most bins of the folded waveform (one period); it has one per sample of the period below that.
const BINS: usize = 256;
/// Fewest bins of the waveform.
const MIN_BINS: usize = 16;
/// Bins while looking for the period (a fixed number, so periods are compared fairly).
const SEARCH_BINS: usize = 32;
/// Bands are corrected only when the period explains at least this share of the residual's
/// variance and swings at least this much (EV, peak to peak).
const MIN_EXPLAINED: f64 = 0.25;
const MIN_SWING: f64 = 0.04;

/// Bands found on a frame: along output rows (`rows`) or columns, with a period and waveform in
/// normalized output coordinates (0..1 across the whole frame).
#[derive(Clone, Debug, PartialEq)]
pub struct Bands {
    pub rows: bool,
    /// Period, as a fraction of the frame's height (rows) or width (columns).
    pub period: f64,
    /// Log2 light of each phase bin relative to the fully lit phase (EV, 0 or less), over one period.
    pub wave: Vec<f32>,
    /// Share of the residual variance the period explains (diagnostics).
    pub explained: f64,
}

impl Bands {
    /// The gain (linear) that removes the bands at normalized position `t` along the axis, at
    /// `amount` (0..1).
    pub fn gain(&self, t: f64, amount: f32) -> f32 {
        (amount * self.ev(t)).exp2()
    }

    /// The light (EV, 0 or more) the bands took away at normalized position `t` along the axis.
    pub fn ev(&self, t: f64) -> f32 {
        let n = self.wave.len().max(1);
        let ph = (t / self.period).rem_euclid(1.0) * n as f64 - 0.5;
        let i = ph.floor();
        let f = (ph - i) as f32;
        let at = |k: f64| self.wave.get((k as i64).rem_euclid(n as i64) as usize).copied().unwrap_or(0.0);
        -(at(i) * (1.0 - f) + at(i + 1.0) * f)
    }

    /// Peak-to-peak swing of the waveform (EV).
    pub fn swing(&self) -> f32 {
        let (lo, hi) = self.wave.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| (a.min(*v), b.max(*v)));
        hi - lo
    }
}

/// Measure the bands of `src` as framed by `frame` into a `full_w × full_h` output (see the module
/// docs). `None` when there are none to speak of.
pub fn estimate(src: &Rgb32f, frame: &Frame, full_w: usize, full_h: usize) -> Option<Bands> {
    let k = (PROXY_EDGE as f64 / full_w.max(full_h).max(1) as f64).min(1.0);
    let (pw, ph) = (((full_w as f64 * k).round() as usize).max(1), ((full_h as f64 * k).round() as usize).max(1));
    let proxy = frame.sample(src, pw, ph);
    estimate_image(&proxy)
}

/// [`estimate`] on an image already framed as the output.
pub fn estimate_image(img: &Rgb32f) -> Option<Bands> {
    let (w, h) = (img.width, img.height);
    if w < 16 || h < 16 {
        return None;
    }
    let lum: Vec<f32> = img.data.iter().map(|c| (0.2627 * c[0] + 0.6780 * c[1] + 0.0593 * c[2]).max(0.0)).collect();
    // light sources and clipped areas would dominate the rows they sit in: cap at the 99th percentile
    let cap = {
        let mut v: Vec<f32> = lum.iter().copied().filter(|v| v.is_finite()).collect();
        if v.is_empty() {
            return None;
        }
        let n = ((v.len() as f64 * 0.99) as usize).min(v.len() - 1);
        *v.select_nth_unstable_by(n, f32::total_cmp).1
    };
    if cap <= 0.0 {
        return None;
    }
    let eps = cap * 1e-4;
    let mut rows = vec![0.0f64; h];
    let mut cols = vec![0.0f64; w];
    for y in 0..h {
        for x in 0..w {
            let v = f64::from(lum[y * w + x].min(cap));
            rows[y] += v;
            cols[x] += v;
        }
    }
    let rows: Vec<f64> = rows.iter().map(|s| (s / w as f64 + f64::from(eps)).log2()).collect();
    let cols: Vec<f64> = cols.iter().map(|s| (s / h as f64 + f64::from(eps)).log2()).collect();
    let r = fit(&rows).map(|(p, wave, e)| Bands { rows: true, period: p / h as f64, wave, explained: e });
    let c = fit(&cols).map(|(p, wave, e)| Bands { rows: false, period: p / w as f64, wave, explained: e });
    let best = match (r, c) {
        (Some(a), Some(b)) => Some(if a.swing() as f64 * a.explained >= b.swing() as f64 * b.explained { a } else { b }),
        (a, b) => a.or(b),
    }?;
    (best.explained >= MIN_EXPLAINED && best.swing() as f64 >= MIN_SWING).then_some(best)
}

/// The period (in samples) of the strongest periodic component of profile `m` among the flicker
/// periods, its waveform and the share of the residual it explains.
fn fit(m: &[f64]) -> Option<(f64, Vec<f32>, f64)> {
    let n = m.len();
    let (lo, hi) = ((n as f64 / MAX_CYCLES).max(6.0), n as f64 / MIN_CYCLES);
    if lo >= hi {
        return None;
    }
    let mut cum = vec![0.0f64; n + 1];
    for (i, v) in m.iter().enumerate() {
        cum[i + 1] = cum[i] + v;
    }
    // the profile's integral at a fractional position (linear between samples)
    let at = |x: f64| {
        let x = x.clamp(0.0, n as f64);
        let i = (x.floor() as usize).min(n - 1);
        cum[i] + (x - i as f64) * (cum[i + 1] - cum[i])
    };
    let score = |p: f64, bins: usize| -> Option<(f64, Vec<f32>)> {
        let half = p / 2.0;
        let (i0, i1) = (half.ceil() as usize, n.saturating_sub(half.ceil() as usize));
        if i1 <= i0 + (2.0 * p) as usize {
            return None;
        }
        let mut sum = vec![0.0f64; bins];
        let mut cnt = vec![0usize; bins];
        let mut var = 0.0f64;
        let mut all = 0.0f64;
        for i in i0..i1 {
            let c = i as f64 + 0.5;
            let r = m[i] - (at(c + half) - at(c - half)) / p;
            let b = (((c / p).rem_euclid(1.0)) * bins as f64) as usize % bins;
            sum[b] += r;
            cnt[b] += 1;
            var += r * r;
            all += r;
        }
        let count = (i1 - i0) as f64;
        let mean = all / count;
        let total = var / count - mean * mean;
        if total <= 1e-12 || cnt.contains(&0) {
            return None;
        }
        let wave: Vec<f64> = sum.iter().zip(&cnt).map(|(s, c)| s / *c as f64 - mean).collect();
        let explained: f64 = wave.iter().zip(&cnt).map(|(w, c)| w * w * *c as f64).sum::<f64>() / count / total;
        Some((explained, wave.iter().map(|v| *v as f32).collect()))
    };
    // coarse search, then refine around the best
    let mut best: Option<(f64, f64, Vec<f32>)> = None;
    let consider = |p: f64, best: &mut Option<(f64, f64, Vec<f32>)>| {
        if let Some((e, w)) = score(p, SEARCH_BINS)
            && best.as_ref().is_none_or(|b| e > b.1)
        {
            *best = Some((p, e, w));
        }
    };
    let mut p = lo;
    while p <= hi {
        consider(p, &mut best);
        p += 0.25;
    }
    let p0 = best.as_ref()?.0;
    let mut p = p0 - 0.3;
    while p <= p0 + 0.3 {
        consider(p, &mut best);
        p += 0.01;
    }
    let (mut p, e, w) = best?;
    // anything periodic folds just as well at a multiple of its period: take the shortest period that
    // explains as much; one shorter than a flicker's (a curtain's folds, tiles) is the scene's own
    if let Some(q) = (2..=12).rev().map(|k| p / k as f64).find(|&q| {
        let bins = (q.floor() as usize).clamp(2, SEARCH_BINS);
        q >= 2.0 && score(q, bins).is_some_and(|(eq, _)| eq >= 0.9 * e)
    }) {
        if q < lo {
            return None;
        }
        p = q;
    }
    // the waveform in full: one bin per sample of the period, so a band's edges stay sharp
    let w = score(p, (p.round() as usize).clamp(MIN_BINS, BINS)).map_or(w, |(_, w)| w);
    // the flicker takes light away: the fully lit rows are the reference and stay as they are, the
    // others are brought up to them (the level of the lit phase: its brightest bins, smoothed so one
    // noisy bin does not set it)
    let n = w.len();
    let lit = (0..n).map(|b| (w[(b + n - 1) % n] + w[b] + w[(b + 1) % n]) / 3.0).fold(f32::NEG_INFINITY, f32::max);
    Some((p, w.iter().map(|v| (v - lit).min(0.0)).collect(), e))
}

/// Gain (EV) up to which a line is trusted, and past which it is taken as lost: the light was off
/// while it was read and what is left is noise. Lost lines are rebuilt from the trusted lines either
/// side; lines in between are blended towards that.
const TRUST_EV: f32 = 2.0;
const LOST_EV: f32 = 3.0;

/// Divide the bands out of the (output-resolution) image `img`, which shows the window of `frame`
/// (or the whole frame), at `amount` (0..1); lines the light never reached are rebuilt from their
/// neighbours across the band.
pub fn apply(img: &mut Rgb32f, frame: &Frame, bands: &Bands, amount: f32) {
    let (w, h) = (img.width, img.height);
    let [vx, vy, vw, vh] = frame.view.map_or([0.0, 0.0, w as f64, h as f64], |v| [v.x, v.y, v.full_w, v.full_h]);
    let (len, start, full) = if bands.rows { (h, vy, vh) } else { (w, vx, vw) };
    let ev: Vec<f32> = (0..len).map(|i| amount * bands.ev((start + i as f64 + 0.5) / full)).collect();
    let gains: Vec<f32> = ev.iter().map(|e| e.min(LOST_EV).exp2()).collect();
    let trust: Vec<f32> = ev
        .iter()
        .map(|e| {
            let t = ((e - TRUST_EV) / (LOST_EV - TRUST_EV)).clamp(0.0, 1.0);
            1.0 - t * t * (3.0 - 2.0 * t)
        })
        .collect();
    // the nearest fully trusted line at or before / at or after each line
    let mut before = vec![None; len];
    let mut after = vec![None; len];
    for i in 0..len {
        before[i] = if trust[i] >= 1.0 { Some(i) } else { i.checked_sub(1).and_then(|j| before[j]) };
    }
    for i in (0..len).rev() {
        after[i] = if trust[i] >= 1.0 { Some(i) } else { after.get(i + 1).copied().flatten() };
    }
    let rebuilt = |i: usize, line: &dyn Fn(usize) -> [f32; 3]| -> Option<[f32; 3]> {
        match (before[i], after[i]) {
            (Some(a), Some(b)) if b > a => {
                let f = (i - a) as f32 / (b - a) as f32;
                let (pa, pb) = (line(a), line(b));
                Some([0, 1, 2].map(|k| pa[k] + (pb[k] - pa[k]) * f))
            }
            (Some(a), _) | (_, Some(a)) => Some(line(a)),
            (None, None) => None,
        }
    };
    if bands.rows {
        crate::for_rows(&mut img.data, w, |y, row| {
            let g = gains[y];
            for px in row.iter_mut() {
                *px = px.map(|v| v * g);
            }
        });
        for y in (0..h).filter(|y| trust[*y] < 1.0) {
            let t = trust[y];
            for x in 0..w {
                let src = &img.data;
                if let Some(r) = rebuilt(y, &|j| src[j * w + x]) {
                    let px = img.data[y * w + x];
                    img.data[y * w + x] = [0, 1, 2].map(|k| px[k] * t + r[k] * (1.0 - t));
                }
            }
        }
    } else {
        crate::for_rows(&mut img.data, w, |_, row| {
            for (px, g) in row.iter_mut().zip(&gains) {
                *px = px.map(|v| v * g);
            }
            for x in (0..w).filter(|x| trust[*x] < 1.0) {
                let t = trust[x];
                if let Some(r) = rebuilt(x, &|j| row[j]) {
                    let px = row[x];
                    row[x] = [0, 1, 2].map(|k| px[k] * t + r[k] * (1.0 - t));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 600;
    const H: usize = 400;

    /// A scene with a soft gradient, vertical folds (a curtain: short period along x) and a lamp.
    fn scene(x: usize, y: usize) -> f32 {
        let folds = 1.0 + 0.3 * ((x as f32) * std::f32::consts::TAU / 9.0).sin();
        let lamp = if (x as isize - 500).abs() < 8 && (y as isize - 30).abs() < 4 { 40.0 } else { 1.0 };
        0.05 + 0.25 * (x as f32 / W as f32) * folds * lamp
    }

    /// Narrow dark bands (LED off-phase), 6.3 per frame height.
    fn band(y: usize) -> f32 {
        let ph = (y as f32 / (H as f32 / 6.3)).fract();
        if ph < 0.18 { 0.6 } else { 1.0 }
    }

    /// Each row's mean light (log2), left of the lamp.
    fn row_profile(img: &Rgb32f) -> Vec<f32> {
        (0..img.height).map(|y| (img.data[y * img.width..y * img.width + 450].iter().map(|c| c[1]).sum::<f32>() / 450.0).log2()).collect()
    }

    /// Mean distance (EV) of the profile from its own 1/6-height moving average.
    fn swing(p: &[f32]) -> f32 {
        let n = p.len();
        let r = n / 12;
        (r..n - r).map(|i| (p[i] - p[i - r..=i + r].iter().sum::<f32>() / (2 * r + 1) as f32).abs()).sum::<f32>() / (n - 2 * r) as f32
    }

    #[test]
    fn led_bands_are_found_and_removed_and_the_curtain_folds_kept() {
        let banded = Rgb32f::from_fn(W, H, |x, y| [scene(x, y) * band(y); 3]);
        let b = estimate_image(&banded).expect("bands found");
        assert!(b.rows, "{b:?}");
        assert!((b.period * H as f64 - H as f64 / 6.3).abs() < 1.0, "period {}", b.period * H as f64);
        assert!(b.swing() > 0.4, "swing {}", b.swing());
        let mut fixed = banded.clone();
        let frame = Frame::new(W, H, &lightcraft_develop::DevelopSettings::default(), true);
        apply(&mut fixed, &frame, &b, 1.0);
        let (before, after) = (swing(&row_profile(&banded)), swing(&row_profile(&fixed)));
        assert!(after < 0.1 * before, "bands {before} -> {after}");
        // the fully lit rows keep their light; the bands are brought up to it
        let (lit, band) = (fixed.data[40 * W + 100][1] / banded.data[40 * W + 100][1], fixed.data[2 * W + 100][1] / banded.data[2 * W + 100][1]);
        assert!((lit - 1.0).abs() < 0.03 && (band - 1.0 / 0.6).abs() < 0.1, "lit row x{lit}, band row x{band}");
        // the folds (along x) are untouched: each row keeps its shape up to a gain
        let y = 200;
        let ratio: Vec<f32> = (0..W).map(|x| fixed.data[y * W + x][1] / banded.data[y * W + x][1]).collect();
        assert!(ratio.iter().all(|r| (r - ratio[0]).abs() < 1e-4));
    }

    #[test]
    fn lines_the_light_never_reached_are_rebuilt_from_their_neighbours() {
        // the LED fully off for part of each cycle (the shutter's slit softens the band's edges): those
        // rows hold almost nothing
        let off = |y: usize| {
            let ph = (y as f32 / (H as f32 / 6.3)).fract();
            if ph < 0.25 { 0.002f32.powf((ph / 0.25 * std::f32::consts::PI).sin().powi(2)) } else { 1.0 }
        };
        let banded = Rgb32f::from_fn(W, H, |x, y| [scene(x, y) * off(y); 3]);
        let b = estimate_image(&banded).expect("bands found");
        let mut fixed = banded.clone();
        let frame = Frame::new(W, H, &lightcraft_develop::DevelopSettings::default(), true);
        apply(&mut fixed, &frame, &b, 1.0);
        // an off row comes back near the scene, not black and not blown up
        let y = (0..H).find(|y| off(*y) < 0.01 && *y > 100).unwrap() + 1;
        for x in [50, 200, 400] {
            let (got, want) = (fixed.data[y * W + x][1], scene(x, y));
            assert!((got / want - 1.0).abs() < 0.15, "row {y} x {x}: {got} vs {want}");
        }
        assert!(fixed.data.iter().all(|c| c.iter().all(|v| v.is_finite() && *v < 20.0)));
    }

    #[test]
    fn a_scene_without_bands_is_left_alone() {
        let plain = Rgb32f::from_fn(W, H, |x, y| [scene(x, y) * (0.6 + 0.4 * y as f32 / H as f32); 3]);
        assert_eq!(estimate_image(&plain), None);
        assert_eq!(estimate_image(&Rgb32f::from_fn(W, H, |_, _| [0.2; 3])), None);
        assert_eq!(estimate_image(&Rgb32f::new(8, 8)), None);
    }

    #[test]
    fn bands_across_a_portrait_frame_are_found_on_the_columns() {
        // the same bands with the frame turned: they run along x
        let banded = Rgb32f::from_fn(H, W, |x, y| [scene(y, x) * band(x); 3]);
        let b = estimate_image(&banded).expect("bands found");
        assert!(!b.rows, "{b:?}");
    }
}
