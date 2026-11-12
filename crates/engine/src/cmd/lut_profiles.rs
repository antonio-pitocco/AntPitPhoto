//! Imported profiles: `.cube` 3D LUTs used as creative profiles (Amount 0–200 % like the
//! built-in ones). A library on disk keeps a copy of each file in its `Profiles/` folder; the
//! list lives in the library's prefs and the LUTs are registered with the pipeline when the
//! library opens.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{Result, Session};

/// One imported LUT profile.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LutProfile {
    /// `lut:<slug>`.
    pub id: String,
    pub name: String,
    pub group: String,
    /// The `.cube` file (the library's copy when there is a library on disk).
    pub file: String,
}

fn profiles_dir(s: &Session) -> Option<PathBuf> {
    s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("Profiles"))
}

/// Register every imported profile's LUT with the pipeline (missing files are skipped).
pub fn register_all(s: &Session) {
    for p in &s.lut_profiles {
        if let Some(l) = std::fs::read_to_string(&p.file).ok().and_then(|t| lightcraft_pipeline::lut::Lut3d::parse_cube(&t).ok()) {
            lightcraft_pipeline::lut::register(&p.id, l);
        }
    }
}

/// `.cube` files in `paths` (folders searched, zip bundles opened), and the files that couldn't
/// be read with the reason (`[name, reason]`).
fn cube_files(paths: &[String]) -> (Vec<(String, String, Option<String>)>, Vec<Value>) {
    // (name, text, group)
    let mut out = Vec::new();
    let mut failed = Vec::new();
    let stem = |f: &Path| f.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            let cube = |f: &Path| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("cube"));
            for ep in crate::walk::files_in(path, None, crate::walk::Limits::default(), cube).files {
                match std::fs::read_to_string(&ep) {
                    Ok(t) => {
                        let group = ep.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string());
                        out.push((stem(&ep), t, group));
                    }
                    Err(e) => failed.push(json!([stem(&ep), e.to_string()])),
                }
            }
        } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
            let b = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    failed.push(json!([stem(path), e.to_string()]));
                    continue;
                }
            };
            for (inner, data) in crate::preset_import::read_zip(&b).unwrap_or_default() {
                if inner.to_ascii_lowercase().ends_with(".cube") {
                    let ip = Path::new(&inner);
                    let group = ip
                        .parent()
                        .and_then(|d| d.file_name())
                        .map(|n| n.to_string_lossy().to_string())
                        .or_else(|| path.file_stem().map(|n| n.to_string_lossy().to_string()));
                    out.push((
                        ip.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                        String::from_utf8_lossy(&data).to_string(),
                        group,
                    ));
                }
            }
        } else {
            match std::fs::read_to_string(path) {
                Ok(t) => out.push((stem(path), t, None)),
                Err(e) => failed.push(json!([stem(path), e.to_string()])),
            }
        }
    }
    (out, failed)
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.import";
    let paths: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if paths.is_empty() {
        return Err(bad(C, "no paths"));
    }
    let dir = profiles_dir(s);
    if let Some(d) = &dir {
        std::fs::create_dir_all(d).map_err(|e| bad(C, format!("{}: {e}", d.display())))?;
    }
    let (files, mut failed) = cube_files(&paths);
    let mut imported = Vec::new();
    for (name, text, group) in files {
        let lut = match lightcraft_pipeline::lut::Lut3d::parse_cube(&text) {
            Ok(l) => l,
            Err(e) => {
                failed.push(json!([name, e]));
                continue;
            }
        };
        let display = lut.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| name.clone());
        let group = super::super::preset_import::group_from_dir(&group.unwrap_or_default()).unwrap_or_else(|| "Imported".into());
        let slug = crate::presets::slug(&format!("{group}-{display}"));
        let mut id = format!("lut:{slug}");
        let mut n = 2;
        while s.lut_profiles.iter().any(|x| x.id == id) {
            id = format!("lut:{slug}-{n}");
            n += 1;
        }
        let file = match &dir {
            Some(d) => {
                let f = d.join(format!("{}.cube", id.trim_start_matches("lut:")));
                std::fs::write(&f, &text).map_err(|e| bad(C, format!("{}: {e}", f.display())))?;
                f.to_string_lossy().to_string()
            }
            None => {
                // no library on disk: remember the original (the in-memory session ends with the app)
                let f = std::env::temp_dir().join(format!("lightcraft-{}.cube", id.trim_start_matches("lut:")));
                std::fs::write(&f, &text).map_err(|e| bad(C, e.to_string()))?;
                f.to_string_lossy().to_string()
            }
        };
        lightcraft_pipeline::lut::register(&id, lut);
        s.lut_profiles.push(LutProfile { id: id.clone(), name: display.clone(), group: group.clone(), file });
        imported.push(json!({"id": id, "name": display, "group": group}));
    }
    s.save_prefs()?;
    Ok(json!({"imported": imported, "failed": failed}))
}

