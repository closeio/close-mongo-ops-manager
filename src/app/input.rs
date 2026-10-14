//! Single-line text input state.

/// Editable single-line text with a cursor. The cursor is a char index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    value: String,
    cursor: usize,
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Cursor position, in chars from the start.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map_or(self.value.len(), |(i, _)| i)
    }

    /// Replaces the value and moves the cursor to the end.
    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.len();
    }

    /// Empties the input. Returns whether the value changed.
    pub fn clear(&mut self) -> bool {
        let changed = !self.value.is_empty();
        self.value.clear();
        self.cursor = 0;
        changed
    }

    /// Inserts a char at the cursor. Control characters are ignored.
    pub fn insert_char(&mut self, c: char) -> bool {
        if c.is_control() {
            return false;
        }
        let at = self.byte_index(self.cursor);
        self.value.insert(at, c);
        self.cursor += 1;
        true
    }

    /// Inserts text at the cursor (e.g. a paste). Line breaks and tabs become
    /// spaces, other control characters are dropped.
    pub fn insert_str(&mut self, text: &str) -> bool {
        let mut changed = false;
        for c in text.chars() {
            let c = if matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            };
            changed |= self.insert_char(c);
        }
        changed
    }

    /// Deletes the char before the cursor.
    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let at = self.byte_index(self.cursor - 1);
        self.value.remove(at);
        self.cursor -= 1;
        true
    }

    /// Deletes the char under the cursor.
    pub fn delete(&mut self) -> bool {
        if self.cursor >= self.len() {
            return false;
        }
        let at = self.byte_index(self.cursor);
        self.value.remove(at);
        true
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len());
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.len();
    }

    /// Moves the cursor, clamped to the value.
    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.len());
    }

    /// Deletes everything before the cursor.
    pub fn delete_to_start(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let at = self.byte_index(self.cursor);
        self.value.replace_range(..at, "");
        self.cursor = 0;
        true
    }

    /// Deletes everything from the cursor on.
    pub fn delete_to_end(&mut self) -> bool {
        let at = self.byte_index(self.cursor);
        if at >= self.value.len() {
            return false;
        }
        self.value.truncate(at);
        true
    }

    /// Deletes the word before the cursor, and the whitespace after it.
    pub fn delete_word_left(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let mut start = self.cursor;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let (from, to) = (self.byte_index(start), self.byte_index(self.cursor));
        self.value.replace_range(from..to, "");
        self.cursor = start;
        true
    }

    /// First visible char when the input is `width` cells wide, keeping the
    /// cursor visible.
    pub fn scroll_offset(&self, width: usize) -> usize {
        if width == 0 {
            return self.cursor;
        }
        let before = &self.value[..self.byte_index(self.cursor)];
        let mut offset = self.cursor;
        let mut index = self.cursor;
        for (at, c) in before.char_indices().rev() {
            if text_width(&before[at..]) >= width {
                break;
            }
            index -= 1;
            if !joins_previous(c) {
                offset = index;
            }
        }
        offset
    }

    /// The value from char `offset` on: what the input scrolled to `offset`
    /// shows.
    pub fn visible(&self, offset: usize) -> &str {
        &self.value[self.byte_index(offset)..]
    }

    /// Cells from the start of the view scrolled to `offset` to the cursor.
    pub fn cursor_column(&self, offset: usize) -> usize {
        let start = self.byte_index(offset.min(self.cursor));
        text_width(&self.value[start..self.byte_index(self.cursor)])
    }

    /// Cursor position for a click `column` cells from the start of the view
    /// scrolled to `offset`.
    pub fn cursor_at_column(&self, offset: usize, column: usize) -> usize {
        let visible = self.visible(offset);
        let mut cursor = offset.min(self.len());
        let mut index = cursor;
        let mut chars = visible.char_indices().peekable();
        while let Some((at, c)) = chars.next() {
            if text_width(&visible[..at + c.len_utf8()]) > column {
                break;
            }
            index += 1;
            if chars.peek().is_none_or(|&(_, next)| !joins_previous(next)) {
                cursor = index;
            }
        }
        cursor
    }
}

