//! Pixel tests of the local (mask) Noise, Moiré and Defringe sliders.

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::Point;
use lightcraft_raster::{Rgb32f, Rgba8};

use crate::{RenderRequest, SourceInfo, render};

const W: usize = 240;
const H: usize = 120;

/// A deterministic hash in −1..1.
fn noise(x: usize, y: usize, k: u32) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ k.wrapping_mul(0xcb1a_b31f);
    v ^= v >> 13;
    v = v.wrapping_mul(0x5bd1_e995);
    v ^= v >> 15;
    (v & 0xffff) as f32 / 32768.0 - 1.0
}

/// Settings with one mask covering the left half (x < 0.45), with `adjust`.
fn left_mask(adjust: LocalAdjustments) -> DevelopSettings {
    let shape = MaskShape::Linear { start: Point::new(0.45, 0.5), end: Point::new(0.46, 0.5) };
    let mut s = DevelopSettings::default();
    s.masks.push(Mask { id: 1, components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], adjust, ..Default::default() });
    s
}

fn shot(src: &Rgb32f, s: &DevelopSettings) -> Rgba8 {
    render(src, &SourceInfo::default(), s, &RenderRequest::fit(W, H)).image
}

/// Standard deviation of `f(pixel)` over columns `x0..x1` (rows 4..H-4).
fn spread(img: &Rgba8, x0: usize, x1: usize, f: impl Fn([u8; 4]) -> f32) -> f32 {
    let v: Vec<f32> = (4..H - 4).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| f(img.data[y * W + x])).collect();
    let m = v.iter().sum::<f32>() / v.len() as f32;
    (v.iter().map(|a| (a - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
}

fn luma(p: [u8; 4]) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

fn chroma(p: [u8; 4]) -> f32 {
    p[0] as f32 - p[2] as f32
}

#[test]
fn local_noise_smooths_luminance_noise_inside_the_mask() {
    let src = Rgb32f::from_fn(W, H, |x, y| [0.18 * (1.0 + 0.12 * noise(x, y, 1)); 3]);
    let plain = shot(&src, &DevelopSettings::default());
    let base = spread(&plain, 10, 90, luma);
    let on = shot(&src, &left_mask(LocalAdjustments { noise: 100.0, ..Default::default() }));
    let off = shot(&src, &left_mask(LocalAdjustments { noise: -100.0, ..Default::default() }));
    let (smoothed, boosted) = (spread(&on, 10, 90, luma), spread(&off, 10, 90, luma));
    assert!(smoothed < 0.6 * base, "noise +100: {smoothed} vs {base}");
    assert!(boosted > 1.2 * base, "noise −100: {boosted} vs {base}");
    // outside the mask nothing changes
    assert_eq!(spread(&on, 150, 230, luma), spread(&plain, 150, 230, luma));
    // the mean brightness is kept
    let mean = |img: &Rgba8| (4..H - 4).flat_map(|y| (10..90).map(move |x| luma(img.data[y * W + x]))).sum::<f32>() / (80 * (H - 8)) as f32;
    assert!((mean(&on) - mean(&plain)).abs() < 2.0);
}

#[test]
fn local_moire_removes_colour_speckle_inside_the_mask() {
    let src = Rgb32f::from_fn(W, H, |x, y| {
        let n = 0.25 * noise(x, y, 2);
        [0.18 * (1.0 + n), 0.18, 0.18 * (1.0 - n)]
    });
    let plain = shot(&src, &DevelopSettings::default());
    let base = spread(&plain, 10, 90, chroma);
    let on = shot(&src, &left_mask(LocalAdjustments { moire: 100.0, ..Default::default() }));
    let reduced = spread(&on, 10, 90, chroma);
    assert!(reduced < 0.4 * base, "moiré +100: {reduced} vs {base}");
    assert_eq!(spread(&on, 150, 230, chroma), spread(&plain, 150, 230, chroma));
    // luminance is kept
    assert!((spread(&on, 10, 90, luma) - spread(&plain, 10, 90, luma)).abs() < 1.0);
    // positive Noise also calms colour noise (less than Moiré)
    let n = spread(&shot(&src, &left_mask(LocalAdjustments { noise: 100.0, ..Default::default() })), 10, 90, chroma);
    assert!(n < 0.8 * base && n > reduced, "{n}");
}

#[test]
fn local_defringe_desaturates_purple_edges_only() {
    // dark | bright edge at x = 40 (inside the mask) and x = 200 (outside), each with a purple
    // fringe column; a flat purple patch (no edge) at x 60..80
    let src = Rgb32f::from_fn(W, H, |x, _| {
        let fringe = x == 39 || x == 40 || x == 199 || x == 200;
        if fringe {
            [0.25, 0.04, 0.3]
        } else if (60..80).contains(&x) {
            [0.2, 0.08, 0.22]
        } else if (40..120).contains(&x) || x >= 200 {
            [0.7; 3]
        } else {
            [0.02; 3]
        }
    });
    let s = left_mask(LocalAdjustments { defringe: 100.0, ..Default::default() });
    let (plain, on) = (shot(&src, &DevelopSettings::default()), shot(&src, &s));
    let sat = |p: [u8; 4]| p[0].max(p[2]) as f32 - p[1] as f32;
    let at = |img: &Rgba8, x: usize| img.data[60 * W + x];
    assert!(sat(at(&plain, 40)) > 40.0, "a purple fringe to start with: {:?}", at(&plain, 40));
    assert!(sat(at(&on, 40)) < 0.5 * sat(at(&plain, 40)), "fringe in the mask: {:?} vs {:?}", at(&on, 40), at(&plain, 40));
    assert_eq!(at(&on, 200), at(&plain, 200), "outside the mask");
    let flat = |img: &Rgba8| sat(at(img, 70));
    assert!((flat(&on) - flat(&plain)).abs() <= 2.0, "flat purple kept: {} vs {}", flat(&on), flat(&plain));
}

/// A lamp many stops over white, in the mask and out of it: Bright Lights brings the one in the
/// mask under white, where local Exposure alone (the photographer's −2.5 EV) leaves it clipped;
/// what lies under the knee is untouched.
#[test]
fn local_bright_lights_bring_a_lamp_under_white() {
    let lamp = |x: usize, y: usize| (y as isize - 60).abs() < 10 && ((x as isize - 50).abs() < 10 || (x as isize - 190).abs() < 10);
    let src = Rgb32f::from_fn(W, H, |x, y| if lamp(x, y) { [0.18 * 2f32.powi(9); 3] } else { [0.05; 3] });
    let info = SourceInfo { raw: true, ..Default::default() };
    let shot = |s: &DevelopSettings| render(&src, &info, s, &RenderRequest::fit(W, H)).image;
    let at = |img: &Rgba8, x: usize, y: usize| luma(img.data[y * W + x]);
    let exposure_only = shot(&left_mask(LocalAdjustments { exposure: -2.5, ..Default::default() }));
    assert!(at(&exposure_only, 50, 60) > 250.0, "exposure alone keeps the lamp clipped: {}", at(&exposure_only, 50, 60));
    let lights = shot(&left_mask(LocalAdjustments { exposure: -2.5, lights: 100.0, ..Default::default() }));
    let (inside, outside) = (at(&lights, 50, 60), at(&lights, 190, 60));
    assert!(inside < 235.0 && inside > 150.0, "the lamp in the mask prints bright but under white: {inside}");
    assert!(outside > 250.0, "the lamp outside the mask is untouched: {outside}");
    // the dark surround (under the knee) is the same as with exposure alone
    assert_eq!(at(&lights, 20, 20), at(&exposure_only, 20, 20));
}

#[test]
fn local_bright_lights_bring_a_coloured_lamp_under_white_with_its_colour() {
    // a warm neon: the red channel far over white, the luminance much lower
    let lamp = |x: usize, y: usize| (y as isize - 60).abs() < 10 && (x as isize - 50).abs() < 10;
    let src = Rgb32f::from_fn(W, H, |x, y| if lamp(x, y) { [0.18 * 2f32.powi(8), 0.18 * 2f32.powi(5), 0.18 * 2f32.powi(4)] } else { [0.05; 3] });
    let info = SourceInfo { raw: true, ..Default::default() };
    let shot = |s: &DevelopSettings| render(&src, &info, s, &RenderRequest::fit(W, H)).image;
    let red = |img: &Rgba8| img.data[60 * W + 50][0];
    let plain = shot(&left_mask(LocalAdjustments::default()));
    assert!(red(&plain) >= 254, "the neon clips red: {}", red(&plain));
    let lit = shot(&left_mask(LocalAdjustments { lights: 100.0, ..Default::default() }));
    let px = lit.data[60 * W + 50];
    assert!(px[0] < 246, "the neon's red comes under white: {px:?}");
    assert!(px[0] > px[2] + 20, "and keeps its colour: {px:?}");
}

#[test]
fn bright_lights_curve_is_smooth_monotone_and_bounded() {
    use crate::finish::{LIGHTS_RANGE, compress_lights};
    let knee = 1.0;
    assert_eq!(compress_lights(knee - 0.5, 1.0, knee), 0.0);
    assert_eq!(compress_lights(12.0, 0.0, knee), 0.0);
    let mut prev = f32::NEG_INFINITY;
    for i in 0..400 {
        let l = knee + i as f32 * 0.05;
        let out = l + compress_lights(l, 1.0, knee);
        assert!(out >= prev && out <= knee + LIGHTS_RANGE + 1e-4, "{l} -> {out}");
        prev = out;
    }
    // stronger compresses more
    assert!(compress_lights(8.0, 1.0, knee) < compress_lights(8.0, 0.5, knee));
    // after the tone map: untouched under the knee, under white at full strength however bright
    use crate::finish::{LIGHTS_DISPLAY_KNEE, LIGHTS_DISPLAY_WHITE, compress_display_lights};
    assert_eq!(compress_display_lights(LIGHTS_DISPLAY_KNEE - 0.1, 1.0), LIGHTS_DISPLAY_KNEE - 0.1);
    let mut prev = 0.0;
    for i in 0..400 {
        let mx = LIGHTS_DISPLAY_KNEE + i as f32 * 0.02;
        let out = compress_display_lights(mx, 1.0);
        assert!(out >= prev && out <= LIGHTS_DISPLAY_WHITE, "{mx} -> {out}");
        prev = out;
    }
}
