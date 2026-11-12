//! 3D LUT profiles: `.cube` files (the plain-text Cube LUT format: `LUT_3D_SIZE`, optional
//! `DOMAIN_MIN` / `DOMAIN_MAX`, then size³ "r g b" lines with red changing fastest), applied to
//! the display-encoded output colour and blended by the profile amount. Profiles are registered
//! here by id (`lut:…`) and found by the finishing stage; LUT profiles render on the CPU.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// A 3D colour lookup table.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut3d {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub title: Option<String>,
}

impl Lut3d {
    /// Parse a `.cube` file.
    pub fn parse_cube(text: &str) -> Result<Lut3d, String> {
        let (mut size, mut title) = (0usize, None);
        let (mut min, mut max) = ([0.0f32; 3], [1.0f32; 3]);
        let mut data = Vec::new();
        let three = |rest: &str| -> Option<[f32; 3]> {
            let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        };
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            if let Some(r) = l.strip_prefix("TITLE") {
                title = Some(r.trim().trim_matches('"').to_string());
            } else if let Some(r) = l.strip_prefix("LUT_3D_SIZE") {
                size = r.trim().parse().map_err(|_| format!("bad LUT_3D_SIZE `{}`", r.trim()))?;
                if !(2..=256).contains(&size) {
                    return Err(format!("LUT_3D_SIZE {size} out of range"));
                }
            } else if l.starts_with("LUT_1D_SIZE") {
                return Err("1D LUTs are not supported (only 3D .cube files)".into());
            } else if let Some(r) = l.strip_prefix("DOMAIN_MIN") {
                min = three(r).ok_or("bad DOMAIN_MIN")?;
            } else if let Some(r) = l.strip_prefix("DOMAIN_MAX") {
                max = three(r).ok_or("bad DOMAIN_MAX")?;
            } else if l.starts_with(|c: char| c.is_ascii_alphabetic()) {
                // other keywords (LUT_IN_VIDEO_RANGE…) are ignored
            } else {
                data.push(three(l).ok_or_else(|| format!("bad line `{l}`"))?);
            }
        }
        if size == 0 {
            return Err("not a 3D .cube LUT (no LUT_3D_SIZE)".into());
        }
        if data.len() != size * size * size {
            return Err(format!("expected {} entries, found {}", size * size * size, data.len()));
        }
        Ok(Lut3d { size, data, domain_min: min, domain_max: max, title })
    }

    /// Look up `c` (trilinear).
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let s = (n - 1) as f32;
        let mut idx = [0usize; 3];
        let mut frac = [0.0f32; 3];
        for k in 0..3 {
            let span = (self.domain_max[k] - self.domain_min[k]).max(1e-6);
            let x = (((c[k] - self.domain_min[k]) / span).clamp(0.0, 1.0)) * s;
            let i = (x.floor() as usize).min(n - 2);
            idx[k] = i;
            frac[k] = x - i as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.data[r + n * (g + n * b)];
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let (r, g, b) = (idx[0], idx[1], idx[2]);
        let c00 = lerp(at(r, g, b), at(r + 1, g, b), frac[0]);
        let c10 = lerp(at(r, g + 1, b), at(r + 1, g + 1, b), frac[0]);
        let c01 = lerp(at(r, g, b + 1), at(r + 1, g, b + 1), frac[0]);
        let c11 = lerp(at(r, g + 1, b + 1), at(r + 1, g + 1, b + 1), frac[0]);
        lerp(lerp(c00, c10, frac[1]), lerp(c01, c11, frac[1]), frac[2])
    }
}

/// Weight of [`Lut3d::fit`]'s pull towards the identity, relative to its smoothness weight.
const RIDGE: f64 = 0.05;

impl Lut3d {
    /// The identity table of `size`³ entries.
    pub fn identity(size: usize) -> Lut3d {
        let s = (size.max(2) - 1) as f32;
        let n = size.max(2);
        let data = (0..n * n * n).map(|i| [(i % n) as f32 / s, ((i / n) % n) as f32 / s, (i / (n * n)) as f32 / s]).collect();
        Lut3d { size: n, data, domain_min: [0.0; 3], domain_max: [1.0; 3], title: None }
    }

    /// The `.cube` text of this table.
    pub fn to_cube(&self) -> String {
        let mut out = String::new();
        if let Some(t) = &self.title {
            out.push_str(&format!("TITLE \"{}\"\n", t.replace('"', "'")));
        }
        out.push_str(&format!("LUT_3D_SIZE {}\n", self.size));
        if self.domain_min != [0.0; 3] || self.domain_max != [1.0; 3] {
            let [a, b, c] = self.domain_min;
            let [d, e, f] = self.domain_max;
            out.push_str(&format!("DOMAIN_MIN {a} {b} {c}\nDOMAIN_MAX {d} {e} {f}\n"));
        }
        for p in &self.data {
            out.push_str(&format!("{:.6} {:.6} {:.6}\n", p[0], p[1], p[2]));
        }
        out
    }

