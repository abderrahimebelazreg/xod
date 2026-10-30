//! xod - a small modal GUI text editor.
//!
//! * `src/keys.rs`  - every keyboard shortcut
//! * `src/logic.rs` - undo history, search, theme files, file listing (unit-tested)
//! * `src/main.rs`  - the application: state, update, view

mod keys;
mod logic;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::keyboard::{self, key::Named, Key};
use iced::widget::text_editor::{Action, Content, Edit, Motion};
use iced::widget::{
    button, container, focus_next, horizontal_space, scrollable, text, text_editor, Column, Row,
};
use iced::{Background, Border, Color, Element, Font, Length, Shadow, Subscription, Task, Theme};

use keys::{Ctx, Mode};
use logic::{History, Snapshot, ThemeSpec, TreeEntry};

fn main() -> iced::Result {
    iced::application(Xod::title, Xod::update, Xod::view)
        .subscription(Xod::subscription)
        .theme(Xod::theme)
        .default_font(Font::MONOSPACE)
        .window_size(iced::Size::new(1100.0, 720.0))
        .run_with(Xod::new)
}

// ------------------------------------------------------------------ messages

#[derive(Debug, Clone, Copy)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A raw editor action (typing, cursor movement, mouse).
    Edit(Action),
    Ctrl(Side, bool),
    CtrlCleared,

    SetMode(Mode),
    Undo,
    Redo,
    DeleteLine,
    ToggleFileEnds,

    Save,
    SaveQuit,
    Quit,

    OpenGoto,
    OpenFind { replace: bool },
    PromptChar(char),
    PromptBackspace,
    PromptTab,
    PromptSubmit(bool),
    PromptReplaceAll,
    PromptCancel,

    ToggleTree,
    TreeUp,
    TreeDown,
    TreeEnter,
    TreeParent,
    TreeClose,
    TreeClick(usize),

    NextTheme,
    PrevTheme,
}

// -------------------------------------------------------------------- state

#[derive(Debug, Clone, PartialEq)]
enum PromptKind {
    Goto,
    Find,
    SaveAs { quit: bool },
}

#[derive(Debug, Clone)]
struct Prompt {
    kind: PromptKind,
    text: String,
    replace: String,
    replace_mode: bool,
    /// Find prompt only: typing goes to the replace field.
    on_replace: bool,
}

impl Prompt {
    fn new(kind: PromptKind) -> Self {
        Prompt {
            kind,
            text: String::new(),
            replace: String::new(),
            replace_mode: false,
            on_replace: false,
        }
    }
}

struct Tree {
    open: bool,
    focus: bool,
    dir: PathBuf,
    entries: Vec<TreeEntry>,
    sel: usize,
}

struct Xod {
    content: Content,
    path: Option<PathBuf>,
    mode: Mode,
    dirty: bool,
    history: History,
    /// The last edit was an alphanumeric keystroke (so the next one joins the same undo step).
    typing: bool,
    lctrl: bool,
    rctrl: bool,
    prompt: Option<Prompt>,
    notice: String,
    themes: Vec<ThemeSpec>,
    theme_idx: usize,
    tree: Tree,
}

fn color(c: [u8; 3]) -> Color {
    Color::from_rgb8(c[0], c[1], c[2])
}

fn side_of(l: keyboard::Location) -> Option<Side> {
    match l {
        keyboard::Location::Left => Some(Side::Left),
        keyboard::Location::Right => Some(Side::Right),
        _ => None,
    }
}

/// Watches the Ctrl keys so that left Ctrl and right Ctrl can mean different things.
fn on_event(
    event: iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(Named::Control),
            location,
            ..
        }) => side_of(location).map(|s| Message::Ctrl(s, true)),
        iced::Event::Keyboard(keyboard::Event::KeyReleased {
            key: Key::Named(Named::Control),
            location,
            ..
        }) => side_of(location).map(|s| Message::Ctrl(s, false)),
        iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) if !m.control() => {
            Some(Message::CtrlCleared)
        }
        _ => None,
    }
}

