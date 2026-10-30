//! Key bindings. Every shortcut in xod is decided in `bind`, so changing a key
//! means editing one line here.

use iced::keyboard::{key::Named, Key};
use iced::widget::text_editor::{Binding, KeyPress, Motion, Status};

use crate::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    View,
    Insert,
}

/// Everything `bind` needs to know about the app, copied into the key handler.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    pub mode: Mode,
    /// Is the physical left / right Ctrl key currently held?
    pub lctrl: bool,
    pub rctrl: bool,
    /// A prompt (go to line, find, save as) is open and swallows typing.
    pub prompt: bool,
    /// The file tree has keyboard focus.
    pub tree_focus: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CtrlSide {
    Left,
    Right,
}

fn custom(m: Message) -> Option<Binding<Message>> {
    Some(Binding::Custom(m))
}

fn motion(shift: bool, m: Motion) -> Option<Binding<Message>> {
    Some(if shift { Binding::Select(m) } else { Binding::Move(m) })
}

pub fn bind(ctx: Ctx, kp: KeyPress) -> Option<Binding<Message>> {
    if !matches!(kp.status, Status::Focused { .. }) {
        return None;
    }

    let ctrl = kp.modifiers.control();
    let shift = kp.modifiers.shift();
    let alt = kp.modifiers.alt();
    let named = match kp.key.as_ref() {
        Key::Named(n) => Some(n),
        _ => None,
    };
    let letter = match kp.key.as_ref() {
        Key::Character(c) => c.chars().next().map(|c| c.to_ascii_lowercase()),
        _ => None,
    };

    // Which Ctrl is down. If the OS never told us (no side info), act as left Ctrl.
    let side = if !ctrl {
        None
    } else if ctx.rctrl && !ctx.lctrl {
        Some(CtrlSide::Right)
    } else {
        Some(CtrlSide::Left)
    };

    // 1. A prompt is open: it owns the keyboard.
    if ctx.prompt {
        return match named {
            Some(Named::Escape) => custom(Message::PromptCancel),
            Some(Named::Enter) if ctrl => custom(Message::PromptReplaceAll),
            Some(Named::Enter) => custom(Message::PromptSubmit(shift)),
            Some(Named::Backspace) => custom(Message::PromptBackspace),
            Some(Named::Tab) => custom(Message::PromptTab),
            _ if ctrl || alt => None,
            _ => kp
                .text
                .as_ref()
                .and_then(|t| t.chars().find(|c| !c.is_control()))
                .and_then(|c| custom(Message::PromptChar(c))),
        };
    }

    // 2. Ctrl + letter shortcuts that work everywhere.
    if ctrl {
        match letter {
            Some('s') => return custom(if shift { Message::SaveQuit } else { Message::Save }),
            Some('q') => return custom(Message::Quit),
            Some('f') => return custom(Message::OpenFind { replace: shift }),
            Some('b') => return custom(Message::ToggleTree),
            Some('t') => return custom(if shift { Message::PrevTheme } else { Message::NextTheme }),
            Some('z') => return custom(if shift { Message::Redo } else { Message::Undo }),
            Some('y') => return custom(Message::Redo),
            // left Ctrl + H = go to line
            Some('h') => {
                return if side == Some(CtrlSide::Left) { custom(Message::OpenGoto) } else { None };
            }
            _ => {}
        }
        // Ctrl + Space = jump between start and end of file
        if matches!(named, Some(Named::Space)) {
            return custom(Message::ToggleFileEnds);
        }
        // left Ctrl + Backspace = delete line (view) / delete word (insert)
        if matches!(named, Some(Named::Backspace)) {
            return match (side, ctx.mode) {
                (Some(CtrlSide::Left), Mode::View) => custom(Message::DeleteLine),
                (Some(CtrlSide::Left), Mode::Insert) => Some(Binding::Sequence(vec![
                    Binding::Select(Motion::WordLeft),
                    Binding::Backspace,
                ])),
                _ => None,
            };
        }
    }

    // 3. File tree has focus: it owns the navigation keys.
    if ctx.tree_focus {
        return match named {
            Some(Named::ArrowUp) => custom(Message::TreeUp),
            Some(Named::ArrowDown) => custom(Message::TreeDown),
            Some(Named::Enter) | Some(Named::ArrowRight) => custom(Message::TreeEnter),
            Some(Named::Backspace) | Some(Named::ArrowLeft) => custom(Message::TreeParent),
            Some(Named::Escape) => custom(Message::TreeClose),
            _ => None,
        };
    }

    // 4. Escape: insert -> view. (Never let it reach the widget: it would unfocus.)
    if matches!(named, Some(Named::Escape)) {
        return if ctx.mode == Mode::Insert { custom(Message::SetMode(Mode::View)) } else { None };
    }

    // 5. Ctrl + arrows, with the side deciding the meaning.
    if let Some(side) = side {
        let m = match (side, named) {
            (CtrlSide::Left, Some(Named::ArrowLeft)) => Some(Motion::WordLeft),
            (CtrlSide::Left, Some(Named::ArrowRight)) => Some(Motion::WordRight),
            (CtrlSide::Right, Some(Named::ArrowLeft)) => Some(Motion::Home),
            (CtrlSide::Right, Some(Named::ArrowRight)) => Some(Motion::End),
            (CtrlSide::Right, Some(Named::ArrowUp)) => Some(Motion::PageUp),
            (CtrlSide::Right, Some(Named::ArrowDown)) => Some(Motion::PageDown),
            _ => None,
        };
        if let Some(m) = m {
            return motion(shift, m);
        }
    }

    // 6. Mode-specific keys.
    match ctx.mode {
        Mode::View => {
            if !ctrl && !alt {
                match letter {
                    Some('i') => return custom(Message::SetMode(Mode::Insert)),
                    Some('u') => return custom(Message::Undo),
                    Some('r') => return custom(Message::Redo),
                    _ => {}
                }
            }
            // View mode never types: only movement, selection and clipboard get through.
            match Binding::from_key_press(kp) {
                Some(
                    b @ (Binding::Move(_)
                    | Binding::Select(_)
                    | Binding::SelectAll
                    | Binding::SelectWord
                    | Binding::SelectLine
                    | Binding::Copy
                    | Binding::Cut
                    | Binding::Paste),
                ) => Some(b),
                _ => None,
            }
        }
        Mode::Insert => {
            if matches!(named, Some(Named::Tab)) {
                return Some(Binding::Sequence(vec![Binding::Insert(' '); 4]));
            }
            Binding::from_key_press(kp)
        }
    }
}