    /// The `size`³ table (domain 0..1) that best maps each pair's input colour to its output colour, both
    /// display-encoded 0..1: least squares on the trilinear lookup, plus `smooth` times a second-difference penalty
    /// along each axis and a weak pull towards the identity, solved as a correction to the identity. Cells no pair reaches therefore stay close to the
    /// identity and colour transitions stay smooth (no posterisation from a sparse sample). `None` without pairs,
    /// with a non-finite value or an unsupported size.
    pub fn fit(pairs: &[([f32; 3], [f32; 3])], size: usize, smooth: f32) -> Option<Lut3d> {
        if pairs.is_empty() || !(2..=65).contains(&size) || !smooth.is_finite() || smooth < 0.0 {
            return None;
        }
        if pairs.iter().any(|(a, b)| a.iter().chain(b).any(|v| !v.is_finite())) {
            return None;
        }
        let n = size;
        let cells = n * n * n;
        let s = (n - 1) as f32;
        // the eight cells and weights of each pair's input
        let corners: Vec<([usize; 8], [f32; 8])> = pairs
            .iter()
            .map(|(x, _)| {
                let mut i0 = [0usize; 3];
                let mut t = [0.0f32; 3];
                for k in 0..3 {
                    let v = x[k].clamp(0.0, 1.0) * s;
                    i0[k] = (v.floor() as usize).min(n - 2);
                    t[k] = v - i0[k] as f32;
                }
                let mut idx = [0usize; 8];
                let mut w = [0.0f32; 8];
                for (c, (ix, wx)) in idx.iter_mut().zip(w.iter_mut()).enumerate() {
                    let (dr, dg, db) = (c & 1, (c >> 1) & 1, (c >> 2) & 1);
                    *ix = (i0[0] + dr) + n * ((i0[1] + dg) + n * (i0[2] + db));
                    let f = |d: usize, k: usize| if d == 1 { t[k] } else { 1.0 - t[k] };
                    *wx = f(dr, 0) * f(dg, 1) * f(db, 2);
                }
                (idx, w)
            })
            .collect();
        // the smoothness weight is per pair, so it means the same whatever the sample size
        let lambda = f64::from(smooth) * pairs.len() as f64 / 30_000.0;
        let ridge = lambda * RIDGE;
        let identity = Lut3d::identity(n);
        let mut data = identity.data.clone();
        for ch in 0..3 {
            // normal equations (AᵀA + λ LᵀL) δ = Aᵀ (y − A·identity), by conjugate gradients, matrix free
            let apply = |d: &[f64], out: &mut [f64]| {
                out.iter_mut().for_each(|o| *o = 0.0);
                for (idx, w) in &corners {
                    let v: f64 = idx.iter().zip(w).map(|(&i, &w)| d[i] * f64::from(w)).sum();
                    for (&i, &w) in idx.iter().zip(w) {
                        out[i] += v * f64::from(w);
                    }
                }
                let at = |r: usize, g: usize, b: usize| r + n * (g + n * b);
                for a in 0..n {
                    for b in 0..n {
                        for m in 0..n - 2 {
                            for axis in 0..3 {
                                let id = |k: usize| match axis {
                                    0 => at(m + k, a, b),
                                    1 => at(a, m + k, b),
                                    _ => at(a, b, m + k),
                                };
                                let (i0, i1, i2) = (id(0), id(1), id(2));
                                let l = d[i0] - 2.0 * d[i1] + d[i2];
                                out[i0] += lambda * l;
                                out[i1] -= 2.0 * lambda * l;
                                out[i2] += lambda * l;
                            }
                        }
                    }
                }
                // a weak pull towards the identity, so a trend the pairs show does not run on unchecked into
                // colours they never reach (it fades over a few cells)
                for (o, v) in out.iter_mut().zip(d) {
                    *o += ridge * v;
                }
            };
            let mut rhs = vec![0.0f64; cells];
            for ((idx, w), (x, y)) in corners.iter().zip(pairs) {
                let target = f64::from(y[ch]) - f64::from(x[ch].clamp(0.0, 1.0));
                for (&i, &w) in idx.iter().zip(w) {
                    rhs[i] += target * f64::from(w);
                }
            }
            let mut delta = vec![0.0f64; cells];
            let mut r = rhs.clone();
            let mut p = r.clone();
            let mut ap = vec![0.0f64; cells];
            let mut rr: f64 = r.iter().map(|v| v * v).sum();
            let stop = rr * 1e-12;
            for _ in 0..600 {
                if rr <= stop || rr == 0.0 {
                    break;
                }
                apply(&p, &mut ap);
                let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
                if pap <= 0.0 || !pap.is_finite() {
                    break;
                }
                let alpha = rr / pap;
                for i in 0..cells {
                    delta[i] += alpha * p[i];
                    r[i] -= alpha * ap[i];
                }
                let next: f64 = r.iter().map(|v| v * v).sum();
                let beta = next / rr;
                rr = next;
                for i in 0..cells {
                    p[i] = r[i] + beta * p[i];
                }
            }
            for (cell, d) in data.iter_mut().zip(&delta) {
                cell[ch] = (cell[ch] + *d as f32).clamp(0.0, 1.0);
            }
        }
        data.iter().all(|p| p.iter().all(|v| v.is_finite())).then_some(Lut3d { size: n, data, ..identity })
    }
}

