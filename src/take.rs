//! Runs a script against a live browser and records what happened when.

use crate::cdp::{now, Browser, Cdp, Frame};
use crate::keys;
use crate::script::{Action, Script, Step};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Everything the renderer needs: frames, and where the cursor, zoom and captions were over time.
pub struct Take {
    pub frames: Vec<Frame>,
    pub start: f64,
    pub end: f64,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    /// (t, x, y) in CSS pixels.
    pub moves: Vec<(f64, f64, f64)>,
    /// (t, level) — each is the zoom from then on.
    pub zooms: Vec<(f64, f64)>,
    /// (from, to) — stretches cut out of the video, such as waiting for an app to start.
    pub cuts: Vec<(f64, f64)>,
    pub captions: Vec<(f64, String)>,
    /// Page furniture that stays put when the view zooms: (from, to, [x, y, w, h], is_caption).
    pub pinned: Vec<(f64, f64, [f64; 4], bool)>,
    /// (t, line) — when each step began.
    pub marks: Vec<(f64, usize)>,
    pub failure: Option<Failure>,
}

impl Take {
    /// Seconds of video, with the cuts taken out.
    pub fn length(&self) -> f64 {
        self.end - self.start - self.cuts.iter().map(|c| c.1 - c.0).sum::<f64>()
    }

    /// A time in the video to a time in the take, both from the start.
    pub fn to_take(&self, t: f64) -> f64 {
        let mut t = t;
        for &(a, b) in &self.cuts {
            if t >= a - self.start {
                t += b - a;
            } else {
                break;
            }
        }
        t
    }

    /// A wall-clock time during the take to a time in the video.
    pub fn to_video(&self, abs: f64) -> f64 {
        let mut t = abs - self.start;
        for &(a, b) in &self.cuts {
            if abs >= b {
                t -= b - a;
            } else if abs > a {
                t -= abs - a;
            }
        }
        t
    }
}

pub struct Failure {
    pub t: f64,
    pub line: usize,
    pub message: String,
    pub screenshot: Option<PathBuf>,
}

struct Runner<'a> {
    cdp: &'a Cdp,
    script: &'a Script,
    pace: f64,
    mouse: (f64, f64),
    pressed: bool,
    arc: f64,
    /// When the current caption went up, and how long it wants to stay.
    caption_hold: Option<(f64, f64)>,
    take: Take,
}

fn ease(s: f64) -> f64 {
    if s < 0.5 {
        4.0 * s * s * s
    } else {
        1.0 - (-2.0 * s + 2.0).powi(3) / 2.0
    }
}

/// How long a caption needs to be read: a base plus time per word.
fn reading_time(text: &str) -> f64 {
    let words = text.split_whitespace().count() as f64;
    (0.9 + words * 0.2).max(1.6)
}

