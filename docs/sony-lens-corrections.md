# Sony embedded distortion corrections

LightCraft reads the signed distortion table in the raw image IFD (`0x7037`) of Sony Bayer ARWs from
**ILCE-7RM4A (A7R IVA)** and **ILCE-7C (A7C)** (16 samples) and **DSC-RX10M5 (RX10 V)** (11 samples).
Other camera models are deliberately left uncorrected, even when they contain a table with the same layout.
It converts that table into `OpcodeList3` / `WarpRectilinear`, so the existing profile correction control,
CPU/GPU optics path, EXIF orientation handling, and DNG export use the same correction.

This adds **distortion only**. Sony vignetting (`0x7032`) and lateral chromatic-aberration (`0x7035`) tables are
read in the measurements below but not applied: on these bodies the camera JPEG shows neither correction relative
to LightCraft's render (see "Vignetting and chromatic aberration").
The enabled and independently validated model is ILCE-7RM4A with the FE 24–105mm F4 G OSS and
FE 200–600mm F5.6–6.3 G OSS. Linear YCbCr ARWs and non-3:2 camera crops are excluded pending validation. An aspect crop may
retain the full-frame radial normalization; the decoder does not guess that relationship. Older files that carry
only encrypted correction metadata, different table lengths, and rejected tables remain uncorrected.
The camera's distortion Off setting does not erase its table. Newly imported photos use LightCraft's existing
embedded-profile default (enabled); the profile correction control can disable or adjust it.

## Clean-room evidence

The [ExifTool EXIF tag-name documentation](https://exiftool.org/TagNames/EXIF.html) identifies
`0x7037` as 17 signed 16-bit integers. The first value is the count, 16, followed by radial samples.
No external photo decoder implementation or Adobe lens profile was consulted.
Sony Imaging Edge Edit 4.1 was used only as a black-box reference on local scratch copies.

The observed inverse mapping, with radius `r` normalized to the default crop's half diagonal, is:

```
f(r) = 1 + interpolate(table, r) / 16384
source = center + (output - center) * zoom * f(r)
zoom = 1 / max(f(r)) over radii on the rectangular output boundary
```

The border radii range from the shorter half edge / half diagonal to 1. Table knots span 0 through 1.
Linear interpolation approximates the observed map closely; we do not claim to reproduce Sony's
internal interpolator. A fixed-size weighted least-squares fit expresses source radius as
`r * (k0 + k1*r² + k2*r⁴ + k3*r⁶)`. The fit is rejected if radial error exceeds 0.0005 of the
half diagonal (about 0.43 px at a 1440 px 3:2 preview), or the mapping folds. Table values are bounded
and the input must have precisely the supported signed-short layout. Crop offset and normalization
are converted into DNG's active-area coordinate system before creating the opcode. The warp's centre
and normalization come from the standard `DefaultCropOrigin` / `DefaultCropSize` when the file has them,
not from Sony's crop tags: on the tested A7R IVA files the former start at raw x=32 and the latter at
x=0, and the standard origin is the one that aligns with Sony's exports. The image itself keeps the
decoder's crop precedence for every Sony body (Sony's crop tags, then the default crop, then the
recorded image size), so the framing is unchanged; the opcode's centre is in active-area coordinates,
so the warp stays on the optical centre either way. On the A7R IVA that framing still sits 32 raw
pixels left of Sony's exports (eight pixels in a 2376-pixel-wide export).

The main raw-IFD table is important: the similarly named MakerNote table has different values and
must not be substituted into this formula. Main raw-IFD and SR2SubIFD values agreed in the test files.

## Validation

At 2376×1584, independent SIFT correspondences between Sony distortion On and Off TIFF exports gave:

| Lens / focal length | Median residual | 95th percentile | Maximum matched radius |
|---|---:|---:|---:|
| FE 200–600 / 200 mm | 0.043 px | 0.191 px | 0.97 half diagonals |
| FE 200–600 / 600 mm | 0.055 px | 0.231 px | 0.95 half diagonals |

These use the fixed 16384 scale and automatic border framing, not fitted per-image scale or zoom.
A median translation below 0.004 px was removed from the residuals. Correspondences do not cover
all pixels; these are geometric feature residuals, not photometric equality or a worst-case guarantee.
Across 20 sampled RAWs (16 at 24–105 mm, four at 200–600 mm), the polynomial's maximum deviation
from the interpolated file table was below 0.12 px at 1440 px. That measures approximation error,
not agreement with an independent renderer. Camera JPEG and Apple Core Image comparisons support
the wide-lens interpretation but are weaker evidence than same-renderer On/Off pairs.