fn registry() -> &'static RwLock<HashMap<String, Arc<Lut3d>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<Lut3d>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Make a LUT available to renders under profile id `id` (`lut:…`).
pub fn register(id: &str, lut: Lut3d) {
    registry().write().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), Arc::new(lut));
}

/// The LUT registered for profile `id`.
pub fn get(id: &str) -> Option<Arc<Lut3d>> {
    if !id.starts_with("lut:") {
        return None;
    }
    registry().read().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
}

/// Forget a LUT profile.
pub fn unregister(id: &str) {
    registry().write().unwrap_or_else(|e| e.into_inner()).remove(id);
}

/// Profile ids that render with a LUT (and so on the CPU).
pub fn is_lut_profile(id: &str) -> bool {
    id.starts_with("lut:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fitted_table_reproduces_a_smooth_mapping_and_round_trips_as_cube() {
        // a camera-like look: a contrast curve, warmer highlights, less saturated blues
        let look = |c: [f32; 3]| {
            let s = |v: f32| v * v * (3.0 - 2.0 * v);
            let l = 0.3 * c[0] + 0.6 * c[1] + 0.1 * c[2];
            [s(c[0]) + 0.04 * l, s(c[1]), 0.8 * s(c[2]) + 0.2 * l - 0.02 * l]
        };
        let mut pairs = Vec::new();
        let mut seed = 7u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..20_000 {
            let c = [next(), next(), next()];
            pairs.push((c, look(c)));
        }
        let lut = Lut3d::fit(&pairs, 17, 1.0).unwrap();
        let mut worst = 0.0f32;
        for _ in 0..2000 {
            let c = [next(), next(), next()];
            let (a, b) = (lut.apply(c), look(c));
            worst = worst.max((0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max));
        }
        assert!(worst < 0.05, "largest error {worst}");
        let back = Lut3d::parse_cube(&lut.to_cube()).unwrap();
        assert_eq!(back.size, 17);
        assert!(back.data.iter().zip(&lut.data).all(|(a, b)| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5)));
    }

    #[test]
    fn colours_the_pairs_never_reach_stay_near_the_identity() {
        // only dark greys are sampled; bright saturated colours must not be pulled anywhere
        let pairs: Vec<_> = (0..500)
            .map(|i| {
                let v = i as f32 / 2000.0;
                ([v; 3], [v * 1.2; 3])
            })
            .collect();
        let lut = Lut3d::fit(&pairs, 9, 1.0).unwrap();
        let far = lut.apply([1.0, 0.1, 0.9]);
        assert!((far[0] - 1.0).abs() < 0.05 && (far[1] - 0.1).abs() < 0.05 && (far[2] - 0.9).abs() < 0.05, "{far:?}");
        assert!((lut.apply([0.2; 3])[0] - 0.24).abs() < 0.02);
        assert!(Lut3d::fit(&[], 9, 1.0).is_none());
        assert!(Lut3d::fit(&[([f32::NAN; 3], [0.0; 3])], 9, 1.0).is_none());
        assert!(Lut3d::fit(&pairs, 1, 1.0).is_none());
    }

    fn cube(f: impl Fn([f32; 3]) -> [f32; 3], n: usize) -> String {
        let mut s = format!("TITLE \"test\"\nLUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let c = f([r as f32 / (n - 1) as f32, g as f32 / (n - 1) as f32, b as f32 / (n - 1) as f32]);
                    s.push_str(&format!("{} {} {}\n", c[0], c[1], c[2]));
                }
            }
        }
        s
    }

    #[test]
    fn identity_and_swap() {
        let id = Lut3d::parse_cube(&cube(|c| c, 17)).unwrap();
        assert_eq!(id.title.as_deref(), Some("test"));
        let v = id.apply([0.2, 0.55, 0.9]);
        assert!(v.iter().zip([0.2, 0.55, 0.9]).all(|(a, b)| (a - b).abs() < 1e-5), "{v:?}");
        let swap = Lut3d::parse_cube(&cube(|c| [c[2], c[1], c[0]], 9)).unwrap();
        let v = swap.apply([0.1, 0.5, 0.8]);
        assert!((v[0] - 0.8).abs() < 1e-5 && (v[2] - 0.1).abs() < 1e-5, "{v:?}");
        assert!(Lut3d::parse_cube("LUT_1D_SIZE 4\n0 0 0").is_err());
        assert!(Lut3d::parse_cube("LUT_3D_SIZE 2\n0 0 0\n").is_err(), "too few entries");
    }
}