impl<'a> Runner<'a> {
    fn sleep(&self, secs: f64) {
        if secs > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(secs));
        }
    }

    /// A pause that scales with `pace`.
    fn beat(&self, secs: f64) {
        self.sleep(secs * self.pace);
    }

    fn js(&self, expr: &str) -> Result<Value> {
        let r = self.cdp.call(
            "Runtime.evaluate",
            json!({ "expression": expr, "awaitPromise": true, "returnByValue": true, "userGesture": true }),
        )?;
        if let Some(e) = r.get("exceptionDetails") {
            let msg = e["exception"]["description"]
                .as_str()
                .or(e["text"].as_str())
                .unwrap_or("script error");
            bail!("{}", msg.lines().next().unwrap_or(msg));
        }
        Ok(r["result"]["value"].clone())
    }

    fn api(&self, f: &str, args: &[Value]) -> Result<Value> {
        let args: Vec<String> = args.iter().map(Value::to_string).collect();
        self.js(&format!(
            "window.__oxdemo ? window.__oxdemo.{f}({}) : null",
            args.join(",")
        ))
    }

    /// Wait for the page to stop changing: no DOM mutations, animations, image
    /// loads or requests for a moment. Returns how much changed.
    fn settle(&self) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut mutations = 0;
        loop {
            let r = self
                .api("settle", &[json!(220), json!(3000)])
                .unwrap_or(Value::Null);
            mutations += r["mutations"].as_u64().unwrap_or(0);
            if self.cdp.shared.inflight(Duration::from_secs(3)) == 0 || Instant::now() > deadline {
                return mutations;
            }
            self.sleep(0.05);
        }
    }

    /// Settle, then a pause sized to how much just changed.
    fn after(&self, base: f64) {
        let m = self.settle();
        let extra = match m {
            0 => 0.0,
            1..=9 => 0.12,
            10..=99 => 0.35,
            _ => 0.6,
        };
        self.beat(base + extra);
    }

    fn mouse_event(&mut self, kind: &str, x: f64, y: f64) -> Result<()> {
        let (button, buttons) = if kind == "mouseMoved" {
            (
                if self.pressed { "left" } else { "none" },
                if self.pressed { 1 } else { 0 },
            )
        } else {
            ("left", if kind == "mousePressed" { 1 } else { 0 })
        };
        self.cdp.call(
            "Input.dispatchMouseEvent",
            json!({ "type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": 1 }),
        )?;
        Ok(())
    }

    /// Move the pointer along a gentle arc with an eased speed, the way a hand does.
    fn glide(&mut self, x: f64, y: f64, drag: Option<&Value>) -> Result<()> {
        let (x0, y0) = self.mouse;
        let d = ((x - x0).powi(2) + (y - y0).powi(2)).sqrt();
        if d < 1.0 {
            return Ok(());
        }
        let dur = (0.3 + d / 1600.0).min(0.95) * self.pace;
        let steps = ((dur * 60.0).ceil() as usize).max(4);
        // Bow the path a little to one side; alternate sides so it never looks mechanical.
        self.arc = -self.arc;
        let bow = (d * 0.08).min(60.0) * self.arc;
        let (cx, cy) = (
            (x0 + x) / 2.0 - (y - y0) / d * bow,
            (y0 + y) / 2.0 + (x - x0) / d * bow,
        );
        let start = Instant::now();
        for i in 1..=steps {
            let s = ease(i as f64 / steps as f64);
            let px = (1.0 - s).powi(2) * x0 + 2.0 * (1.0 - s) * s * cx + s * s * x;
            let py = (1.0 - s).powi(2) * y0 + 2.0 * (1.0 - s) * s * cy + s * s * y;
            match drag {
                Some(data) => {
                    self.cdp.call(
                        "Input.dispatchDragEvent",
                        json!({ "type": "dragOver", "x": px, "y": py, "data": data }),
                    )?;
                }
                None => self.mouse_event("mouseMoved", px, py)?,
            }
            self.mouse = (px, py);
            self.take.moves.push((now(), px, py));
            let due = dur * i as f64 / steps as f64;
            let el = start.elapsed().as_secs_f64();
            if due > el {
                self.sleep(due - el);
            }
        }
        Ok(())
    }

    /// Find a target, scroll it into view if it is not, and return its centre.
    /// `@x,y` is a point on the page itself, for canvases and remote desktops.
    fn locate(&self, target: &str) -> Result<(f64, f64)> {
        if let Some(p) = point(target) {
            return Ok(p);
        }
        let r = self.api("resolve", &[json!(target)])?;
        if r.is_null() {
            bail!("the page has no oxdemo overlay yet (is it still loading?)");
        }
        if r["ok"] != true {
            let mut msg = r["error"].as_str().unwrap_or("no match").to_string();
            if let Some(c) = r["candidates"].as_array() {
                for c in c {
                    msg.push_str(&format!("\n    {}", c["desc"].as_str().unwrap_or("")));
                    if let Some(u) = c["use"].as_str() {
                        msg.push_str(&format!("\n        use: {}", Value::String(u.to_string())));
                    }
                }
            }
            if let Some(n) = r["near"].as_array().filter(|n| !n.is_empty()) {
                let names: Vec<String> = n.iter().map(|v| v.to_string()).collect();
                msg.push_str(&format!("\n    on screen: {}", names.join(", ")));
            }
            bail!(msg);
        }
        let mut b = r["box"].clone();
        if b["inView"] != true {
            self.api("scroll", &[])?;
            // Wait for the smooth scroll to come to rest.
            let mut last = String::new();
            for _ in 0..60 {
                self.sleep(0.05);
                b = self.api("box", &[])?;
                let now = b.to_string();
                if now == last {
                    break;
                }
                last = now;
            }
            self.beat(0.15);
        }
        let f = |k: &str| b[k].as_f64().unwrap_or(0.0);
        Ok((f("x") + f("w") / 2.0, f("y") + f("h") / 2.0))
    }

    fn move_to(&mut self, target: &str) -> Result<(f64, f64)> {
        let (x, y) = self.locate(target)?;
        self.glide(x, y, None)?;
        Ok((x, y))
    }

    fn click_here(&mut self, target: &str, button: &str, count: u32) -> Result<()> {
        let (x, y) = self.mouse;
        if point(target).is_none() {
            if let Some(b) = self.api("blocker", &[json!(x), json!(y)])?.as_str() {
                bail!("a click on {target:?} would land on {b}, which covers it");
            }
        }
        for n in 1..=count {
            for kind in ["mousePressed", "mouseReleased"] {
                self.cdp.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": kind, "x": x, "y": y, "button": button,
                            "buttons": if kind == "mousePressed" { if button == "right" { 2 } else { 1 } } else { 0 },
                            "clickCount": n }),
                )?;
                self.sleep(0.05);
            }
        }
        Ok(())
    }

    fn click(&mut self, target: &str) -> Result<()> {
        self.click_with(target, "left", 1)
    }

    fn click_with(&mut self, target: &str, button: &str, count: u32) -> Result<()> {
        self.move_to(target)?;
        self.beat(0.15);
        self.click_here(target, button, count)
    }

    /// Press, travel through each point, release: a brush stroke or a lasso.
    fn draw(&mut self, points: &[String]) -> Result<()> {
        let pts = points
            .iter()
            .map(|p| self.locate(p))
            .collect::<Result<Vec<_>>>()?;
        self.glide(pts[0].0, pts[0].1, None)?;
        self.beat(0.15);
        self.pressed = true;
        self.mouse_event("mousePressed", pts[0].0, pts[0].1)?;
        for &(x, y) in &pts[1..] {
            self.glide(x, y, None)?;
        }
        self.pressed = false;
        let (x, y) = self.mouse;
        self.mouse_event("mouseReleased", x, y)?;
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        for (i, c) in text.chars().enumerate() {
            let s = c.to_string();
            if c == '\n' {
                self.press(&keys::parse("Enter")?)?;
                continue;
            }
            self.cdp.call(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyDown", "text": s, "unmodifiedText": s, "key": s }),
            )?;
            self.cdp.call(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyUp", "key": s }),
            )?;
            // 45–60 ms a key, varied but repeatable.
            self.sleep((0.045 + ((i * 7919) % 16) as f64 / 1000.0) * self.pace);
        }
        Ok(())
    }

    fn boxes(&self) -> Value {
        self.api("boxes", &[]).unwrap_or(Value::Null)
    }

    fn rect(v: &Value) -> Option<[f64; 4]> {
        let a = v.as_array()?;
        Some([
            a.first()?.as_f64()?,
            a.get(1)?.as_f64()?,
            a.get(2)?.as_f64()?,
            a.get(3)?.as_f64()?,
        ])
    }

    fn press(&mut self, k: &keys::Key) -> Result<()> {
        if let Some(b) = &k.badge {
            self.api("badge", &[json!(b)])?;
            if let Some(r) = Self::rect(&self.boxes()["badge"]) {
                let t = now();
                self.take.pinned.push((t, t + 1.2, r, false));
            }
        }
        // Hold each modifier down first, the way a keyboard does. Remote desktops
        // only forward a modifier they saw go down.
        const MODS: [(u32, &str, &str, u32); 4] = [
            (2, "Control", "ControlLeft", 17),
            (8, "Shift", "ShiftLeft", 16),
            (1, "Alt", "AltLeft", 18),
            (4, "Meta", "MetaLeft", 91),
        ];
        let held: Vec<_> = MODS.iter().filter(|m| k.modifiers & m.0 != 0).collect();
        let mut state = 0;
        for m in &held {
            state |= m.0;
            self.cdp.call(
                "Input.dispatchKeyEvent",
                json!({ "type": "rawKeyDown", "key": m.1, "code": m.2,
                "windowsVirtualKeyCode": m.3, "nativeVirtualKeyCode": m.3, "modifiers": state }),
            )?;
        }
        let mut down = json!({ "type": if k.text.is_some() { "keyDown" } else { "rawKeyDown" }, "key": k.key, "code": k.code,
            "windowsVirtualKeyCode": k.key_code, "nativeVirtualKeyCode": k.key_code, "modifiers": k.modifiers });
        if let Some(t) = &k.text {
            down["text"] = json!(t);
            down["unmodifiedText"] = json!(t);
        }
        self.cdp.call("Input.dispatchKeyEvent", down)?;
        self.sleep(0.03);
        self.cdp.call("Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": k.key, "code": k.code,
            "windowsVirtualKeyCode": k.key_code, "nativeVirtualKeyCode": k.key_code, "modifiers": k.modifiers }))?;
        for m in held.iter().rev() {
            state &= !m.0;
            self.cdp.call(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyUp", "key": m.1, "code": m.2,
                "windowsVirtualKeyCode": m.3, "nativeVirtualKeyCode": m.3, "modifiers": state }),
            )?;
        }
        Ok(())
    }

    fn hold_caption(&mut self) {
        if let Some((since, need)) = self.caption_hold.take() {
            // Reading speed does not change with pace.
            let left = since + need - now();
            self.sleep(left);
        }
    }

    fn goto(&mut self, url: &str) -> Result<()> {
        // The app may still be starting up after `before`, so retry for a while.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let r = self.cdp.call("Page.navigate", json!({ "url": url }))?;
            match r["errorText"].as_str() {
                None | Some("") => break,
                Some(e) if Instant::now() > deadline => bail!("could not open {url}: {e}"),
                Some(_) => self.sleep(0.5),
            }
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while self
            .js("document.readyState")
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .as_deref()
            != Some("complete")
        {
            if Instant::now() > deadline {
                bail!("{url} never finished loading");
            }
            self.sleep(0.05);
        }
        self.settle();
        let (x, y) = self.mouse;
        self.mouse_event("mouseMoved", x, y)?;
        Ok(())
    }

    fn drag(&mut self, from: &str, to: &str) -> Result<()> {
        self.move_to(from)?;
        self.beat(0.2);
        let (x0, y0) = self.mouse;
        self.api("dragStarted", &[])?;
        *self.cdp.shared.drag.lock().unwrap() = None;
        self.cdp
            .call("Input.setInterceptDrags", json!({ "enabled": true }))?;
        self.pressed = true;
        self.mouse_event("mousePressed", x0, y0)?;
        self.sleep(0.08);
        // Nudge past the drag threshold so the page decides whether this is a drag.
        self.mouse_event("mouseMoved", x0 + 6.0, y0 + 4.0)?;
        self.mouse = (x0 + 6.0, y0 + 4.0);
        let html5 = self.api("dragStarted", &[])?.as_bool().unwrap_or(false);
        let mut data = None;
        if html5 {
            // An HTML5 drag: Chrome hands the drag to us, and we play the rest of it.
            for _ in 0..40 {
                if let Some(d) = self.cdp.shared.drag.lock().unwrap().take() {
                    data = Some(d);
                    break;
                }
                self.sleep(0.025);
            }
        }
        self.cdp
            .call("Input.setInterceptDrags", json!({ "enabled": false }))?;
        let (x, y) = self.locate(to)?;
        if x < 0.0
            || y < 0.0
            || x > self.script.settings.width as f64
            || y > self.script.settings.height as f64
        {
            bail!("the drop target {to:?} is off screen, so a drop there would land nowhere");
        }
        if let Some(d) = &data {
            let (mx, my) = self.mouse;
            self.cdp.call(
                "Input.dispatchDragEvent",
                json!({ "type": "dragEnter", "x": mx, "y": my, "data": d }),
            )?;
        }
        // A canvas has no DOM to compare, so a drop onto a point is taken on trust.
        let check = point(from).is_none() && point(to).is_none();
        let before = self.api("signature", &[])?;
        self.glide(x, y, data.as_ref())?;
        self.beat(0.25);
        match &data {
            Some(d) => {
                self.cdp.call(
                    "Input.dispatchDragEvent",
                    json!({ "type": "drop", "x": x, "y": y, "data": d }),
                )?;
                self.pressed = false;
                self.mouse_event("mouseReleased", x, y)?;
            }
            None => {
                self.pressed = false;
                self.mouse_event("mouseReleased", x, y)?;
            }
        }
        self.settle();
        let after = self.api("signature", &[])?;
        if check && before == after {
            bail!("dropped {from:?} on {to:?} and nothing on the page changed");
        }
        Ok(())
    }

    fn step(&mut self, step: &Step) -> Result<()> {
        match &step.action {
            Action::Say(text) => {
                self.hold_caption();
                let t = now();
                // The previous caption stays pinned through its fade-out.
                if let Some(last) = self.take.pinned.iter_mut().rev().find(|p| p.3) {
                    if last.1 == f64::INFINITY {
                        last.1 = t + 0.35;
                    }
                }
                self.api("caption", &[json!(text)])?;
                self.take.captions.push((t, text.clone()));
                if let Some(r) = Self::rect(&self.boxes()["caption"]) {
                    self.take.pinned.push((t, f64::INFINITY, r, true));
                }
                if text.is_empty() {
                    self.beat(0.35);
                } else {
                    self.caption_hold = Some((now(), reading_time(text)));
                    self.beat(0.6);
                }
            }
            Action::Click(t) => {
                self.click(t)?;
                self.after(0.35);
            }
            Action::DoubleClick(t) => {
                self.click_with(t, "left", 2)?;
                self.after(0.35);
            }
            Action::RightClick(t) => {
                self.click_with(t, "right", 1)?;
                self.after(0.35);
            }
            Action::Draw(points) => {
                self.draw(points)?;
                self.after(0.3);
            }
            Action::Snap(file) => {
                let r = self
                    .cdp
                    .call("Page.captureScreenshot", json!({ "format": "png" }))?;
                let data = r["data"].as_str().unwrap_or("");
                let bytes =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)?;
                std::fs::write(file, bytes)
                    .with_context(|| format!("writing {}", file.display()))?;
            }
            Action::Hover(t) => {
                self.move_to(t)?;
                self.after(0.5);
            }
            Action::Type { target, text } => {
                if let Some(t) = target {
                    self.click(t)?;
                    self.beat(0.15);
                }
                self.type_text(text)?;
                self.after(0.3);
            }
            Action::Clear(t) => {
                self.click(t)?;
                self.beat(0.15);
                self.api("clear", &[])?;
                self.press(&keys::parse("Backspace")?)?;
                self.after(0.2);
            }
            Action::Key(spec) => {
                let k = keys::parse(spec)?;
                self.press(&k)?;
                self.after(if k.badge.is_some() { 0.7 } else { 0.25 });
            }
            Action::Pick { target, value } => {
                // An open native dropdown does not render in headless video, so travel
                // to the control, pause, and set it.
                self.move_to(target)?;
                self.beat(0.25);
                let r = self.api("pick", &[json!(value)])?;
                if r["ok"] != true {
                    bail!("{}", r["error"].as_str().unwrap_or("could not pick"));
                }
                self.after(0.8);
            }
            Action::Drag { from, to } => {
                self.drag(from, to)?;
                self.after(0.5);
            }
            Action::Scroll(t) => {
                self.locate(t)?;
                self.after(0.3);
            }
            Action::Upload { target, file } => {
                if !file.is_file() {
                    bail!("no such file {}", file.display());
                }
                *self.cdp.shared.chooser.lock().unwrap() = None;
                self.click(target)?;
                let mut node = None;
                for _ in 0..120 {
                    if let Some(n) = self.cdp.shared.chooser.lock().unwrap().take() {
                        node = Some(n);
                        break;
                    }
                    self.sleep(0.025);
                }
                let node =
                    node.ok_or_else(|| anyhow!("clicking {target:?} opened no file chooser"))?;
                let abs = std::fs::canonicalize(file)?;
                self.cdp.call(
                    "DOM.setFileInputFiles",
                    json!({ "files": [abs], "backendNodeId": node }),
                )?;
                self.after(0.6);
            }
            Action::WaitFor { target, timeout } => {
                let deadline = Instant::now() + Duration::from_secs_f64(*timeout);
                loop {
                    match self.api("resolve", &[json!(target)]) {
                        Ok(r) if r["ok"] == true => break,
                        Ok(r) if Instant::now() > deadline => {
                            bail!(
                                "waited {timeout}s: {}",
                                r["error"].as_str().unwrap_or("never appeared")
                            )
                        }
                        Err(e) if Instant::now() > deadline => return Err(e),
                        _ => self.sleep(0.1),
                    }
                }
                self.after(0.2);
            }
            Action::Goto(url) => {
                self.goto(url)?;
                self.beat(0.4);
            }
            Action::Zoom(z) => {
                self.take.zooms.push((now(), *z));
            }
            Action::Pause(s) => self.sleep(*s),
            Action::Skip(s) => {
                let t = now();
                self.sleep(*s);
                self.take.cuts.push((t, now()));
            }
            Action::Eval(src) => {
                self.js(src)?;
                self.after(0.2);
            }
        }
        Ok(())
    }
}