/// Display width of `s`, in terminal cells, as ratatui draws it. Text is
/// measured as a whole, not char by char: emoji sequences and variation
/// selectors change the width of what precedes them.
fn text_width(s: &str) -> usize {
    ratatui::text::Span::raw(s).width()
}

/// Whether `c` is drawn with the char before it (variation selector, joiner,
/// combining mark): the view does not start at one, and a click does not put
/// the cursor before one.
fn joins_previous(c: char) -> bool {
    text_width(c.encode_utf8(&mut [0; 4])) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(value: &str) -> TextInput {
        let mut i = TextInput::new();
        i.set_value(value);
        i
    }

    #[test]
    fn insert_and_delete() {
        let mut i = TextInput::new();
        assert!(i.insert_char('a'));
        assert!(i.insert_char('c'));
        i.move_left();
        assert!(i.insert_char('b'));
        assert_eq!(i.value(), "abc");
        assert_eq!(i.cursor(), 2);
        assert!(i.backspace());
        assert_eq!(i.value(), "ac");
        assert!(i.delete());
        assert_eq!(i.value(), "a");
        assert!(!i.delete());
        assert!(!i.insert_char('\u{7}'));
    }

    #[test]
    fn multibyte_chars() {
        let mut i = input("añb");
        i.move_left();
        assert!(i.backspace());
        assert_eq!(i.value(), "ab");
        i.insert_char('ü');
        assert_eq!(i.value(), "aüb");
        assert_eq!(i.cursor(), 2);
    }

    #[test]
    fn paste_flattens_line_breaks() {
        let mut i = TextInput::new();
        assert!(i.insert_str("a\nb\tc"));
        assert_eq!(i.value(), "a b c");
    }

    #[test]
    fn movement_is_clamped() {
        let mut i = input("ab");
        i.move_right();
        assert_eq!(i.cursor(), 2);
        i.move_home();
        i.move_left();
        assert_eq!(i.cursor(), 0);
        i.set_cursor(10);
        assert_eq!(i.cursor(), 2);
    }

    #[test]
    fn line_editing() {
        let mut i = input("hello big world");
        assert!(i.delete_word_left());
        assert_eq!(i.value(), "hello big ");
        assert!(i.delete_word_left());
        assert_eq!(i.value(), "hello ");
        i.set_cursor(2);
        assert!(i.delete_to_end());
        assert_eq!(i.value(), "he");
        assert!(i.delete_to_start());
        assert_eq!(i.value(), "");
        assert!(!i.delete_to_start());
    }

    #[test]
    fn scroll_keeps_cursor_visible() {
        let i = input("0123456789");
        assert_eq!(i.scroll_offset(20), 0);
        assert_eq!(i.scroll_offset(5), 6);
        let mut j = i.clone();
        j.move_home();
        assert_eq!(j.scroll_offset(5), 0);

        let wide = input("日本語");
        assert_eq!(wide.scroll_offset(5), 1);
        assert_eq!(wide.visible(1), "本語");
        assert_eq!(wide.cursor_column(1), 4);
        assert_eq!(wide.cursor_at_column(0, 2), 1);
        assert_eq!(wide.cursor_at_column(1, 2), 2);

        // U+FE0F makes the heart 2 cells wide, though it has no width of its
        // own.
        let emoji = input("\u{2764}\u{fe0f}x");
        assert_eq!(emoji.cursor_column(0), 3);
        assert_eq!(emoji.scroll_offset(4), 0);
        assert_eq!(emoji.scroll_offset(3), 2);
        assert_eq!(emoji.cursor_at_column(0, 1), 0);
        assert_eq!(emoji.cursor_at_column(0, 2), 2);
    }
}
