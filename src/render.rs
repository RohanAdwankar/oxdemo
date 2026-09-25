//! Turns a take into files: mp4, GIF, a contact sheet and a caption track.

use crate::script::Settings;
use crate::take::Take;
use anyhow::{anyhow, bail, Context, Result};
use image::{imageops, Rgb, RgbImage};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const VIEW_HZ: f64 = 100.0;
/// How long a zoom takes to ease in or out.
const ZOOM_EASE: f64 = 0.8;
/// How quickly the zoomed view catches up with the cursor.
const FOLLOW: f64 = 0.45;

#[derive(Clone, Copy, Debug)]
struct View {
    cx: f64,
    cy: f64,
    z: f64,
}

pub struct Renderer<'a> {
    take: &'a Take,
    views: Vec<View>,
}

fn smooth(s: f64) -> f64 {
    let s = s.clamp(0.0, 1.0);
    s * s * (3.0 - 2.0 * s)
}

impl<'a> Renderer<'a> {
    pub fn new(take: &'a Take) -> Self {
        let (w, h) = (take.width as f64, take.height as f64);
        let n = ((take.end - take.start) * VIEW_HZ).ceil() as usize + 1;
        let mut views = Vec::with_capacity(n);
        let (mut cx, mut cy) = (w / 2.0, h / 2.0);
        let mut mi = 0;
        let mut target = (w / 2.0, h / 2.0);
        let dt = 1.0 / VIEW_HZ;
        for i in 0..n {
            let t = take.start + i as f64 * dt;
            // Zoom: ease from the previous level to each new one.
            let mut z = 1.0;
            let mut prev = 1.0;
            for &(tz, level) in &take.zooms {
                if tz > t {
                    break;
                }
                z = prev + (level - prev) * smooth((t - tz) / ZOOM_EASE);
                prev = level;
            }
            // Follow the cursor, looking slightly ahead so the view arrives with it.
            while mi < take.moves.len() && take.moves[mi].0 <= t + 0.15 {
                target = (take.moves[mi].1, take.moves[mi].2);
                mi += 1;
            }
            let k = 1.0 - (-dt / FOLLOW).exp();
            if i == 0 {
                (cx, cy) = target;
            } else {
                cx += (target.0 - cx) * k;
                cy += (target.1 - cy) * k;
            }
            views.push(View { cx, cy, z });
        }
        Renderer { take, views }
    }

    pub fn duration(&self) -> f64 {
        self.take.end - self.take.start
    }

    fn view(&self, t: f64) -> View {
        let i = ((t * VIEW_HZ).round() as usize).min(self.views.len() - 1);
        self.views[i]
    }

    fn raw_index(&self, t: f64) -> usize {
        let abs = self.take.start + t;
        let f = &self.take.frames;
        match f.partition_point(|fr| fr.t <= abs) {
            0 => 0,
            n => n - 1,
        }
    }

    /// A raw frame, cropped to the viewport (the window can be taller than it).
    fn decode(&self, i: usize) -> Result<RgbImage> {
        let p = &self.take.frames[i].path;
        let img = image::open(p)
            .with_context(|| format!("reading {}", p.display()))?
            .to_rgb8();
        let (sw, sh) = img.dimensions();
        let vw = ((self.take.width as f64 * self.take.scale).round() as u32).min(sw);
        let vh = ((self.take.height as f64 * self.take.scale).round() as u32).min(sh);
        if (vw, vh) == (sw, sh) {
            return Ok(img);
        }
        Ok(imageops::crop_imm(&img, 0, 0, vw, vh).to_image())
    }

    /// The picture at `t` seconds into the take, cropped to the zoom and scaled to `out`.
    fn picture(&self, src: &RgbImage, t: f64, zoom: bool, out: (u32, u32)) -> RgbImage {
        let (sw, sh) = src.dimensions();
        let v = if zoom {
            self.view(t)
        } else {
            View {
                cx: 0.0,
                cy: 0.0,
                z: 1.0,
            }
        };
        if v.z <= 1.001 {
            if (sw, sh) == out {
                return src.clone();
            }
            return imageops::resize(src, out.0, out.1, imageops::FilterType::Triangle);
        }
        let abs = self.take.start + t;
        let pinned: Vec<_> = self
            .take
            .pinned
            .iter()
            .filter(|p| p.0 <= abs && abs < p.1)
            .collect();
        let px = sw as f64 / self.take.width as f64;
        let (cw, ch) = (sw as f64 / v.z, sh as f64 / v.z);
        let x = (v.cx * px - cw / 2.0).clamp(0.0, sw as f64 - cw);
        let mut y = (v.cy * px - ch / 2.0).clamp(0.0, sh as f64 - ch);
        // Keep the zoomed view above the caption and badges, and draw those at their
        // own size and place. Until the view is zoomed enough to clear them, they are
        // left to zoom with the page, so they never show twice.
        let top = pinned.iter().map(|p| p.2[1]).fold(f64::INFINITY, f64::min);
        let limit = (top - 8.0) * px - ch;
        let pin = top.is_finite() && limit >= 0.0;
        if pin {
            y = y.min(limit);
        }
        let crop = imageops::crop_imm(src, x as u32, y as u32, cw as u32, ch as u32).to_image();
        let mut img = imageops::resize(&crop, out.0, out.1, imageops::FilterType::CatmullRom);
        if pin {
            for p in pinned {
                paste_rounded(
                    &mut img,
                    src,
                    p.2,
                    px,
                    out.0 as f64 / self.take.width as f64,
                    6.0,
                );
            }
        }
        img
    }