fn run_before(script: &Script) -> Result<()> {
    for cmd in &script.settings.before {
        eprintln!("  before: {cmd}");
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(script.dir())
            .status()
            .with_context(|| format!("running {cmd:?}"))?;
        if !status.success() {
            bail!("`before` command failed ({status}): {cmd}");
        }
    }
    Ok(())
}

fn overlay(script: &Script) -> String {
    let t = &script.settings.theme;
    let theme = json!({
        "font": t.font, "size": t.size, "caption_bg": t.caption_bg, "caption_fg": t.caption_fg,
        "cursor": t.cursor, "cursor_stroke": t.cursor_stroke, "ring": t.ring, "position": t.position,
        "css": script.settings.styles.join("\n"),
    });
    include_str!("overlay.js").replace("__OXDEMO_THEME__", &theme.to_string())
}

/// Headless Chrome keeps room for browser UI inside its window, and the screencast
/// only sees what fits, so grow the window until the whole viewport does. The
/// spare strip below the viewport is cropped off when rendering.
fn fit_window(cdp: &Cdp, width: u32, height: u32) -> Result<()> {
    let Ok(w) = cdp.call("Browser.getWindowForTarget", json!({})) else {
        return Ok(());
    };
    let id = w["windowId"].clone();
    let r = cdp.call("Runtime.evaluate", json!({ "expression": "[outerWidth - innerWidth, outerHeight - innerHeight]", "returnByValue": true }))?;
    let dx = r["result"]["value"][0].as_u64().unwrap_or(0) as u32;
    let dy = r["result"]["value"][1].as_u64().unwrap_or(0) as u32;
    cdp.call(
        "Browser.setWindowBounds",
        json!({ "windowId": id, "bounds": { "width": width + dx, "height": height + dy + 40, "windowState": "normal" } }),
    )?;
    Ok(())
}

