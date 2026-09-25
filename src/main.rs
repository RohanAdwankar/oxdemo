mod cdp;
mod keys;
mod render;
mod script;
mod take;

use anyhow::{bail, Result};
use script::Script;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const HELP: &str = "oxdemo: scripted product demos with a real cursor, captions and zoom

usage:
  oxdemo record <script> [--out DIR] [--watch] [--headed] [--keep-frames] [--quiet]
  oxdemo check <script>        parse the script and report errors, without recording
  oxdemo init [file]           write a starter script (default: demo.oxd)

record writes, next to the script unless `output` or --out says otherwise:
  <name>.mp4         the video (needs an ffmpeg with H.264)
  <name>.gif         a looping GIF with one shared palette
  <name>-sheet.png   every few seconds of the take on one image
  <name>.vtt         the captions, as subtitles or chapters

environment:
  OXDEMO_CHROME   path to Chrome or Chromium
  OXDEMO_FFMPEG   path to ffmpeg
";

const STARTER: &str = r#"# An oxdemo take. One command per line; see the README for all of them.
url http://localhost:3000
viewport 1440x900
# before "./reset.sh && python3 seed.py"
# theme font="IBM Plex Sans" caption=#211d19 ring=#0f7a5c
# gif width=800 fps=12 start=2s

say "One sentence that says what this beat shows."
click "New board"
type "board name" "Roadmap"
key Enter
zoom 1.8
pick "columns" "stage"
zoom off
say ""
"#;

struct Opts {
    out: Option<PathBuf>,
    watch: bool,
    headed: bool,
    keep: bool,
    quiet: bool,
}

fn outputs(script: &Script, out: Option<&Path>) -> Vec<PathBuf> {
    if !script.settings.outputs.is_empty() {
        return match out {
            Some(dir) => script
                .settings
                .outputs
                .iter()
                .map(|p| dir.join(p.file_name().unwrap()))
                .collect(),
            None => script.settings.outputs.clone(),
        };
    }
    let dir = out.map(Path::to_path_buf).unwrap_or_else(|| script.dir());
    let stem = script.stem();
    ["mp4", "gif", "sheet.png", "vtt"]
        .iter()
        .map(|e| {
            if *e == "sheet.png" {
                dir.join(format!("{stem}-sheet.png"))
            } else {
                dir.join(format!("{stem}.{e}"))
            }
        })
        .collect()
}

fn once(path: &Path, o: &Opts) -> Result<bool> {
    let script = Script::load(path)?;
    let files = outputs(&script, o.out.as_deref());
    if let Some(d) = &o.out {
        std::fs::create_dir_all(d)?;
    }
    let frames = files[0].with_extension("frames");
    let _ = std::fs::remove_dir_all(&frames);
    let started = Instant::now();
    eprintln!("recording {}", path.display());
    let take = take::record(&script, &frames, o.headed, !o.quiet)?;
    let secs = take.end - take.start;
    eprintln!(
        "  {:.1}s take, {} frames, recorded in {:.1}s",
        secs,
        take.frames.len(),
        started.elapsed().as_secs_f64()
    );

    let r = render::Renderer::new(&take);
    let mut wrote = vec![];
    for f in &files {
        let name = f.to_string_lossy();
        let res = if name.ends_with(".mp4") {
            render::mp4(&r, &script.settings, f)
        } else if name.ends_with(".gif") {
            render::gif(&r, &script.settings, f)
        } else if name.ends_with(".png") {
            render::sheet(
                &r,
                f,
                take.failure.as_ref().and_then(|x| x.screenshot.as_deref()),
            )
        } else if name.ends_with(".vtt") {
            render::vtt(&take, f)
        } else {
            Err(anyhow::anyhow!(
                "don't know how to write {name} (mp4, gif, png, vtt)"
            ))
        };
        match res {
            Ok(()) => wrote.push(f.clone()),
            Err(e) => eprintln!("  skipped {}: {e:#}", f.display()),
        }
    }
    if let Some(fail) = &take.failure {
        if let Some(shot) = &fail.screenshot {
            let dest = files[0].with_file_name(format!("{}-failed.png", script.stem()));
            if std::fs::copy(shot, &dest).is_ok() {
                wrote.push(dest);
            }
        }
    }
    for f in &wrote {
        let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        eprintln!("  wrote {} ({})", f.display(), human(size));
    }
    if !o.keep {
        let _ = std::fs::remove_dir_all(&frames);
    }
    if let Some(fail) = &take.failure {
        eprintln!(
            "\n{}:{}: failed {:.1}s into the take:\n  {}",
            path.display(),
            fail.line,
            fail.t - take.start,
            fail.message.replace('\n', "\n  ")
        );
        return Ok(false);
    }
    Ok(true)
}