/// Long edge of the renders a camera look is fitted on: enough pairs for a 17³ table, no fine detail
/// (sharpening, noise and demosaicing differ between the two pictures and are not the look).
const CAMERA_LOOK_EDGE: usize = 256;
/// Entries per axis of a camera-look table.
const CAMERA_LOOK_SIZE: usize = 17;
/// The group camera-look profiles are listed in.
const CAMERA_LOOK_GROUP: &str = "Come in macchina";
/// A fit is kept only when it brings the held-out pixels at least this much closer (mean squared error ratio):
/// a JPEG of another picture, or one the raw cannot be matched to, gives nothing to learn.
const CAMERA_LOOK_MIN_GAIN: f64 = 0.8;

/// Colour pairs (display-encoded 0..1) between `ours` and `camera`, both scaled to `w × h`, from the central 90 %
/// (framing and lens corrections differ most at the edges).
fn look_pairs(ours: &lightcraft_raster::Rgba8, camera: &lightcraft_raster::Rgba8, w: usize, h: usize) -> Vec<([f32; 3], [f32; 3])> {
    use lightcraft_raster::resample::{Filter, resize};
    let float = |img: &lightcraft_raster::Rgba8| {
        let mut f = lightcraft_raster::Rgb32f::new(img.width, img.height);
        for (o, i) in f.data.iter_mut().zip(&img.data) {
            *o = [f32::from(i[0]) / 255.0, f32::from(i[1]) / 255.0, f32::from(i[2]) / 255.0];
        }
        resize(&f, w, h, Filter::Box)
    };
    let (a, b) = (float(ours), float(camera));
    let (mx, my) = (w / 20, h / 20);
    let mut pairs = Vec::with_capacity(w * h);
    for y in my..h - my {
        for x in mx..w - mx {
            pairs.push((a.data[y * w + x], b.data[y * w + x]));
        }
    }
    pairs
}

fn mse(pairs: &[([f32; 3], [f32; 3])], f: impl Fn([f32; 3]) -> [f32; 3]) -> f64 {
    let sum: f64 = pairs
        .iter()
        .map(|(x, y)| {
            let m = f(*x);
            (0..3).map(|k| f64::from(m[k] - y[k]).powi(2)).sum::<f64>()
        })
        .sum();
    sum / pairs.len().max(1) as f64
}

