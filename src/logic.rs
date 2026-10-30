//! Pure logic for xod: undo history, text/cursor maths, search, themes, file listing.
//!
//! Nothing in here depends on the GUI crate, so it can be unit-tested on its own:
//! `rustc --edition 2021 --test src/logic.rs -o /tmp/logic_tests && /tmp/logic_tests`

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- undo / redo

pub const MAX_HISTORY: usize = 300;

/// Whole-buffer snapshot. `col` is counted in characters, not bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub text: String,
    pub line: usize,
    pub col: usize,
}

#[derive(Default, Debug)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl History {
    /// Remember the state *before* an edit.
    pub fn record(&mut self, s: Snapshot) {
        self.undo.push(s);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

// ------------------------------------------------------------ text / cursor

pub fn line_count(text: &str) -> usize {
    text.split('\n').count()
}

pub fn line_text(text: &str, line: usize) -> &str {
    text.split('\n').nth(line).unwrap_or("")
}

pub fn line_len_chars(text: &str, line: usize) -> usize {
    line_text(text, line).chars().count()
}

/// Convert a byte column inside `line` to a character column (never panics).
pub fn byte_col_to_char_col(line: &str, byte_col: usize) -> usize {
    let mut b = byte_col.min(line.len());
    while !line.is_char_boundary(b) {
        b -= 1;
    }
    line[..b].chars().count()
}

/// (line, char column) -> absolute character offset. Out-of-range values clamp.
pub fn offset_of(text: &str, line: usize, col: usize) -> usize {
    let mut off = 0;
    for (i, l) in text.split('\n').enumerate() {
        let n = l.chars().count();
        if i == line {
            return off + col.min(n);
        }
        off += n + 1;
    }
    off.saturating_sub(1)
}

/// Absolute character offset -> (line, char column).
pub fn line_col_of(text: &str, offset: usize) -> (usize, usize) {
    let mut rem = offset;
    for (i, l) in text.split('\n').enumerate() {
        let n = l.chars().count();
        if rem <= n {
            return (i, rem);
        }
        rem -= n + 1;
    }
    let last = line_count(text) - 1;
    (last, line_len_chars(text, last))
}

// ------------------------------------------------------------------- search

/// First match starting at or after `from` (character offset), wrapping around.
/// Returns (start, length) in characters.
pub fn find_next(text: &str, query: &str, from: usize) -> Option<(usize, usize)> {
    let t: Vec<char> = text.chars().collect();
    let q: Vec<char> = query.chars().collect();
    if q.is_empty() || q.len() > t.len() {
        return None;
    }
    let last = t.len() - q.len();
    let hit = |s: usize| t[s..s + q.len()] == q[..];
    let from = from.min(last + 1);
    (from..=last)
        .find(|&s| hit(s))
        .or_else(|| (0..from).find(|&s| hit(s)))
        .map(|s| (s, q.len()))
}

/// Last match starting strictly before `before`, wrapping around.
pub fn find_prev(text: &str, query: &str, before: usize) -> Option<(usize, usize)> {
    let t: Vec<char> = text.chars().collect();
    let q: Vec<char> = query.chars().collect();
    if q.is_empty() || q.len() > t.len() {
        return None;
    }
    let last = t.len() - q.len();
    let hit = |s: usize| t[s..s + q.len()] == q[..];
    let top = before.min(last + 1);
    (0..top)
        .rev()
        .find(|&s| hit(s))
        .or_else(|| (top..=last).rev().find(|&s| hit(s)))
        .map(|s| (s, q.len()))
}

pub fn replace_all(text: &str, query: &str, with: &str) -> (String, usize) {
    if query.is_empty() {
        return (text.to_string(), 0);
    }
    (text.replace(query, with), text.matches(query).count())
}

// ------------------------------------------------------------------- themes

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeSpec {
    pub name: String,
    pub background: [u8; 3],
    pub foreground: [u8; 3],
    pub selection: [u8; 3],
    pub accent: [u8; 3],
    pub bar: [u8; 3],
}

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let m = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if let Some(rest) = v.strip_prefix('"') {
        rest.split('"').next().unwrap_or("").to_string()
    } else if let Some(rest) = v.strip_prefix('\'') {
        rest.split('\'').next().unwrap_or("").to_string()
    } else {
        v.split_whitespace().next().unwrap_or("").to_string()
    }
}