/// Run one take. Frames land in `frame_dir`.
pub fn record(script: &Script, frame_dir: &Path, headed: bool, progress: bool) -> Result<Take> {
    run_before(script)?;
    let s = &script.settings;
    let browser = Browser::launch(s.width, s.height, s.scale, headed)?;
    let cdp = &browser.cdp;
    for m in [
        "Page.enable",
        "Runtime.enable",
        "Network.enable",
        "DOM.enable",
    ] {
        cdp.call(m, json!({}))?;
    }
    cdp.call("Page.setBypassCSP", json!({ "enabled": true }))?;
    fit_window(cdp, s.width, s.height)?;
    cdp.call(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": s.width, "height": s.height, "deviceScaleFactor": s.scale, "mobile": false }),
    )?;
    cdp.call(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": overlay(script) }),
    )?;
    cdp.call(
        "Page.setInterceptFileChooserDialog",
        json!({ "enabled": true }),
    )?;

    let mut r = Runner {
        cdp,
        script,
        pace: s.pace,
        mouse: (s.width as f64 / 2.0, s.height as f64 / 2.0),
        pressed: false,
        arc: 1.0,
        caption_hold: None,
        take: Take {
            frames: vec![],
            start: 0.0,
            end: 0.0,
            width: s.width,
            height: s.height,
            scale: s.scale,
            moves: vec![],
            zooms: vec![],
            cuts: vec![],
            captions: vec![],
            pinned: vec![],
            marks: vec![],
            failure: None,
        },
    };

    // Open the app before recording starts, so frame one is the product, not a blank page.
    let mut steps: &[Step] = &script.steps;
    let first = match (&s.url, steps.first()) {
        (Some(u), _) => u.clone(),
        (
            None,
            Some(Step {
                action: Action::Goto(u),
                ..
            }),
        ) => {
            steps = &steps[1..];
            u.clone()
        }
        _ => unreachable!("the parser requires a url"),
    };
    r.goto(&first)?;

    std::fs::create_dir_all(frame_dir)?;
    *cdp.shared.frame_dir.lock().unwrap() = Some(frame_dir.to_path_buf());
    cdp.shared.recording.store(true, Ordering::SeqCst);
    let (w, h) = (
        (s.width as f64 * s.scale) as u32,
        (s.height as f64 * s.scale) as u32,
    );
    cdp.call(
        "Page.startScreencast",
        json!({ "format": "jpeg", "quality": 92, "maxWidth": w * 2, "maxHeight": h * 2, "everyNthFrame": 1 }),
    )?;
    r.take.start = now();
    let (mx, my) = r.mouse;
    r.take.moves.push((r.take.start, mx, my));
    r.beat(0.5);

    for step in steps {
        if progress {
            eprintln!("  {:>4}  {}", step.line, step.source);
        }
        r.take.marks.push((now(), step.line));
        if let Err(e) = r.step(step) {
            let t = now();
            let shot = frame_dir.join("failed.png");
            let screenshot = cdp
                .call("Page.captureScreenshot", json!({ "format": "png" }))
                .ok()
                .and_then(|v| v["data"].as_str().map(str::to_string))
                .and_then(|d| {
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, d).ok()
                })
                .and_then(|b| std::fs::write(&shot, b).ok())
                .map(|_| shot);
            r.take.failure = Some(Failure {
                t,
                line: step.line,
                message: format!("{e:#}"),
                screenshot,
            });
            break;
        }
    }
    if r.take.failure.is_none() {
        r.hold_caption();
        r.sleep(s.tail);
    }
    let _ = cdp.call("Page.stopScreencast", json!({}));
    cdp.shared.recording.store(false, Ordering::SeqCst);
    r.take.end = now();
    let mut take = r.take;
    take.frames = cdp.shared.frames.lock().unwrap().clone();
    take.frames.sort_by(|a, b| a.t.total_cmp(&b.t));
    for e in cdp.shared.errors.lock().unwrap().iter() {
        eprintln!("  page error: {e}");
    }
    drop(browser);
    if take.frames.is_empty() {
        bail!("the browser sent no frames");
    }
    Ok(take)
}

/// `@x,y`: a point on the page, in CSS pixels.
fn point(target: &str) -> Option<(f64, f64)> {
    let (x, y) = target.strip_prefix('@')?.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_map_both_ways() {
        let take = Take {
            frames: vec![],
            start: 100.0,
            end: 130.0,
            width: 10,
            height: 10,
            scale: 1.0,
            moves: vec![],
            zooms: vec![],
            cuts: vec![(105.0, 115.0)],
            captions: vec![],
            pinned: vec![],
            marks: vec![],
            failure: None,
        };
        assert_eq!(take.length(), 20.0);
        assert_eq!(take.to_take(3.0), 3.0);
        assert_eq!(take.to_take(6.0), 16.0);
        assert_eq!(take.to_video(103.0), 3.0);
        assert_eq!(take.to_video(110.0), 5.0);
        assert_eq!(take.to_video(120.0), 10.0);
    }
}
