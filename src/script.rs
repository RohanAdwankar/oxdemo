//! The take script: one command per line, VHS-style.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Theme {
    pub font: String,
    pub size: u32,
    pub caption_bg: String,
    pub caption_fg: String,
    pub cursor: String,
    pub cursor_stroke: String,
    pub ring: String,
    pub position: String,
    /// Pixels between the caption and the edge of the screen.
    pub offset: u32,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            font: "system-ui, -apple-system, 'Segoe UI', sans-serif".into(),
            size: 17,
            caption_bg: "rgba(33,29,25,.92)".into(),
            caption_fg: "#fffdfa".into(),
            cursor: "#211d19".into(),
            cursor_stroke: "#fffdfa".into(),
            ring: "#0f7a5c".into(),
            position: "bottom".into(),
            offset: 26,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Video {
    pub fps: u32,
    pub width: Option<u32>,
    pub crf: u32,
}

#[derive(Debug, Clone)]
pub struct Gif {
    pub fps: u32,
    pub width: u32,
    /// Where the loop begins, in seconds into the take. Frame one is the most-seen frame.
    pub start: Option<f64>,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub url: Option<String>,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub before: Vec<String>,
    pub outputs: Vec<PathBuf>,
    pub watch: Vec<PathBuf>,
    /// CSS added to every page before recording, e.g. to hide a cookie banner.
    pub styles: Vec<String>,
    pub theme: Theme,
    pub video: Video,
    pub gif: Gif,
    /// Multiplies every automatic pause and cursor move. 1.0 is the default feel.
    pub pace: f64,
    /// Seconds to hold the last frame before the take ends.
    pub tail: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            url: None,
            width: 1440,
            height: 900,
            scale: 1.0,
            before: vec![],
            outputs: vec![],
            watch: vec![],
            styles: vec![],
            theme: Theme::default(),
            video: Video {
                fps: 30,
                width: None,
                crf: 22,
            },
            gif: Gif {
                fps: 12,
                width: 800,
                start: None,
                from: None,
                to: None,
            },
            pace: 1.0,
            tail: 1.2,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    Say(String),
    Click(String),
    Hover(String),
    Type {
        target: Option<String>,
        text: String,
    },
    Clear(String),
    Key(String),
    Pick {
        target: String,
        value: String,
    },
    Drag {
        from: String,
        to: String,
    },
    Scroll(String),
    Upload {
        target: String,
        file: PathBuf,
    },
    WaitFor {
        target: String,
        timeout: f64,
    },
    Goto(String),
    Zoom(f64),
    Pause(f64),
    Eval(String),
    DoubleClick(String),
    RightClick(String),
    Draw(Vec<String>),
    Snap(PathBuf),
    Skip(f64),
    Wheel {
        dy: f64,
        secs: f64,
    },
}

#[derive(Debug, Clone)]
pub struct Step {
    pub line: usize,
    pub source: String,
    pub action: Action,
}

#[derive(Debug, Clone)]
pub struct Script {
    pub path: PathBuf,
    pub settings: Settings,
    pub steps: Vec<Step>,
}

impl Script {
    pub fn dir(&self) -> PathBuf {
        match self.path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => ".".into(),
        }
    }

    pub fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "demo".into())
    }

    pub fn load(path: &Path) -> Result<Script> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut script = parse(&text).map_err(|e| anyhow!("{}:{}", path.display(), e))?;
        script.path = path.to_path_buf();
        let dir = script.dir();
        // A bare path is a local page relative to the script; a bare host:port is http.
        let local = |u: &mut String| -> Result<()> {
            if u.contains("://") || u.starts_with("about:") || u.starts_with("data:") {
                return Ok(());
            }
            if let Ok(abs) = std::fs::canonicalize(dir.join(&*u)) {
                *u = format!("file://{}", abs.display());
            } else if u.starts_with('/') {
                // An absolute path may be made by `before`, so it need not exist yet.
                *u = format!("file://{u}");
            } else if u.starts_with("localhost")
                || u.split('/').next().is_some_and(|h| h.contains(':'))
            {
                *u = format!("http://{u}");
            } else {
                bail!("{u:?} is neither a file next to the script nor a URL");
            }
            Ok(())
        };
        if let Some(u) = script.settings.url.as_mut() {
            local(u).map_err(|e| anyhow!("{}: {e}", path.display()))?;
        }
        for step in script.steps.iter_mut() {
            if let Action::Goto(u) = &mut step.action {
                local(u).map_err(|e| anyhow!("{}:{}: {e}", path.display(), step.line))?;
            }
        }
        for p in script
            .settings
            .outputs
            .iter_mut()
            .chain(script.settings.watch.iter_mut())
        {
            if p.is_relative() {
                *p = dir.join(&*p);
            }
        }
        for step in script.steps.iter_mut() {
            if let Action::Upload { file, .. } | Action::Snap(file) = &mut step.action {
                if file.is_relative() {
                    *file = dir.join(&*file);
                }
            }
        }
        Ok(script)
    }
}