/// Parse a theme file made of `key = "value"` lines. `background` and
/// `foreground` are required; the rest are derived when missing.
pub fn parse_theme_file(src: &str, fallback_name: &str) -> Option<ThemeSpec> {
    let (mut name, mut bg, mut fg, mut sel, mut acc, mut bar) =
        (None, None, None, None, None, None);
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = unquote(v);
        match k.trim().to_ascii_lowercase().as_str() {
            "name" => name = Some(v),
            "background" | "bg" => bg = parse_hex(&v),
            "foreground" | "fg" | "text" => fg = parse_hex(&v),
            "selection" => sel = parse_hex(&v),
            "accent" => acc = parse_hex(&v),
            "statusbar" | "bar" => bar = parse_hex(&v),
            _ => {}
        }
    }
    let (bg, fg) = (bg?, fg?);
    Some(ThemeSpec {
        name: name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| fallback_name.to_string()),
        background: bg,
        foreground: fg,
        selection: sel.unwrap_or_else(|| mix(bg, fg, 0.25)),
        accent: acc.unwrap_or(fg),
        bar: bar.unwrap_or_else(|| mix(bg, fg, 0.08)),
    })
}

pub fn builtin_themes() -> Vec<ThemeSpec> {
    fn t(name: &str, bg: &str, fg: &str, sel: &str, acc: &str, bar: &str) -> ThemeSpec {
        ThemeSpec {
            name: name.to_string(),
            background: parse_hex(bg).unwrap(),
            foreground: parse_hex(fg).unwrap(),
            selection: parse_hex(sel).unwrap(),
            accent: parse_hex(acc).unwrap(),
            bar: parse_hex(bar).unwrap(),
        }
    }
    vec![
        t("xod-dark", "#1a1b26", "#c0caf5", "#33467c", "#7aa2f7", "#16161e"),
        t("dracula", "#282a36", "#f8f8f2", "#44475a", "#bd93f9", "#21222c"),
        t("gruvbox-dark", "#282828", "#ebdbb2", "#504945", "#fabd2f", "#1d2021"),
        t("nord", "#2e3440", "#d8dee9", "#434c5e", "#88c0d0", "#272c36"),
        t("solarized-light", "#fdf6e3", "#586e75", "#eee8d5", "#268bd2", "#eee8d5"),
        t("paper", "#fafafa", "#2b2b2b", "#cfe3ff", "#d9480f", "#ececec"),
    ]
}

pub fn config_dir() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(x).join("xod"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("xod"))
}

/// Built-in themes plus every `*.toml` in `~/.config/xod/themes/`.
/// A custom theme with the same name as a built-in one replaces it.
pub fn load_themes() -> Vec<ThemeSpec> {
    let mut all = builtin_themes();
    let Some(dir) = config_dir().map(|d| d.join("themes")) else {
        return all;
    };
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .collect();
    files.sort();
    for f in files {
        let stem = f
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(spec) = fs::read_to_string(&f)
            .ok()
            .and_then(|s| parse_theme_file(&s, &stem))
        {
            match all.iter().position(|t| t.name == spec.name) {
                Some(i) => all[i] = spec,
                None => all.push(spec),
            }
        }
    }
    all
}