    /// Render the pictures at `times`, in parallel, handing each to `sink` in order.
    fn render(
        &self,
        times: &[f64],
        zoom: bool,
        out: (u32, u32),
        mut sink: impl FnMut(usize, RgbImage) -> Result<()>,
    ) -> Result<()> {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(8);
        let batch = threads * 4;
        for (b, chunk) in times.chunks(batch).enumerate() {
            let per = chunk.len().div_ceil(threads);
            let results: Vec<Result<Vec<RgbImage>>> = std::thread::scope(|s| {
                let handles: Vec<_> = chunk
                    .chunks(per)
                    .map(|part| {
                        s.spawn(move || -> Result<Vec<RgbImage>> {
                            let mut cache: Option<(usize, RgbImage)> = None;
                            let mut out_frames = vec![];
                            for &t in part {
                                let i = self.raw_index(t);
                                if cache.as_ref().map(|c| c.0) != Some(i) {
                                    cache = Some((i, self.decode(i)?));
                                }
                                out_frames.push(self.picture(
                                    &cache.as_ref().unwrap().1,
                                    t,
                                    zoom,
                                    out,
                                ));
                            }
                            Ok(out_frames)
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let mut k = b * batch;
            for r in results {
                for img in r? {
                    sink(k, img)?;
                    k += 1;
                }
            }
        }
        Ok(())
    }
}

/// Copy the CSS-pixel rectangle `r` of `src` into `dst` at the same place, with rounded corners.
fn paste_rounded(
    dst: &mut RgbImage,
    src: &RgbImage,
    r: [f64; 4],
    src_px: f64,
    dst_px: f64,
    radius: f64,
) {
    let (sx, sy) = (
        (r[0] * src_px).max(0.0) as u32,
        (r[1] * src_px).max(0.0) as u32,
    );
    let sw = ((r[2] * src_px) as u32).min(src.width().saturating_sub(sx));
    let sh = ((r[3] * src_px) as u32).min(src.height().saturating_sub(sy));
    if sw == 0 || sh == 0 {
        return;
    }
    let (dw, dh) = (
        ((r[2] * dst_px) as u32).max(1),
        ((r[3] * dst_px) as u32).max(1),
    );
    let piece = imageops::crop_imm(src, sx, sy, sw, sh).to_image();
    let piece = if (sw, sh) == (dw, dh) {
        piece
    } else {
        imageops::resize(&piece, dw, dh, imageops::FilterType::Triangle)
    };
    let (dx, dy) = ((r[0] * dst_px) as i64, (r[1] * dst_px) as i64);
    let rad = radius * dst_px;
    for (x, y, p) in piece.enumerate_pixels() {
        let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
        let cx = if fx < rad {
            rad - fx
        } else if fx > dw as f64 - rad {
            fx - (dw as f64 - rad)
        } else {
            0.0
        };
        let cy = if fy < rad {
            rad - fy
        } else if fy > dh as f64 - rad {
            fy - (dh as f64 - rad)
        } else {
            0.0
        };
        if cx * cx + cy * cy > rad * rad {
            continue;
        }
        let (tx, ty) = (dx + x as i64, dy + y as i64);
        if tx >= 0 && ty >= 0 && (tx as u32) < dst.width() && (ty as u32) < dst.height() {
            dst.put_pixel(tx as u32, ty as u32, *p);
        }
    }
}

fn even(n: u32) -> u32 {
    n & !1
}

fn out_size(take: &Take, width: u32) -> (u32, u32) {
    let h = (width as f64 * take.height as f64 / take.width as f64).round() as u32;
    (even(width), even(h))
}

/// An ffmpeg that can write H.264: $OXDEMO_FFMPEG, PATH, or the one `pip install imageio-ffmpeg` ships.
pub fn find_ffmpeg() -> Option<PathBuf> {
    let mut spots = vec![];
    if let Ok(p) = std::env::var("OXDEMO_FFMPEG") {
        spots.push(PathBuf::from(p));
    }
    if let Some(p) = crate::cdp::which("ffmpeg") {
        spots.push(p);
    }
    if let Ok(o) = Command::new("python3")
        .args([
            "-c",
            "import imageio_ffmpeg; print(imageio_ffmpeg.get_ffmpeg_exe())",
        ])
        .stderr(Stdio::null())
        .output()
    {
        let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !p.is_empty() {
            spots.push(PathBuf::from(p));
        }
    }
    spots.into_iter().find(|p| {
        Command::new(p)
            .args(["-hide_banner", "-encoders"])
            .stderr(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("libx264"))
            .unwrap_or(false)
    })
}

pub fn mp4(r: &Renderer, s: &Settings, path: &Path) -> Result<()> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        anyhow!("no ffmpeg with H.264 found. `pip install imageio-ffmpeg`, or set OXDEMO_FFMPEG")
    })?;
    let fps = s.video.fps.max(1);
    let (w, h) = out_size(r.take, s.video.width.unwrap_or(s.width));
    let n = (r.duration() * fps as f64).floor() as usize;
    let times: Vec<f64> = (0..n).map(|i| i as f64 / fps as f64).collect();
    let mut child = Command::new(&ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
        ])
        .arg(format!("{w}x{h}"))
        .args([
            "-r",
            &fps.to_string(),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-preset",
            "slow",
            "-crf",
            &s.video.crf.to_string(),
        ])
        .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", ffmpeg.display()))?;
    let mut stdin = child.stdin.take().unwrap();
    r.render(&times, true, (w, h), |_, img| {
        Ok(stdin.write_all(img.as_raw())?)
    })?;
    drop(stdin);
    let status = child.wait()?;
    if !status.success() {
        bail!("ffmpeg failed ({status})");
    }
    Ok(())
}

/// One palette for the whole GIF, from a sample of its frames, with a
/// 6-bit-per-channel lookup table so mapping a pixel is one index.
struct Palette {
    rgb: Vec<u8>,
    lut: Vec<u8>,
}

const TRANSPARENT: u8 = 255;

impl Palette {
    fn build(samples: &[RgbImage]) -> Palette {
        let mut rgba = vec![];
        for img in samples {
            for (i, p) in img.pixels().enumerate() {
                if i % 3 == 0 {
                    rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
        }
        let nq = color_quant::NeuQuant::new(10, 255, &rgba);
        let rgb = nq.color_map_rgb();
        let mut lut = vec![0u8; 1 << 18];
        for (i, slot) in lut.iter_mut().enumerate() {
            let c = |s: usize| (((i >> s) & 63) << 2 | 2) as u8;
            *slot = nq.index_of(&[c(12), c(6), c(0), 255]) as u8;
        }
        Palette { rgb, lut }
    }

    fn index(&self, p: &Rgb<u8>) -> u8 {
        self.lut[((p[0] as usize >> 2) << 12) | ((p[1] as usize >> 2) << 6) | (p[2] as usize >> 2)]
    }
}

pub fn gif(r: &Renderer, s: &Settings, path: &Path) -> Result<()> {
    let g = &s.gif;
    let fps = g.fps.max(1) as f64;
    let (w, h) = out_size(r.take, g.width);
    let from = g.from.unwrap_or(0.0).max(0.0);
    let to = g.to.unwrap_or(r.duration()).min(r.duration());
    if to <= from {
        bail!("gif from={from}s to={to}s is empty");
    }
    let mut times: Vec<f64> = (0..((to - from) * fps).floor() as usize)
        .map(|i| from + i as f64 / fps)
        .collect();
    // Start the loop where it reads best: the first frame is the one seen most.
    // Unless told otherwise, begin once the first caption is up.
    let first_caption = r
        .take
        .captions
        .iter()
        .find(|c| !c.1.is_empty())
        .map(|c| c.0 - r.take.start + 0.4);
    if let Some(st) = g.start.or(first_caption) {
        let k = times.partition_point(|&t| t < st);
        let k = k.min(times.len());
        times.rotate_left(k);
    }

    let picks: Vec<f64> = (0..16).map(|i| times[i * times.len() / 16]).collect();
    let mut samples = vec![];
    r.render(&picks, true, (w, h), |_, img| {
        samples.push(img);
        Ok(())
    })?;
    let pal = Palette::build(&samples);

    let file = std::fs::File::create(path)?;
    let mut enc = gif::Encoder::new(std::io::BufWriter::new(file), w as u16, h as u16, &pal.rgb)?;
    enc.set_repeat(gif::Repeat::Infinite)?;
    let cs = |i: usize| (i as f64 * 100.0 / fps).round() as u16;
    let mut shown: Vec<u8> = vec![];
    // A frame is held back until the next one differs, so unchanged stretches become one long frame.
    let mut held: Option<(gif::Frame<'static>, usize)> = None;
    let write = |held: &mut Option<(gif::Frame<'static>, usize)>,
                 end: usize,
                 enc: &mut gif::Encoder<_>|
     -> Result<()> {
        if let Some((mut f, begin)) = held.take() {
            f.delay = cs(end) - cs(begin);
            enc.write_frame(&f)?;
        }
        Ok(())
    };
    let n = times.len();
    r.render(&times, true, (w, h), |i, img| {
        let idx: Vec<u8> = img.pixels().map(|p| pal.index(p)).collect();
        if i == 0 {
            let f = gif::Frame {
                width: w as u16,
                height: h as u16,
                buffer: idx.clone().into(),
                dispose: gif::DisposalMethod::Keep,
                ..Default::default()
            };
            held = Some((f, 0));
            shown = idx;
            return Ok(());
        }
        // Only the rectangle that changed, with unchanged pixels inside it transparent.
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for y in 0..h {
            let row = (y * w) as usize;
            for x in 0..w {
                if idx[row + x as usize] != shown[row + x as usize] {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        if x1 < x0 {
            return Ok(());
        }
        let (fw, fh) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut buf = Vec::with_capacity((fw * fh) as usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let k = (y * w + x) as usize;
                buf.push(if idx[k] == shown[k] {
                    TRANSPARENT
                } else {
                    idx[k]
                });
                shown[k] = idx[k];
            }
        }
        write(&mut held, i, &mut enc)?;
        let f = gif::Frame {
            left: x0 as u16,
            top: y0 as u16,
            width: fw as u16,
            height: fh as u16,
            buffer: buf.into(),
            transparent: Some(TRANSPARENT),
            dispose: gif::DisposalMethod::Keep,
            ..Default::default()
        };
        held = Some((f, i));
        Ok(())
    })?;
    write(&mut held, n, &mut enc)?;
    Ok(())
}

/// Every few seconds of the take on one image, three across, so a whole take can be checked at a glance.
pub fn sheet(r: &Renderer, path: &Path, failure_shot: Option<&Path>) -> Result<()> {
    let tile = out_size(r.take, 480);
    let d = r.duration();
    let every = (d / 12.0).max(1.0);
    let times: Vec<f64> = (0..)
        .map(|i| i as f64 * every)
        .take_while(|&t| t < d)
        .collect();
    let mut tiles = vec![];
    r.render(&times, false, tile, |_, img| {
        tiles.push((img, false));
        Ok(())
    })?;
    if let Some(p) = failure_shot {
        if let Ok(img) = image::open(p) {
            let img = imageops::resize(
                &img.to_rgb8(),
                tile.0,
                tile.1,
                imageops::FilterType::Triangle,
            );
            tiles.push((img, true));
        }
    }
    let (cols, gap) = (3u32, 6u32);
    let rows = (tiles.len() as u32).div_ceil(cols);
    let mut out = RgbImage::from_pixel(
        cols * (tile.0 + gap) + gap,
        rows * (tile.1 + gap) + gap,
        Rgb([255, 255, 255]),
    );
    for (k, (img, failed)) in tiles.iter().enumerate() {
        let (x, y) = (
            gap + (k as u32 % cols) * (tile.0 + gap),
            gap + (k as u32 / cols) * (tile.1 + gap),
        );
        if *failed {
            for dy in 0..tile.1 + 6 {
                for dx in 0..tile.0 + 6 {
                    out.put_pixel(x + dx - 3, y + dy - 3, Rgb([214, 40, 40]));
                }
            }
        }
        imageops::replace(&mut out, img, x as i64, y as i64);
    }
    out.save(path)?;
    Ok(())
}

fn stamp(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// The captions as WebVTT: subtitles for the mp4, and chapter marks for a player or README.
pub fn vtt(take: &Take, path: &Path) -> Result<()> {
    let mut out = String::from("WEBVTT\n");
    let caps: Vec<_> = take
        .captions
        .iter()
        .map(|(t, s)| (t - take.start, s))
        .collect();
    for (i, (t, text)) in caps.iter().enumerate() {
        if text.is_empty() {
            continue;
        }
        let end = caps
            .get(i + 1)
            .map(|c| c.0)
            .unwrap_or(take.end - take.start);
        out.push_str(&format!(
            "\n{}\n{} --> {}\n{}\n",
            i + 1,
            stamp(*t),
            stamp(end),
            text
        ));
    }
    std::fs::write(path, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps() {
        assert_eq!(stamp(65.4321), "00:01:05.432");
    }
}
