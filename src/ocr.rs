//! Finding text on screen, for targets the page itself cannot resolve: a remote
//! desktop, a canvas, a video. Uses ocrs, a pure Rust OCR engine; its two
//! models (about 12 MB) are downloaded once on first use.

use anyhow::{anyhow, bail, Context, Result};
use image::{imageops, RgbImage};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use rten::Model;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

const MODELS: &str = "https://ocrs-models.s3-accelerate.amazonaws.com";

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    norm: String,
    line: usize,
    index: usize,
    /// CSS pixels.
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Lowercase letters and digits only, so "Plasma..." matches "plasma".
pub fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Where the models live: $OXDEMO_OCR_MODELS, else ~/.cache/oxdemo.
fn model_dir() -> Result<PathBuf> {
    if let Ok(d) = std::env::var("OXDEMO_OCR_MODELS") {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var("HOME").context("no HOME for the model cache")?;
    Ok(PathBuf::from(home).join(".cache/oxdemo"))
}

fn model(name: &str) -> Result<Model> {
    let dir = model_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    if !path.exists() {
        eprintln!("  downloading the text recognition model {name} (once)");
        // curl follows the environment's proxy settings and certificates.
        let tmp = path.with_extension("part");
        let ok = Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&tmp)
            .arg(format!("{MODELS}/{name}"))
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            bail!(
                "could not download {MODELS}/{name}; put it in {} yourself",
                dir.display()
            );
        }
        std::fs::rename(&tmp, &path)?;
    }
    Model::load_file(&path).with_context(|| format!("loading {}", path.display()))
}

fn engine() -> Result<&'static OcrEngine> {
    static ENGINE: OnceLock<OcrEngine> = OnceLock::new();
    if let Some(e) = ENGINE.get() {
        return Ok(e);
    }
    let e = OcrEngine::new(OcrEngineParams {
        detection_model: Some(model("text-detection.onnx")?),
        recognition_model: Some(model("text-recognition.onnx")?),
        ..Default::default()
    })
    .map_err(|e| anyhow!("starting text recognition: {e}"))?;
    Ok(ENGINE.get_or_init(|| e))
}

/// Read every word in `shot`, a screenshot at `scale` device pixels per CSS pixel.
pub fn read(shot: &RgbImage, scale: f64) -> Result<Vec<Word>> {
    let engine = engine()?;
    // Screen text is small; it reads far better at twice the size.
    let up = if scale >= 2.0 { 1.0 } else { 2.0 };
    let big = if up == 1.0 {
        shot.clone()
    } else {
        imageops::resize(
            shot,
            (shot.width() as f64 * up) as u32,
            (shot.height() as f64 * up) as u32,
            imageops::FilterType::CatmullRom,
        )
    };
    let err = |e: &dyn std::fmt::Display| anyhow!("text recognition: {e}");
    let source = ImageSource::from_bytes(big.as_raw(), big.dimensions()).map_err(|e| err(&e))?;
    let input = engine.prepare_input(source).map_err(|e| err(&e))?;
    let rects = engine.detect_words(&input).map_err(|e| err(&e))?;
    let lines = engine.find_text_lines(&input, &rects);
    let texts = engine.recognize_text(&input, &lines).map_err(|e| err(&e))?;
    let k = 1.0 / (up * scale);
    let mut words = vec![];
    for (line, text) in texts.iter().enumerate() {
        let Some(text) = text else { continue };
        for (index, word) in text.words().enumerate() {
            let t = word.to_string();
            let norm = norm(&t);
            if norm.is_empty() {
                continue;
            }
            let r = word.bounding_rect();
            words.push(Word {
                text: t,
                norm,
                line,
                index,
                x: r.left() as f64 * k,
                y: r.top() as f64 * k,
                w: r.width() as f64 * k,
                h: r.height() as f64 * k,
            });
        }
    }
    Ok(words)
}

/// Edit distance, for forgiving a misread letter.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let cur = row[j + 1];
            row[j + 1] = (prev + (ca != b[j]) as usize).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// Whether an on-screen word reads as `want`, allowing one misread letter in longer words.
fn same(seen: &str, want: &str) -> bool {
    seen == want || (want.chars().count() >= 4 && distance(seen, want) <= 1)
}

/// Every place the words of `target` appear in order on one line, as [x, y, w, h].
pub fn find(words: &[Word], target: &str) -> Vec<[f64; 4]> {
    let want: Vec<String> = target
        .split_whitespace()
        .map(norm)
        .filter(|w| !w.is_empty())
        .collect();
    if want.is_empty() {
        return vec![];
    }
    let mut hits = vec![];
    for (i, first) in words.iter().enumerate() {
        if !same(&first.norm, &want[0]) {
            continue;
        }
        let run: Vec<&Word> = words[i..].iter().take(want.len()).collect();
        let fits = run.len() == want.len()
            && run.iter().enumerate().all(|(k, w)| {
                w.line == first.line && w.index == first.index + k && same(&w.norm, &want[k])
            });
        if fits {
            let last = run[run.len() - 1];
            let (y0, y1) = (
                run.iter().map(|w| w.y).fold(f64::INFINITY, f64::min),
                run.iter().map(|w| w.y + w.h).fold(0.0, f64::max),
            );
            hits.push([first.x, y0, last.x + last.w - first.x, y1 - y0]);
        }
    }
    hits
}

/// A few words on screen that look like `target`, for an error message.
pub fn near(words: &[Word], target: &str) -> Vec<String> {
    let t = norm(target.split_whitespace().next().unwrap_or(""));
    let mut scored: Vec<(usize, &String)> = words
        .iter()
        .filter(|w| w.norm.len() > 2)
        .map(|w| (distance(&w.norm, &t), &w.text))
        .filter(|(d, _)| *d <= t.len() / 2 + 1)
        .collect();
    scored.sort();
    let mut out: Vec<String> = vec![];
    for (_, text) in scored {
        if !out.contains(text) {
            out.push(text.clone());
        }
    }
    out.truncate(6);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(line: usize, y: f64, texts: &[&str]) -> Vec<Word> {
        let mut x = 10.0;
        texts
            .iter()
            .enumerate()
            .map(|(index, t)| {
                let w = Word {
                    text: t.to_string(),
                    norm: norm(t),
                    line,
                    index,
                    x,
                    y,
                    w: t.len() as f64 * 7.0,
                    h: 12.0,
                };
                x += w.w + 6.0;
                w
            })
            .collect()
    }

    #[test]
    fn phrases_on_one_line() {
        let mut words = line(0, 50.0, &["GNU", "Image", "Manipulation", "Program"]);
        words.extend(line(1, 200.0, &["Image", "Plasma..."]));
        words.extend(line(2, 300.0, &["File", "Edit", "Heln"]));
        assert_eq!(find(&words, "GNU Image Manipulation Program").len(), 1);
        assert_eq!(find(&words, "Image").len(), 2);
        assert_eq!(find(&words, "plasma").len(), 1);
        assert!(find(&words, "Image Program").is_empty());
        // One misread letter is forgiven in longer words, not in short ones.
        assert_eq!(find(&words, "Help").len(), 1);
        assert!(find(&words, "Fil").is_empty());
        assert_eq!(near(&words, "Plasm"), vec!["Plasma..."]);
    }
}