/// Split a line into words. Double quotes group, and may start mid-word (`font="IBM Plex"`).
pub fn tokenize(line: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut in_quote = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quote = !in_quote;
                quoted = true;
            }
            '\\' if in_quote => match chars.next() {
                Some('n') => cur.push('\n'),
                Some(o) => cur.push(o),
                None => bail!("line ends in a backslash"),
            },
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() || quoted {
                    out.push(std::mem::take(&mut cur));
                }
                quoted = false;
            }
            c => cur.push(c),
        }
    }
    if in_quote {
        bail!("unclosed quote");
    }
    if !cur.is_empty() || quoted {
        out.push(cur);
    }
    Ok(out)
}

/// `1.5s`, `800ms`, `2` (seconds).
pub fn duration(s: &str) -> Result<f64> {
    let (num, mul) = if let Some(n) = s.strip_suffix("ms") {
        (n, 0.001)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1.0)
    } else {
        (s, 1.0)
    };
    let v: f64 = num
        .parse()
        .map_err(|_| anyhow!("not a duration: {s:?} (try 1.5s or 800ms)"))?;
    Ok(v * mul)
}

fn kv(words: &[String]) -> Result<Vec<(String, String)>> {
    words
        .iter()
        .map(|w| {
            w.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .ok_or_else(|| anyhow!("expected key=value, got {w:?}"))
        })
        .collect()
}

fn num<T: std::str::FromStr>(k: &str, v: &str) -> Result<T> {
    v.parse().map_err(|_| anyhow!("{k}={v:?} is not a number"))
}

