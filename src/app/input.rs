//! One-line text input with a cursor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Default)]
pub struct Input {
    pub text: String,
    /// Cursor position in chars.
    pub cursor: usize,
}

impl Input {
    pub fn new(text: &str) -> Self {
        Input { text: text.to_string(), cursor: text.chars().count() }
    }

    fn byte(&self, char_idx: usize) -> usize {
        self.text.char_indices().nth(char_idx).map(|(b, _)| b).unwrap_or(self.text.len())
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Handle an editing key. Returns true if the key was used.
    pub fn key(&mut self, e: &KeyEvent) -> bool {
        let ctrl = e.modifiers.contains(KeyModifiers::CONTROL);
        let alt = e.modifiers.contains(KeyModifiers::ALT);
        match e.code {
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.len(),
            KeyCode::Char('u') if ctrl => {
                let b = self.byte(self.cursor);
                self.text.replace_range(..b, "");
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => {
                let b = self.byte(self.cursor);
                self.text.truncate(b);
            }
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Backspace if ctrl || alt => self.delete_word(),
            KeyCode::Char(c) if !ctrl && !alt => {
                let b = self.byte(self.cursor);
                self.text.insert(b, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    let b = self.byte(self.cursor);
                    self.text.remove(b);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.len() {
                    let b = self.byte(self.cursor);
                    self.text.remove(b);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            _ => return false,
        }
        true
    }

    fn delete_word(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let (a, b) = (self.byte(i), self.byte(self.cursor));
        self.text.replace_range(a..b, "");
        self.cursor = i;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn edits_multibyte_text() {
        let mut i = Input::new("café");
        i.key(&k(KeyCode::Left, KeyModifiers::NONE));
        i.key(&k(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(i.text, "caé");
        i.key(&k(KeyCode::Char('ß'), KeyModifiers::NONE));
        assert_eq!((i.text.as_str(), i.cursor), ("caßé", 3));
        i.key(&k(KeyCode::End, KeyModifiers::NONE));
        i.key(&k(KeyCode::Char(' '), KeyModifiers::NONE));
        i.key(&k(KeyCode::Char('x'), KeyModifiers::NONE));
        i.key(&k(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(i.text, "caßé ");
    }
}