fn human(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else {
        format!("{} KB", n.div_ceil(1024))
    }
}

/// The newest modification time under the script and any `watch` paths.
fn stamp(paths: &[PathBuf]) -> SystemTime {
    fn walk(p: &Path, best: &mut SystemTime, depth: usize) {
        let Ok(m) = std::fs::metadata(p) else { return };
        if let Ok(t) = m.modified() {
            *best = (*best).max(t);
        }
        if m.is_dir() && depth < 12 {
            let skip = ["target", "node_modules", ".git", ".next", "dist"];
            if p.file_name()
                .map(|n| skip.contains(&n.to_string_lossy().as_ref()))
                .unwrap_or(false)
            {
                return;
            }
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    walk(&e.path(), best, depth + 1);
                }
            }
        }
    }
    let mut best = SystemTime::UNIX_EPOCH;
    for p in paths {
        walk(p, &mut best, 0);
    }
    best
}

fn record(path: &Path, o: &Opts) -> Result<bool> {
    if !o.watch {
        return once(path, o);
    }
    loop {
        if let Err(e) = once(path, o) {
            eprintln!("error: {e:#}");
        }
        let mut watched = vec![path.to_path_buf()];
        if let Ok(s) = Script::load(path) {
            watched.extend(s.settings.watch);
        }
        let base = stamp(&watched);
        eprintln!(
            "watching {} for changes (ctrl-c to stop)",
            watched
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        while stamp(&watched) <= base {
            std::thread::sleep(Duration::from_millis(500));
        }
        eprintln!();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&args) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("error: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

fn run(args: &[String]) -> Result<bool> {
    let Some(cmd) = args.first() else {
        print!("{HELP}");
        return Ok(true);
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "record" | "rec" => {
            let mut o = Opts {
                out: None,
                watch: false,
                headed: false,
                keep: false,
                quiet: false,
            };
            let mut file = None;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--out" | "-o" => o.out = it.next().map(PathBuf::from),
                    "--watch" | "-w" => o.watch = true,
                    "--headed" => o.headed = true,
                    "--keep-frames" => o.keep = true,
                    "--quiet" | "-q" => o.quiet = true,
                    f if !f.starts_with('-') && file.is_none() => file = Some(PathBuf::from(f)),
                    other => bail!("unknown option {other}"),
                }
            }
            let Some(file) = file else {
                bail!("usage: oxdemo record <script>")
            };
            record(&file, &o)
        }
        "check" => {
            let Some(file) = rest.first() else {
                bail!("usage: oxdemo check <script>")
            };
            let s = Script::load(Path::new(file))?;
            println!("{}: {} steps, ok", file, s.steps.len());
            Ok(true)
        }
        "init" => {
            let file = rest.first().map(String::as_str).unwrap_or("demo.oxd");
            if Path::new(file).exists() {
                bail!("{file} already exists");
            }
            std::fs::write(file, STARTER)?;
            println!("wrote {file}. Edit the url and steps, then: oxdemo record {file}");
            Ok(true)
        }
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            Ok(true)
        }
        "-V" | "--version" => {
            println!("oxdemo {}", env!("CARGO_PKG_VERSION"));
            Ok(true)
        }
        other => bail!("unknown command {other}\n\n{HELP}"),
    }
}