pub fn saved_theme_name() -> Option<String> {
    fs::read_to_string(config_dir()?.join("theme"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn save_theme_name(name: &str) {
    if let Some(d) = config_dir() {
        let _ = fs::create_dir_all(&d);
        let _ = fs::write(d.join("theme"), name);
    }
}

// ---------------------------------------------------------------- file tree

#[derive(Clone, Debug, PartialEq)]
pub struct TreeEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Directory listing: `..` first, then folders, then files (case-insensitive).
pub fn list_dir(dir: &Path) -> Vec<TreeEntry> {
    let mut out = Vec::new();
    if let Some(parent) = dir.parent() {
        out.push(TreeEntry {
            name: "..".into(),
            path: parent.to_path_buf(),
            is_dir: true,
        });
    }
    let mut items: Vec<TreeEntry> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let path = e.path();
            let is_dir = path.is_dir();
            TreeEntry {
                name: e.file_name().to_string_lossy().into_owned(),
                path,
                is_dir,
            }
        })
        .collect();
    items.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out.extend(items);
    out
}

// -------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(t: &str) -> Snapshot {
        Snapshot { text: t.into(), line: 0, col: 0 }
    }

    #[test]
    fn history_undo_redo_roundtrip() {
        let mut h = History::default();
        h.record(snap("a"));
        h.record(snap("ab"));
        let back = h.undo(snap("abc")).unwrap();
        assert_eq!(back.text, "ab");
        let back = h.undo(snap("ab")).unwrap();
        assert_eq!(back.text, "a");
        assert!(h.undo(snap("a")).is_none());
        let fwd = h.redo(snap("a")).unwrap();
        assert_eq!(fwd.text, "ab");
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut h = History::default();
        h.record(snap("a"));
        h.undo(snap("b"));
        h.record(snap("c"));
        assert!(h.redo(snap("c")).is_none());
    }

    #[test]
    fn history_is_capped() {
        let mut h = History::default();
        for i in 0..(MAX_HISTORY + 50) {
            h.record(snap(&i.to_string()));
        }
        let mut n = 0;
        while h.undo(snap("x")).is_some() {
            n += 1;
        }
        assert_eq!(n, MAX_HISTORY);
    }

    #[test]
    fn byte_to_char_col_handles_multibyte() {
        assert_eq!(byte_col_to_char_col("héllo", 3), 2); // after "hé"
        assert_eq!(byte_col_to_char_col("héllo", 2), 1); // inside é -> snaps back
        assert_eq!(byte_col_to_char_col("abc", 99), 3);
    }

    #[test]
    fn offsets_and_line_cols_agree() {
        let text = "ab\n\ncdé\nf";
        for off in 0..=text.chars().count() {
            let (l, c) = line_col_of(text, off);
            assert_eq!(offset_of(text, l, c), off, "offset {off}");
        }
        assert_eq!(line_col_of(text, 3), (1, 0));
        assert_eq!(offset_of(text, 99, 99), text.chars().count());
        assert_eq!(line_len_chars(text, 2), 3);
        assert_eq!(line_count(text), 4);
    }

    #[test]
    fn find_next_wraps_and_respects_from() {
        let t = "foo bar foo";
        assert_eq!(find_next(t, "foo", 0), Some((0, 3)));
        assert_eq!(find_next(t, "foo", 1), Some((8, 3)));
        assert_eq!(find_next(t, "foo", 9), Some((0, 3))); // wraps
        assert_eq!(find_next(t, "zzz", 0), None);
        assert_eq!(find_next(t, "", 0), None);
    }

    #[test]
    fn find_prev_wraps() {
        let t = "foo bar foo";
        assert_eq!(find_prev(t, "foo", 8), Some((0, 3)));
        assert_eq!(find_prev(t, "foo", 11), Some((8, 3)));
        assert_eq!(find_prev(t, "foo", 0), Some((8, 3))); // wraps to last
        assert_eq!(find_prev(t, "zzz", 5), None);
    }

    #[test]
    fn find_spans_newlines_and_unicode() {
        let t = "é\nx\nyé";
        let (s, n) = find_next(t, "x\ny", 0).unwrap();
        assert_eq!((s, n), (2, 3));
        assert_eq!(line_col_of(t, s), (1, 0));
    }

    #[test]
    fn replace_all_counts() {
        assert_eq!(replace_all("a-a-a", "a", "bb"), ("bb-bb-bb".to_string(), 3));
        assert_eq!(replace_all("abc", "", "x"), ("abc".to_string(), 0));
        assert_eq!(replace_all("abc", "b", ""), ("ac".to_string(), 1));
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("FF8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("#fff"), None);
        assert_eq!(parse_hex("#gg0000"), None);
        assert_eq!(parse_hex("#ééé"), None);
    }

    #[test]
    fn theme_file_full_and_minimal() {
        let full = "# my theme\nname = \"sunset\"\nbackground = \"#101010\" # dark\nforeground = #eeeeee\nselection = \"#303030\"\naccent = \"#ff8800\"\nstatusbar = \"#000000\"\n";
        let t = parse_theme_file(full, "file").unwrap();
        assert_eq!(t.name, "sunset");
        assert_eq!(t.background, [0x10, 0x10, 0x10]);
        assert_eq!(t.foreground, [0xee, 0xee, 0xee]);
        assert_eq!(t.accent, [0xff, 0x88, 0x00]);
        assert_eq!(t.bar, [0, 0, 0]);

        let min = "bg = \"#000000\"\nfg = \"#ffffff\"\n";
        let t = parse_theme_file(min, "mini").unwrap();
        assert_eq!(t.name, "mini");
        assert_eq!(t.accent, [255, 255, 255]);
        assert_eq!(t.selection, [64, 64, 64]);

        assert!(parse_theme_file("name = \"x\"\nbackground = \"#000000\"\n", "x").is_none());
        assert!(parse_theme_file("background = \"nope\"\nforeground = \"#fff\"\n", "x").is_none());
    }

    #[test]
    fn builtin_themes_are_valid_and_unique() {
        let all = builtin_themes();
        assert!(all.len() >= 5);
        let mut names: Vec<_> = all.iter().map(|t| t.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len());
    }

    #[test]
    fn list_dir_orders_dirs_first() {
        let base = std::env::temp_dir().join(format!("xod_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("zdir")).unwrap();
        fs::write(base.join("b.txt"), "").unwrap();
        fs::write(base.join("A.txt"), "").unwrap();
        let names: Vec<String> = list_dir(&base).into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["..", "zdir", "A.txt", "b.txt"]);
        let _ = fs::remove_dir_all(&base);
    }
}
