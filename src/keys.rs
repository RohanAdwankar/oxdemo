//! Key names to the fields `Input.dispatchKeyEvent` wants.

use anyhow::{bail, Result};

pub struct Key {
    pub key: String,
    pub code: String,
    pub key_code: u32,
    pub text: Option<String>,
    pub modifiers: u32,
    /// What the on-screen badge says, e.g. "⌘ K". Only shortcuts get one.
    pub badge: Option<String>,
}

const ALT: u32 = 1;
const CTRL: u32 = 2;
const META: u32 = 4;
const SHIFT: u32 = 8;

fn named(name: &str) -> Option<(&'static str, &'static str, u32, Option<&'static str>)> {
    Some(match name.to_lowercase().as_str() {
        "enter" | "return" => ("Enter", "Enter", 13, Some("\r")),
        "tab" => ("Tab", "Tab", 9, None),
        "escape" | "esc" => ("Escape", "Escape", 27, None),
        "backspace" => ("Backspace", "Backspace", 8, None),
        "delete" | "del" => ("Delete", "Delete", 46, None),
        "space" => (" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => ("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => ("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => ("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => ("ArrowRight", "ArrowRight", 39, None),
        "home" => ("Home", "Home", 36, None),
        "end" => ("End", "End", 35, None),
        "pageup" => ("PageUp", "PageUp", 33, None),
        "pagedown" => ("PageDown", "PageDown", 34, None),
        _ => return None,
    })
}

fn symbol(name: &str) -> String {
    match name {
        "Enter" => "↵".into(),
        "Escape" => "Esc".into(),
        "Backspace" => "⌫".into(),
        "ArrowUp" => "↑".into(),
        "ArrowDown" => "↓".into(),
        "ArrowLeft" => "←".into(),
        "ArrowRight" => "→".into(),
        " " => "Space".into(),
        other => other.to_uppercase(),
    }
}

/// What the on-screen key display shows for this key: "⌘ K", "Esc", "↵", "x".
pub fn label(k: &Key) -> String {
    match &k.badge {
        Some(b) => b.clone(),
        None if k.key.chars().count() == 1 && k.key != " " => k.key.clone(),
        None => symbol(&k.key),
    }
}

pub fn parse(spec: &str) -> Result<Key> {
    let parts: Vec<&str> = spec.split('+').collect();
    let (last, mods) = parts.split_last().unwrap();
    let mut modifiers = 0;
    let mut badge = vec![];
    for m in mods {
        let (bit, label) = match m.to_lowercase().as_str() {
            "alt" | "option" | "opt" => (ALT, "⌥"),
            "ctrl" | "control" => (CTRL, "Ctrl"),
            "meta" | "cmd" | "command" | "super" => (META, "⌘"),
            "shift" => (SHIFT, "⇧"),
            other => bail!("unknown modifier {other:?} in {spec:?} (Alt, Ctrl, Meta, Shift)"),
        };
        modifiers |= bit;
        badge.push(label.to_string());
    }
    let (key, code, key_code, text) = if let Some((k, c, n, t)) = named(last) {
        (k.to_string(), c.to_string(), n, t.map(str::to_string))
    } else if last.chars().count() == 1 {
        let c = last.chars().next().unwrap();
        let upper = c.to_ascii_uppercase();
        let code = if c.is_ascii_alphabetic() {
            format!("Key{upper}")
        } else if c.is_ascii_digit() {
            format!("Digit{c}")
        } else {
            String::new()
        };
        let shifted = modifiers & SHIFT != 0;
        let key = if shifted {
            upper.to_string()
        } else {
            c.to_ascii_lowercase().to_string()
        };
        (key.clone(), code, upper as u32, Some(key))
    } else if let Some(n) = last
        .strip_prefix(['F', 'f'])
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| (1..=12).contains(n))
    {
        (format!("F{n}"), format!("F{n}"), 111 + n, None)
    } else {
        bail!("unknown key {last:?}");
    };
    // A shortcut types nothing.
    let text = if modifiers & (CTRL | META | ALT) != 0 {
        None
    } else {
        text
    };
    let badge = if modifiers & (CTRL | META | ALT) != 0 {
        badge.push(symbol(&key));
        Some(badge.join(" "))
    } else {
        None
    };
    Ok(Key {
        key,
        code,
        key_code,
        text,
        modifiers,
        badge,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_get_badges() {
        let k = parse("Meta+K").unwrap();
        assert_eq!(k.modifiers, META);
        assert_eq!(k.code, "KeyK");
        assert_eq!(k.badge.as_deref(), Some("⌘ K"));
        assert!(k.text.is_none());
        let k = parse("Enter").unwrap();
        assert_eq!(k.text.as_deref(), Some("\r"));
        assert!(k.badge.is_none());
        assert!(parse("Hyper+Q").is_err());
        assert_eq!(parse("F5").unwrap().key_code, 116);
    }
}