/// `profile.fromCamera`: fit a 3D LUT that takes the photo's own default rendering to the camera's JPEG (the film
/// simulation, Creative Look or picture style the photographer chose), store it as an imported profile in the
/// «Come in macchina» group and select it for the photo. The JPEG supplies colour statistics only; the editing
/// stays on the raw.
fn from_camera(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.fromCamera";
    let id = s.active().ok_or_else(|| bad(C, "no photo selected"))?;
    let photo = s.catalog.photo(id).cloned().ok_or_else(|| bad(C, "no such photo"))?;
    let lightcraft_catalog::Source::File { path } = &photo.source else {
        return Err(bad(C, "only a photo read from a file has a camera JPEG"));
    };
    if !photo.develops_raw() {
        return Err(bad(C, "only a raw file has a camera JPEG to match"));
    }
    let bytes = std::fs::read(path).map_err(|e| bad(C, format!("{path}: {e}")))?;
    let opts = lightcraft_codecs::DecodeOptions { max_size: Some((1024, 1024)), max_pixels: 128_000_000 };
    let jpeg = crate::files::decode_raw_preview(&bytes, opts).ok_or_else(|| bad(C, "the raw file has no camera JPEG"))?.to_srgb8();
    let orientation = lightcraft_raw::probe_info(&bytes).map(|i| i.orientation).unwrap_or_default();
    drop(bytes);
    // the rendering the table starts from: the photo as imported, with the neutral profile
    let mut start = photo.camera_defaults();
    start.profile = lightcraft_develop::Profile::default();
    let ours = s.variant_job(id, &start, 2 * CAMERA_LOOK_EDGE).ok_or_else(|| bad(C, "no such photo"))?.run().rendered.map_err(|e| bad(C, e))?.image;
    let (ow, oh) = (ours.width.max(1), ours.height.max(1));
    let scale = CAMERA_LOOK_EDGE as f64 / ow.max(oh) as f64;
    let (w, h) = (((ow as f64 * scale).round() as usize).max(20), ((oh as f64 * scale).round() as usize).max(20));
    // the camera JPEG is stored as the sensor records it; try it as displayed and as stored, keep what matches
    let mut best: Option<(f64, Vec<([f32; 3], [f32; 3])>)> = None;
    for candidate in [jpeg.oriented(orientation), jpeg] {
        let aspect = |a: usize, b: usize| a as f64 / b.max(1) as f64;
        if (aspect(candidate.width, candidate.height) / aspect(ow, oh) - 1.0).abs() > 0.03 {
            continue;
        }
        let pairs = look_pairs(&ours, &candidate, w, h);
        let before = mse(&pairs, |c| c);
        if best.as_ref().is_none_or(|(b, _)| before < *b) {
            best = Some((before, pairs));
        }
    }
    let (_, pairs) = best.ok_or_else(|| bad(C, "the camera JPEG is framed differently from the raw"))?;
    // every fourth pair is held out to check the table on pixels it was not fitted on
    let (train, held): (Vec<_>, Vec<_>) = pairs.iter().enumerate().partition(|(i, _)| i % 4 != 0);
    let train: Vec<_> = train.into_iter().map(|(_, p)| *p).collect();
    let held: Vec<_> = held.into_iter().map(|(_, p)| *p).collect();
    let smooth = p.get("smooth").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.01, 100.0) as f32;
    let probe = lightcraft_pipeline::lut::Lut3d::fit(&train, CAMERA_LOOK_SIZE, smooth).ok_or_else(|| bad(C, "no colours to fit"))?;
    let (before, after) = (mse(&held, |c| c), mse(&held, |c| probe.apply(c)));
    if !(after < before * CAMERA_LOOK_MIN_GAIN) {
        return Err(bad(C, format!("the camera JPEG does not match this rendering closely enough (error {before:.5} -> {after:.5})")));
    }
    let mut lut = lightcraft_pipeline::lut::Lut3d::fit(&pairs, CAMERA_LOOK_SIZE, smooth).ok_or_else(|| bad(C, "no colours to fit"))?;
    let stem = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| format!("{}", id.0));
    let name = str_param(p, "name").map(str::to_string).unwrap_or(stem);
    lut.title = Some(name.clone());
    let slug = crate::presets::slug(&format!("camera-{name}"));
    let pid = format!("lut:{slug}");
    let text = lut.to_cube();
    let file = match profiles_dir(s) {
        Some(d) => {
            std::fs::create_dir_all(&d).map_err(|e| bad(C, format!("{}: {e}", d.display())))?;
            d.join(format!("{slug}.cube"))
        }
        None => std::env::temp_dir().join(format!("lightcraft-{slug}.cube")),
    };
    std::fs::write(&file, &text).map_err(|e| bad(C, format!("{}: {e}", file.display())))?;
    lightcraft_pipeline::lut::register(&pid, lut);
    // fitting the same photo again replaces its profile
    s.lut_profiles.retain(|x| x.id != pid);
    s.lut_profiles.push(LutProfile {
        id: pid.clone(),
        name: name.clone(),
        group: CAMERA_LOOK_GROUP.into(),
        file: file.to_string_lossy().to_string(),
    });
    s.save_prefs()?;
    let amount = p.get("amount").and_then(Value::as_f64).unwrap_or(100.0).clamp(0.0, 200.0);
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    d.profile.id = pid.clone();
    d.profile.amount = amount;
    s.set_develop(id, d, "Profile")?;
    s.note_profile_used(&pid);
    Ok(json!({"id": pid, "name": name, "group": CAMERA_LOOK_GROUP, "pairs": pairs.len(), "heldOutError": [before, after]}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "profile.import",
            "Import Profiles",
            [],
            None,
            "{paths: [.cube file, folder or .zip]} — 3D LUTs become creative profiles (Amount 0–200 %; rendered on the CPU) in the profile browser's groups (folder / zip name, else Imported) → {imported: [{id, name, group}], failed}",
            always,
            import
        ),
        cmd!(
            "profile.fromCamera",
            "Match Camera JPEG",
            [],
            None,
            "{amount?: 0..200, name?, smooth?} — the open raw photo gets a profile fitted to its own camera JPEG (film simulation, Creative Look, picture style), listed in «Come in macchina» → {id, name, group, pairs, heldOutError: [before, after]}",
            super::has_active,
            from_camera
        ),
        cmd!("profile.deleteImported", "Delete Imported Profile", [], None, "{id: lut:…}", always, |s, p| {
            let id = str_param(p, "id").ok_or_else(|| bad("profile.deleteImported", "missing id"))?.to_string();
            let Some(i) = s.lut_profiles.iter().position(|x| x.id == id) else {
                return Err(bad("profile.deleteImported", format!("no imported profile `{id}`")));
            };
            let gone = s.lut_profiles.remove(i);
            if profiles_dir(s).is_some_and(|d| Path::new(&gone.file).starts_with(d)) {
                let _ = std::fs::remove_file(&gone.file);
            }
            lightcraft_pipeline::lut::unregister(&id);
            s.save_prefs()?;
            Ok(json!({"deleted": id}))
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_look_pairs_cover_the_centre_at_the_fitting_size() {
        let mut ours = lightcraft_raster::Rgba8::new(40, 30);
        let mut camera = lightcraft_raster::Rgba8::new(80, 60);
        ours.data.iter_mut().for_each(|p| *p = [51, 102, 153, 255]);
        camera.data.iter_mut().for_each(|p| *p = [255, 0, 0, 255]);
        let pairs = look_pairs(&ours, &camera, 20, 20);
        // a 1-pixel border is left out on each side at 20 × 20
        assert_eq!(pairs.len(), 18 * 18);
        let (a, b) = pairs[0];
        assert!((a[0] - 0.2).abs() < 1e-3 && (a[2] - 0.6).abs() < 1e-3);
        assert_eq!(b, [1.0, 0.0, 0.0]);
        assert!((mse(&pairs, |c| c) - (0.8f64.powi(2) + 0.4f64.powi(2) + 0.6f64.powi(2))).abs() < 1e-3);
        assert_eq!(mse(&[], |c| c), 0.0);
    }
}