Historical validation before restoring main's image crop precedence: LightCraft exports with the
standard default crop framing were compared directly with Sony's corrected
TIFFs. At 2376 px wide, median / 95th-percentile feature residuals were 0.35 / 1.04 px at 24 mm,
0.15 / 0.40 px at 200 mm, and 0.13 / 0.56 px at 600 mm (median translation below 0.024 px).
The uncorrected medians at 200 and 600 mm were 8.63 and 10.82 px respectively. Different colour,
sharpening, and demosaicing affect feature localization; these are geometry checks, not colour parity.
An isolated desktop run on Apple M1 Metal detected the profile, rendered without GPU fallback or
warnings, and exposed the existing Optics controls. Existing library edits are not reset: once the
source has loaded, enable lens corrections if they were previously disabled.

After restoring main's crop precedence, fresh 2376×1584 exports on 2026-10-10
(main `b70ae174` plus the aspect-framing guard) measured:

| Focal length | Corrected median / p95 residual | Uncorrected median | Removed translation x / y | Matches / maximum radius |
|---|---|---|---|---|
| 24 mm | 0.36 / 1.12 px | 5.62 px | 7.98 / −0.01 px | 1,442 / 0.95 |
| 200 mm | 0.13 / 0.35 px | 8.65 px | 8.01 / 0.02 px | 11,606 / 0.98 |
| 600 mm | 0.10 / 0.37 px | 10.33 px | 8.02 / 0.00 px | 12,268 / 0.97 |

Residuals subtract only the median feature translation; no scale or lens coefficients are fitted to
these reference pairs. The roughly eight-pixel horizontal translation is the retained crop-origin
difference described above, not a claim of exact Sony framing. Maximum radius is in half diagonals.
The distortion geometry remains aligned while every body's image framing follows main's crop order.

Tests cover signedness/length rejection, zero and malformed tables, independent 600 mm reference
geometry, barrel framing, offset crop normalization, both TIFF byte orders, header/full consistency,
and preservation through DNG export. No private photographs or reference TIFFs are committed.

## ILCE-7C and DSC-RX10M5 (2026-11-12, AntPitLab)