pub fn parse(text: &str) -> Result<Script> {
    let mut s = Settings::default();
    let mut steps = vec![];
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        let words = tokenize(trimmed).map_err(|e| anyhow!("{line}: {e}"))?;
        let cmd = words[0].to_lowercase();
        let args = &words[1..];
        let want = |n: usize, usage: &str| -> Result<()> {
            if args.len() != n {
                bail!("{line}: usage: {usage}");
            }
            Ok(())
        };
        let mut push = |action| {
            steps.push(Step {
                line,
                source: trimmed.to_string(),
                action,
            })
        };
        let r: Result<()> = (|| {
            match cmd.as_str() {
                "url" => {
                    want(1, "url <address>")?;
                    s.url = Some(args[0].clone());
                }
                "viewport" => {
                    want(1, "viewport 1440x900")?;
                    let (w, h) = args[0]
                        .split_once('x')
                        .ok_or_else(|| anyhow!("{line}: usage: viewport 1440x900"))?;
                    s.width = num("width", w)?;
                    s.height = num("height", h)?;
                }
                "scale" => {
                    want(1, "scale 2")?;
                    s.scale = num("scale", &args[0])?;
                }
                "before" => {
                    want(1, "before \"./reset.sh && python3 seed.py\"")?;
                    s.before.push(args[0].clone());
                }
                "output" => {
                    if args.is_empty() {
                        bail!("{line}: usage: output demo.mp4 demo.gif");
                    }
                    s.outputs.extend(args.iter().map(PathBuf::from));
                }
                "watch" => s.watch.extend(args.iter().map(PathBuf::from)),
                "style" | "css" => {
                    want(1, "style \"#banner { display: none }\"")?;
                    s.styles.push(args[0].clone());
                }
                "pace" => {
                    want(1, "pace 1.0")?;
                    s.pace = num("pace", &args[0])?;
                }
                "tail" => {
                    want(1, "tail 1.5s")?;
                    s.tail = duration(&args[0])?;
                }
                "theme" => {
                    for (k, v) in kv(args)? {
                        let t = &mut s.theme;
                        match k.as_str() {
                            "font" => t.font = if v.contains(',') { v } else { format!("\"{v}\", system-ui, sans-serif") },
                            "size" => t.size = num(&k, &v)?,
                            "caption" | "caption-bg" => t.caption_bg = v,
                            "text" | "caption-fg" => t.caption_fg = v,
                            "cursor" => t.cursor = v,
                            "cursor-stroke" => t.cursor_stroke = v,
                            "ring" => t.ring = v,
                            "offset" => t.offset = num(&k, &v)?,
                            "position" => {
                                if v != "top" && v != "bottom" {
                                    bail!("position is top or bottom");
                                }
                                t.position = v
                            }
                            _ => bail!("unknown theme key {k:?} (font, size, caption, text, cursor, cursor-stroke, ring, position, offset)"),
                        }
                    }
                }
                "video" => {
                    for (k, v) in kv(args)? {
                        match k.as_str() {
                            "fps" => s.video.fps = num(&k, &v)?,
                            "width" => s.video.width = Some(num(&k, &v)?),
                            "crf" => s.video.crf = num(&k, &v)?,
                            _ => bail!("unknown video key {k:?} (fps, width, crf)"),
                        }
                    }
                }
                "gif" => {
                    for (k, v) in kv(args)? {
                        match k.as_str() {
                            "fps" => s.gif.fps = num(&k, &v)?,
                            "width" => s.gif.width = num(&k, &v)?,
                            "start" => s.gif.start = Some(duration(&v)?),
                            "from" => s.gif.from = Some(duration(&v)?),
                            "to" => s.gif.to = Some(duration(&v)?),
                            _ => bail!("unknown gif key {k:?} (fps, width, start, from, to)"),
                        }
                    }
                }
                "say" => match args.len() {
                    0 => push(Action::Say(String::new())),
                    1 => push(Action::Say(args[0].clone())),
                    _ => bail!("{line}: say takes one quoted sentence"),
                },
                "click" => {
                    want(1, "click <target>")?;
                    push(Action::Click(args[0].clone()));
                }
                "double-click" | "dclick" => {
                    want(1, "double-click <target>")?;
                    push(Action::DoubleClick(args[0].clone()));
                }
                "right-click" | "rclick" => {
                    want(1, "right-click <target>")?;
                    push(Action::RightClick(args[0].clone()));
                }
                "draw" => {
                    if args.len() < 2 {
                        bail!("{line}: usage: draw @x,y @x,y ... (two or more points)");
                    }
                    push(Action::Draw(args.to_vec()));
                }
                "snap" => {
                    want(1, "snap still.png")?;
                    push(Action::Snap(PathBuf::from(&args[0])));
                }
                "hover" => {
                    want(1, "hover <target>")?;
                    push(Action::Hover(args[0].clone()));
                }
                "type" => match args.len() {
                    1 => push(Action::Type {
                        target: None,
                        text: args[0].clone(),
                    }),
                    2 => push(Action::Type {
                        target: Some(args[0].clone()),
                        text: args[1].clone(),
                    }),
                    _ => bail!("{line}: usage: type [<target>] \"text\""),
                },
                "clear" => {
                    want(1, "clear <target>")?;
                    push(Action::Clear(args[0].clone()));
                }
                "key" | "press" => {
                    want(1, "key Enter | key Meta+K")?;
                    crate::keys::parse(&args[0]).map_err(|e| anyhow!("{line}: {e}"))?;
                    push(Action::Key(args[0].clone()));
                }
                "pick" | "select" => {
                    want(2, "pick <select> <value>")?;
                    push(Action::Pick {
                        target: args[0].clone(),
                        value: args[1].clone(),
                    });
                }
                "drag" => {
                    if args.len() != 3 || args[1] != "to" {
                        bail!("{line}: usage: drag <target> to <target>");
                    }
                    push(Action::Drag {
                        from: args[0].clone(),
                        to: args[2].clone(),
                    });
                }
                "scroll" => {
                    want(1, "scroll <target>")?;
                    push(Action::Scroll(args[0].clone()));
                }
                "upload" => {
                    want(2, "upload <target> <file>")?;
                    push(Action::Upload {
                        target: args[0].clone(),
                        file: PathBuf::from(&args[1]),
                    });
                }
                "wait-for" | "waitfor" => match args.len() {
                    1 => push(Action::WaitFor {
                        target: args[0].clone(),
                        timeout: 10.0,
                    }),
                    2 => push(Action::WaitFor {
                        target: args[0].clone(),
                        timeout: duration(&args[1])?,
                    }),
                    _ => bail!("{line}: usage: wait-for <target> [10s]"),
                },
                "goto" | "open" => {
                    want(1, "goto <address>")?;
                    push(Action::Goto(args[0].clone()));
                }
                "zoom" => {
                    want(1, "zoom 1.8 | zoom off")?;
                    let z = if args[0] == "off" {
                        1.0
                    } else {
                        num::<f64>("zoom", args[0].trim_end_matches('x'))?
                    };
                    if !(1.0..=4.0).contains(&z) {
                        bail!("{line}: zoom is between 1 and 4");
                    }
                    push(Action::Zoom(z));
                }
                "wheel" => match args.len() {
                    1 | 2 => push(Action::Wheel {
                        dy: num("wheel", &args[0])?,
                        secs: args.get(1).map(|a| duration(a)).transpose()?.unwrap_or(1.0),
                    }),
                    _ => bail!("{line}: usage: wheel 600 [1.5s]"),
                },
                "skip" => {
                    want(1, "skip 10s")?;
                    push(Action::Skip(duration(&args[0])?));
                }
                "pause" | "wait" | "sleep" => {
                    want(1, "pause 1.5s")?;
                    push(Action::Pause(duration(&args[0])?));
                }
                "eval" | "js" => {
                    want(1, "eval \"javascript\"")?;
                    push(Action::Eval(args[0].clone()));
                }
                other => bail!("{line}: unknown command {other:?}"),
            }
            Ok(())
        })();
        r.map_err(|e| {
            let m = e.to_string();
            if m.starts_with(&format!("{line}:")) {
                e
            } else {
                anyhow!("{line}: {m}")
            }
        })?;
    }
    if s.url.is_none()
        && !matches!(
            steps.first(),
            Some(Step {
                action: Action::Goto(_),
                ..
            })
        )
    {
        bail!("1: the script needs a `url` to open");
    }
    Ok(Script {
        path: PathBuf::new(),
        settings: s,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_and_quotes() {
        assert_eq!(
            tokenize(r#"say "Hello, \"you\"" x"#).unwrap(),
            vec!["say", "Hello, \"you\"", "x"]
        );
        assert_eq!(
            tokenize(r#"theme font="IBM Plex Sans" ring=#0f7a5c"#).unwrap(),
            vec!["theme", "font=IBM Plex Sans", "ring=#0f7a5c"]
        );
        assert_eq!(tokenize(r#"say """#).unwrap(), vec!["say", ""]);
        assert!(tokenize(r#"say "open"#).is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(duration("800ms").unwrap(), 0.8);
        assert_eq!(duration("1.5s").unwrap(), 1.5);
        assert_eq!(duration("2").unwrap(), 2.0);
        assert!(duration("soon").is_err());
    }

    #[test]
    fn a_whole_script() {
        let s = parse(
            r#"
# comment
url http://localhost:3000
viewport 1280x800
theme font="IBM Plex Sans" caption=#211d19
gif width=640 start=3s
say "Make a board."
click "new board"
type "board name" "Roadmap"
key Enter
drag "Sidecar" to "css:[data-cell=agents-building]"
zoom 2
pause 800ms
"#,
        )
        .unwrap();
        assert_eq!(s.settings.width, 1280);
        assert_eq!(s.settings.gif.start, Some(3.0));
        assert!(s.settings.theme.font.starts_with("\"IBM Plex Sans\""));
        assert_eq!(s.steps.len(), 7);
        assert_eq!(s.steps[0].line, 7);
        assert!(matches!(&s.steps[3].action, Action::Key(k) if k == "Enter"));
    }

    #[test]
    fn errors_name_the_line() {
        let e = parse("url x\nclick\n").unwrap_err().to_string();
        assert!(e.starts_with("2: usage: click"), "{e}");
        let e = parse("url x\nfrobnicate\n").unwrap_err().to_string();
        assert!(e.contains("2: unknown command"), "{e}");
        let e = parse("url x\nkey Hyper+Q\n").unwrap_err().to_string();
        assert!(e.starts_with("2:"), "{e}");
    }
}