impl Xod {
    fn new() -> (Self, Task<Message>) {
        let arg = std::env::args().nth(1).map(PathBuf::from);
        let (content, path, notice) = match &arg {
            Some(p) => match std::fs::read_to_string(p) {
                Ok(s) => (Content::with_text(&s), Some(p.clone()), String::new()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    (Content::new(), Some(p.clone()), "New file".to_string())
                }
                Err(e) => (
                    Content::new(),
                    None,
                    format!("Cannot open {}: {e}", p.display()),
                ),
            },
            None => (Content::new(), None, String::new()),
        };

        let start_dir = path
            .as_ref()
            .and_then(|p| p.parent())
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let start_dir = std::fs::canonicalize(&start_dir).unwrap_or(start_dir);

        let themes = logic::load_themes();
        let theme_idx = logic::saved_theme_name()
            .and_then(|n| themes.iter().position(|t| t.name == n))
            .unwrap_or(0);

        let app = Xod {
            content,
            path,
            mode: Mode::View,
            dirty: false,
            history: History::default(),
            typing: false,
            lctrl: false,
            rctrl: false,
            prompt: None,
            notice,
            themes,
            theme_idx,
            tree: Tree {
                open: false,
                focus: false,
                dir: start_dir,
                entries: Vec::new(),
                sel: 0,
            },
        };
        // Give the editor keyboard focus right away (click it once if keys do nothing).
        (app, focus_next())
    }

    fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[No Name]".into());
        format!("{}{} - xod", name, if self.dirty { " +" } else { "" })
    }

    fn spec(&self) -> &ThemeSpec {
        &self.themes[self.theme_idx]
    }

    fn theme(&self) -> Theme {
        let s = self.spec();
        Theme::custom(
            s.name.clone(),
            iced::theme::Palette {
                background: color(s.background),
                text: color(s.foreground),
                primary: color(s.accent),
                success: color(s.accent),
                danger: Color::from_rgb8(0xe0, 0x5a, 0x5a),
            },
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::event::listen_with(on_event)
    }

    // ---------------------------------------------------------------- update

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Edit(action) => {
                self.notice.clear();
                self.apply_action(action);
            }
            Message::Ctrl(side, down) => match side {
                Side::Left => self.lctrl = down,
                Side::Right => self.rctrl = down,
            },
            Message::CtrlCleared => {
                self.lctrl = false;
                self.rctrl = false;
            }

            Message::SetMode(mode) => {
                self.mode = mode;
                self.typing = false;
            }
            Message::Undo => self.undo(),
            Message::Redo => self.redo(),
            Message::DeleteLine => self.delete_line(),
            Message::ToggleFileEnds => {
                self.typing = false;
                let at_start = self.content.cursor_position() == (0, 0);
                let target = if at_start { Motion::DocumentEnd } else { Motion::DocumentStart };
                self.content.perform(Action::Move(target));
            }

            Message::Save => return self.save(false),
            Message::SaveQuit => return self.save(true),
            Message::Quit => return iced::exit(),

            Message::OpenGoto => self.prompt = Some(Prompt::new(PromptKind::Goto)),
            Message::OpenFind { replace } => {
                let mut p = Prompt::new(PromptKind::Find);
                p.replace_mode = replace;
                // Start from the selected text, if any.
                if let Some(sel) = self.content.selection() {
                    if !sel.contains('\n') {
                        p.text = sel;
                    }
                }
                self.prompt = Some(p);
            }
            Message::PromptChar(c) => {
                if let Some(p) = &mut self.prompt {
                    if p.kind == PromptKind::Goto && !c.is_ascii_digit() {
                        return Task::none();
                    }
                    if p.on_replace {
                        p.replace.push(c);
                    } else {
                        p.text.push(c);
                    }
                }
            }
            Message::PromptBackspace => {
                if let Some(p) = &mut self.prompt {
                    if p.on_replace {
                        p.replace.pop();
                    } else {
                        p.text.pop();
                    }
                }
            }
            Message::PromptTab => {
                if let Some(p) = &mut self.prompt {
                    if p.replace_mode {
                        p.on_replace = !p.on_replace;
                    }
                }
            }
            Message::PromptCancel => self.prompt = None,
            Message::PromptSubmit(backwards) => return self.prompt_submit(backwards),
            Message::PromptReplaceAll => self.replace_all(),

            Message::ToggleTree => {
                if self.tree.open {
                    self.tree.open = false;
                    self.tree.focus = false;
                } else {
                    self.tree.open = true;
                    self.tree.focus = true;
                    self.tree_refresh();
                }
            }
            Message::TreeUp => self.tree.sel = self.tree.sel.saturating_sub(1),
            Message::TreeDown => {
                let last = self.tree.entries.len().saturating_sub(1);
                self.tree.sel = (self.tree.sel + 1).min(last);
            }
            Message::TreeEnter => self.tree_activate(self.tree.sel),
            Message::TreeClick(i) => {
                self.tree.sel = i;
                self.tree_activate(i);
            }
            Message::TreeParent => {
                if let Some(parent) = self.tree.dir.parent().map(Path::to_path_buf) {
                    self.tree.dir = parent;
                    self.tree_refresh();
                }
            }
            Message::TreeClose => {
                self.tree.open = false;
                self.tree.focus = false;
            }

            Message::NextTheme => self.cycle_theme(1),
            Message::PrevTheme => self.cycle_theme(-1),
        }
        Task::none()
    }

    fn apply_action(&mut self, action: Action) {
        match &action {
            Action::Edit(edit) => {
                let typing_char = matches!(edit, Edit::Insert(c) if c.is_alphanumeric());
                if !(typing_char && self.typing) {
                    self.push_undo();
                }
                self.typing = typing_char;
                self.dirty = true;
            }
            Action::Click(_) => {
                self.typing = false;
                self.tree.focus = false;
            }
            _ => self.typing = false,
        }
        self.content.perform(action);
    }

    // --------------------------------------------------------- undo and redo

    fn snapshot(&self) -> Snapshot {
        let text = self.content.text();
        let (line, byte_col) = self.content.cursor_position();
        let col = logic::byte_col_to_char_col(logic::line_text(&text, line), byte_col);
        Snapshot { text, line, col }
    }

    fn push_undo(&mut self) {
        let snap = self.snapshot();
        self.history.record(snap);
    }

    fn restore(&mut self, snap: &Snapshot) {
        self.content = Content::with_text(&snap.text);
        self.goto(snap.line, snap.col);
    }

    fn undo(&mut self) {
        let current = self.snapshot();
        match self.history.undo(current) {
            Some(prev) => {
                self.restore(&prev);
                self.dirty = true;
                self.typing = false;
            }
            None => self.notice = "Nothing to undo".into(),
        }
    }

    fn redo(&mut self) {
        let current = self.snapshot();
        match self.history.redo(current) {
            Some(next) => {
                self.restore(&next);
                self.dirty = true;
                self.typing = false;
            }
            None => self.notice = "Nothing to redo".into(),
        }
    }

    // -------------------------------------------------------- cursor helpers

    /// Put the cursor at (line, char column), both zero-based.
    fn goto(&mut self, line: usize, col: usize) {
        let text = self.content.text();
        let line = line.min(logic::line_count(&text).saturating_sub(1));
        let col = col.min(logic::line_len_chars(&text, line));

        self.content.perform(Action::Move(Motion::DocumentStart));
        // Moving down may cross several visual rows of a wrapped line, so check the
        // real line number each step. The guard keeps this loop finite.
        let mut guard = text.len() + line + 16;
        while self.content.cursor_position().0 < line && guard > 0 {
            self.content.perform(Action::Move(Motion::Down));
            guard -= 1;
        }
        self.content.perform(Action::Move(Motion::Home));
        for _ in 0..col {
            self.content.perform(Action::Move(Motion::Right));
        }
    }

    fn delete_line(&mut self) {
        self.push_undo();
        self.typing = false;
        self.dirty = true;

        let text = self.content.text();
        let (line, _) = self.content.cursor_position();
        let total = logic::line_count(&text);
        let len = logic::line_len_chars(&text, line);

        self.goto(line, 0);
        if len > 0 {
            for _ in 0..len {
                self.content.perform(Action::Select(Motion::Right));
            }
            self.content.perform(Action::Edit(Edit::Backspace)); // removes the selection
        }
        if line + 1 < total {
            self.content.perform(Action::Edit(Edit::Delete)); // joins the next line up
        } else if line > 0 {
            self.content.perform(Action::Edit(Edit::Backspace)); // last line: join with previous
        }
    }

    // ------------------------------------------------------------ save / quit

    fn write_to(&self, path: &Path) -> std::io::Result<()> {
        let mut text = self.content.text();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        std::fs::write(path, text)
    }

    fn save(&mut self, quit: bool) -> Task<Message> {
        let Some(path) = self.path.clone() else {
            self.prompt = Some(Prompt::new(PromptKind::SaveAs { quit }));
            return Task::none();
        };
        match self.write_to(&path) {
            Ok(()) => {
                self.dirty = false;
                self.notice = format!("Saved {}", path.display());
                if quit {
                    return iced::exit();
                }
            }
            Err(e) => self.notice = format!("Save failed: {e}"),
        }
        Task::none()
    }

    // ---------------------------------------------------------------- prompts

    fn prompt_submit(&mut self, backwards: bool) -> Task<Message> {
        let Some(p) = self.prompt.clone() else {
            return Task::none();
        };
        match p.kind {
            PromptKind::Goto => {
                match p.text.trim().parse::<usize>() {
                    Ok(n) if n > 0 => {
                        self.typing = false;
                        self.goto(n - 1, 0);
                        self.notice = format!("Line {n}");
                    }
                    _ => self.notice = "Type a line number".into(),
                }
                self.prompt = None;
            }
            PromptKind::SaveAs { quit } => {
                self.prompt = None;
                let name = p.text.trim();
                if name.is_empty() {
                    return Task::none();
                }
                self.path = Some(PathBuf::from(name));
                return self.save(quit);
            }
            PromptKind::Find => {
                if p.on_replace {
                    self.replace_current(&p.text, &p.replace);
                } else {
                    self.find(&p.text, backwards);
                }
            }
        }
        Task::none()
    }

    /// Select the next (or previous) match. The prompt stays open.
    fn find(&mut self, query: &str, backwards: bool) -> bool {
        if query.is_empty() {
            return false;
        }
        self.typing = false;
        let text = self.content.text();
        let (line, byte_col) = self.content.cursor_position();
        let col = logic::byte_col_to_char_col(logic::line_text(&text, line), byte_col);
        let at = logic::offset_of(&text, line, col);
        let selected = self.content.selection().map_or(0, |s| s.chars().count());

        let hit = if backwards {
            logic::find_prev(&text, query, at.saturating_sub(selected))
        } else {
            logic::find_next(&text, query, at)
        };
        match hit {
            Some((start, len)) => {
                let (l, c) = logic::line_col_of(&text, start);
                self.goto(l, c);
                for _ in 0..len {
                    self.content.perform(Action::Select(Motion::Right));
                }
                true
            }
            None => {
                self.notice = format!("Not found: {query}");
                false
            }
        }
    }

    fn replace_current(&mut self, find: &str, with: &str) {
        if find.is_empty() {
            return;
        }
        if self.content.selection().as_deref() == Some(find) {
            self.push_undo();
            self.dirty = true;
            self.typing = false;
            let edit = if with.is_empty() {
                Edit::Delete
            } else {
                Edit::Paste(Arc::new(with.to_string()))
            };
            self.content.perform(Action::Edit(edit));
        }
        self.find(find, false);
    }

    fn replace_all(&mut self) {
        let Some(p) = self.prompt.clone() else {
            return;
        };
        if !p.replace_mode || p.text.is_empty() {
            return;
        }
        let snap = self.snapshot();
        let (new_text, count) = logic::replace_all(&snap.text, &p.text, &p.replace);
        if count > 0 {
            self.history.record(snap.clone());
            self.content = Content::with_text(&new_text);
            self.goto(snap.line, snap.col);
            self.dirty = true;
            self.typing = false;
        }
        self.notice = format!("Replaced {count}");
    }

    // -------------------------------------------------------------- file tree

    fn tree_refresh(&mut self) {
        self.tree.entries = logic::list_dir(&self.tree.dir);
        self.tree.sel = self.tree.sel.min(self.tree.entries.len().saturating_sub(1));
    }

    fn tree_activate(&mut self, i: usize) {
        let Some(entry) = self.tree.entries.get(i).cloned() else {
            return;
        };
        if entry.is_dir {
            self.tree.dir = entry.path;
            self.tree.sel = 0;
            self.tree_refresh();
        } else {
            self.open_file(entry.path);
        }
    }

    fn open_file(&mut self, path: PathBuf) {
        if self.dirty {
            self.notice = "Unsaved changes: save with Ctrl+S first (or Ctrl+Q to quit)".into();
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                self.content = Content::with_text(&s);
                self.notice = format!("Opened {}", path.display());
                self.path = Some(path);
                self.history.clear();
                self.mode = Mode::View;
                self.typing = false;
                self.tree.focus = false;
            }
            Err(e) => self.notice = format!("Cannot open {}: {e}", path.display()),
        }
    }

    // ---------------------------------------------------------------- themes

    fn cycle_theme(&mut self, step: isize) {
        // Re-read the theme folder so edits to your theme files show up immediately.
        let current = self.spec().name.clone();
        self.themes = logic::load_themes();
        let idx = self.themes.iter().position(|t| t.name == current).unwrap_or(0);
        let n = self.themes.len() as isize;
        self.theme_idx = (idx as isize + step).rem_euclid(n) as usize;
        let name = self.spec().name.clone();
        logic::save_theme_name(&name);
        self.notice = format!("Theme: {name}");
    }

    // ------------------------------------------------------------------ view

    fn view(&self) -> Element<'_, Message> {
        let s = self.spec();
        let bg = color(s.background);
        let fg = color(s.foreground);
        let sel = color(s.selection);
        let acc = color(s.accent);
        let bar = color(s.bar);
        let dim = color(logic_mix(s.foreground, s.background));

        let ctx = Ctx {
            mode: self.mode,
            lctrl: self.lctrl,
            rctrl: self.rctrl,
            prompt: self.prompt.is_some(),
            tree_focus: self.tree.open && self.tree.focus,
        };

        let editor = text_editor(&self.content)
            .on_action(Message::Edit)
            .key_binding(move |kp| keys::bind(ctx, kp))
            .height(Length::Fill)
            .padding(12.0)
            .size(16.0)
            .style(move |_theme, _status| text_editor::Style {
                background: Background::Color(bg),
                border: Border::default(),
                icon: fg,
                placeholder: dim,
                value: fg,
                selection: sel,
            });

        let body: Element<'_, Message> = if self.tree.open {
            Row::new().push(self.tree_view(fg, dim, sel, bar)).push(editor).into()
        } else {
            editor.into()
        };

        let mut page = Column::new().push(body);
        if let Some(p) = &self.prompt {
            page = page.push(
                container(text(prompt_line(p)).size(14.0).color(fg))
                    .width(Length::Fill)
                    .padding([6.0, 12.0])
                    .style(move |_t| container::Style {
                        background: Some(Background::Color(sel)),
                        ..container::Style::default()
                    }),
            );
        }
        page.push(self.status_bar(fg, dim, bg, acc, bar)).into()
    }

    fn status_bar(&self, fg: Color, dim: Color, bg: Color, acc: Color, bar: Color) -> Element<'_, Message> {
        let (label, badge_color) = match self.mode {
            Mode::View => (" VIEW ", acc),
            Mode::Insert => (" INSERT ", Color::from_rgb8(0x98, 0xc3, 0x79)),
        };
        let badge = container(text(label).size(13.0).color(bg))
            .padding([2.0, 6.0])
            .style(move |_t| container::Style {
                background: Some(Background::Color(badge_color)),
                border: Border { radius: 3.0.into(), ..Border::default() },
                ..container::Style::default()
            });

        let name = self
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "[No Name]".into());
        let (line, col) = self.content.cursor_position();

        let row = Row::new()
            .spacing(12.0)
            .align_y(iced::alignment::Vertical::Center)
            .push(badge)
            .push(text(format!("{}{}", name, if self.dirty { " [+]" } else { "" })).size(13.0).color(fg))
            .push(text(self.notice.clone()).size(13.0).color(dim))
            .push(horizontal_space())
            .push(text(format!("Ln {}, Col {}", line + 1, col + 1)).size(13.0).color(dim))
            .push(text(self.spec().name.clone()).size(13.0).color(dim));

        container(row)
            .width(Length::Fill)
            .padding([4.0, 10.0])
            .style(move |_t| container::Style {
                background: Some(Background::Color(bar)),
                text_color: Some(fg),
                ..container::Style::default()
            })
            .into()
    }

    fn tree_view(&self, fg: Color, dim: Color, sel: Color, bar: Color) -> Element<'_, Message> {
        let t = &self.tree;
        let start = t.sel.saturating_sub(20);
        let end = (start + 40).min(t.entries.len());

        let mut list = Column::new().spacing(1.0);
        for i in start..end {
            let e = &t.entries[i];
            let label = if e.is_dir { format!("▸ {}/", e.name) } else { format!("  {}", e.name) };
            let selected = i == t.sel && t.focus;
            list = list.push(
                button(text(label).size(14.0))
                    .width(Length::Fill)
                    .padding([2.0, 6.0])
                    .on_press(Message::TreeClick(i))
                    .style(move |_theme, status| button::Style {
                        background: if selected || matches!(status, button::Status::Hovered) {
                            Some(Background::Color(sel))
                        } else {
                            None
                        },
                        text_color: fg,
                        border: Border::default(),
                        shadow: Shadow::default(),
                    }),
            );
        }

        container(
            Column::new()
                .spacing(6.0)
                .push(text(t.dir.display().to_string()).size(12.0).color(dim))
                .push(scrollable(list).height(Length::Fill)),
        )
        .width(Length::Fixed(260.0))
        .height(Length::Fill)
        .padding(8.0)
        .style(move |_t| container::Style {
            background: Some(Background::Color(bar)),
            ..container::Style::default()
        })
        .into()
    }
}

fn logic_mix(a: [u8; 3], b: [u8; 3]) -> [u8; 3] {
    [
        ((a[0] as u16 + b[0] as u16) / 2) as u8,
        ((a[1] as u16 + b[1] as u16) / 2) as u8,
        ((a[2] as u16 + b[2] as u16) / 2) as u8,
    ]
}

fn prompt_line(p: &Prompt) -> String {
    match &p.kind {
        PromptKind::Goto => format!("Go to line: {}_", p.text),
        PromptKind::SaveAs { .. } => format!("Save as: {}_", p.text),
        PromptKind::Find if !p.replace_mode => format!(
            "Find: {}_    (Enter next, Shift+Enter previous, Esc close)",
            p.text
        ),
        PromptKind::Find => {
            let (a, b) = if p.on_replace { ("", "_") } else { ("_", "") };
            format!(
                "Find: {}{}  ->  Replace: {}{}    (Tab switch field, Enter find/replace, Ctrl+Enter replace all, Esc close)",
                p.text, a, p.replace, b
            )
        }
    }
}