The same formula, with the knots evenly spaced from centre (0) to corner (1) whatever their number, describes both
bodies. The DSC-RX10M5 writes 11 knots (the tag's first value), padded with zeros to the same 17 values; the code
accepts exactly the knot count recorded for each validated model and refuses non-zero padding. The automatic
frame-filling zoom is the same: on the RX10 V tables it comes out at 0.9999–1.0, i.e. the table already carries
its own scale (the 24 mm centre value 749 is that scale).

Evidence: SIFT correspondences (OpenCV, 7k–14k matches per frame) between LightCraft's uncorrected render and the
camera's own corrected JPEG of the same frame, fitted with the table's radial shape and only a free zoom (fixed
centre at the crop centre). Pixels at the JPEG's full size.

| Body / lens / focal length | Camera setting | Uncorrected median | Fitted zoom (formula's) | Median / p95 residual |
|---|---|---:|---:|---:|
| RX10 V, 24 mm eq. | Auto, full-size JPEG 5472 px | 24.2 px | 1.0005 (0.9999) | 0.78 / 1.85 px |
| RX10 V, 103 mm eq. | Auto, full-size JPEG | 15.9 px | 0.9997 (1.0) | 0.80 / 1.66 px |
| RX10 V, 600 mm eq. | Auto, full-size JPEG | 6.4 px | 0.9995 (1.0) | 0.86 / 1.45 px |
| A7C, FE 28–60 at 28 mm | forced by the lens, 1616 px preview | — | 1.0288 (1.0291) | 0.32 / 0.89 px |
| A7C, FE 28–60 at 50 mm | forced by the lens | — | 0.9992 | 0.32 / 0.83 px |
| A7C, FE 28–60 at 60 mm | forced by the lens | — | 0.9998 | 0.27 / 0.77 px |

Freeing the centre lowers the RX10 V medians to 0.3–0.4 px with an offset of about 10–18 px that is not the same
on the three frames (not recorded in the file), so the centre stays at the crop centre. Freeing the table's scale
or knot spacing changes the residual by less than 0.05 px: the layout and the 1/16384 scale are the camera's.
The A7C's other lenses (85 GM II, Tamron 28–75, 35 GM) record the correction Off, so their JPEGs cannot certify the
warp; the body and its 16-knot format are what was validated, as for the A7R IVA. Unit tests pin the three RX10 V
tables to the JPEG-fitted geometry and the A7C 28 mm zoom. No photograph is committed.

### Vignetting and chromatic aberration

With the geometry aligned, the median ratio of the camera JPEG to LightCraft's render per radial ring (linearised,
mid-tones) stays within ±0.1 in log units from centre to corner on all six frames, where the vignetting table would
predict 0.3–0.9 at the corner under any of the scales tried (`1 + v/16384`, `2^(v/16384)`, `2^(v/8192)`,
`1 + v/8192`). The camera applies no shading on top of what the raw already has, so the table is not applied:
it would brighten the corners past the camera's rendition. LightCraft's render already registers the colour planes:
the radial red/blue shift against green is at most 0.36 px at 5472 px (RX10 V 24 mm red; 0.06–0.12 px elsewhere),
like the camera JPEG's 0.06 px, so the CA table is not applied either (as for Fujifilm).

## Camera coverage and regression boundary

The model check is exact: `ILCE-7RM4` (A7R IV without the A suffix), `ILCE-7M3`, unknown/missing
model names, and every other model remain unsupported by this distortion decoder. Matching tag
numbers and table lengths do not establish the scale, interpolation or crop normalization on another
body. Support can expand after independent geometry validation; do not add model-name prefix matching.

The current `cargo xtask corpus --download` manifest contains 23 Sony CC0 samples across 17 bodies,
including samples 4822 (A7R IVA / 24–105 mm) and 3989 (A9 II / 200–600 mm). All 23 files have
pinned SHA-256 hashes and are checked with `sony_embedded_distortion_is_limited_to_validated_models`:

| Camera | Corpus samples | Raw-IFD table observed | Expected correction |
|---|---|---|---|
| ILCE-7RM4A | Compressed (4822) | 16 samples | One distortion warp |
| ILCE-9M2 | Compressed (3989) | 16 samples | None |
| ILCE-7M3 | Compressed, uncompressed | 16 samples | None |
| ILCE-7M4 | 14-bit, lossless L/M/S | 11 samples in the 14-bit file; 16 in L/M/S | None |
| ILCE-7RM2 | 12-bit uncompressed | 16 samples | None |
| DSC-RX100 | One | No raw-IFD table | None |
| DSC-RX100M3 | One | 11 samples | None |
| ILCE-7CR | Compressed, lossless L/M | Not used; model excluded | None |
| ILCE-7RM4 | 14-bit compressed | Not used; model excluded | None |
| DSLR-A500, DSLR-A700, SLT-A33 | One each | Not used; model excluded | None |
| ILCE-3500, ILCE-7S | 5600 K, Shade respectively | Not used; model excluded | None |
| ILCE-7SM2, DSLR-A580 | 16:9 framing, one each | Not used; model excluded | None |
| DSLR-A900 | Packed 12-bit | Not used; model excluded | None |

These are **model-boundary regression checks, not new geometric validation of those models**. In particular,
refusing the A7 III tables resolves the unverified corner warp identified in review. The lossless M/S
files are linear YCbCr and remain excluded independently of the model check. Header and full decode
must agree for every sample. Synthetic tests also verify both TIFF byte orders, exact model matching,
that the warp is centred on the standard default crop while the image keeps Sony's crop, and that DNG export preserves an accepted warp without adding one to rejected
models. A non-3:2 Sony image crop is also rejected when the standard default crop still describes
3:2 geometry; the regression fails without the image-framing guard. The existing independent Sony
geometry tests continue to cover the enabled A7R IVA path.
Before restoring main's crop precedence, re-exporting the 24, 200 and 600 mm reference files after
the model restriction produced byte-identical corrected PNGs to the earlier validated exports. Isolated headless app checks showed the A7R IVA profile
controls present and the A7 III profile controls absent, with no notices and completed RAW renders.

Before enabling another body, compare corrected and uncorrected renders against independent references
across multiple lenses/focal lengths and photographs with features near the edges and corners. Record
residuals and spatial coverage, and check crop/aspect and compression variants. Embedded JPEGs are
useful when the camera actually applied distortion correction; an Off JPEG cannot certify an On warp.
Prefer same-renderer Sony On/Off exports when the camera setting or geometry is ambiguous. Table-fit
error alone only checks our polynomial approximation, not whether the table's interpretation is correct.
