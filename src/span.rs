//! Source files, spans and positions.
//!
//! Positions are byte offsets into the (already UTF-8 validated) file text.
//! Diagnostic columns are **code-point** based (1-based), which is what §9.2 prints.
//! Display width (CJK = 2 columns) is a separate concern and lives in `fmt`.

use std::collections::HashMap;

pub type FileId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: FileId, start: u32, end: u32) -> Span {
        Span { file, start, end }
    }
    pub fn merge(self, other: Span) -> Span {
        debug_assert_eq!(self.file, other.file);
        Span {
            file: self.file,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
    pub fn empty_at(file: FileId, at: u32) -> Span {
        Span { file, start: at, end: at }
    }
}

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub id: FileId,
    /// Path as printed in diagnostics: slash separated, relative to the module root when the
    /// file lives inside it (§9.2 requires a stable, absolute-path-free rendering — §7.3).
    pub display_path: String,
    pub text: String,
    /// Byte offset of the first character of each line, 0-based index == line number - 1.
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(id: FileId, display_path: String, text: String) -> SourceFile {
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        SourceFile { id, display_path, text, line_starts }
    }

    pub fn line_starts(&self) -> &[u32] {
        &self.line_starts
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// 1-based line, 1-based byte-column.
    pub fn byte_line_col(&self, off: u32) -> (u32, u32) {
        let idx = self.line_starts.partition_point(|&s| s <= off);
        let line = idx.max(1) as u32;
        let start = self.line_starts[(idx - 1) as usize];
        (line, off - start + 1)
    }

    /// Raw text of a 1-based line, without the trailing newline.
    pub fn line_text(&self, line: u32) -> &str {
        let i = (line as usize).saturating_sub(1);
        if i >= self.line_starts.len() {
            return "";
        }
        let start = self.line_starts[i] as usize;
        let end = if i + 1 < self.line_starts.len() {
            (self.line_starts[i + 1] as usize).saturating_sub(1)
        } else {
            self.text.len()
        };
        self.text.get(start..end).unwrap_or("")
    }

    /// Byte offset of `column` (1-based code-point column) on 1-based `line`.
    pub fn offset_of_line_col(&self, line: u32, column: u32) -> Option<u32> {
        let text = self.line_text(line);
        let mut off = self.line_starts.get(line as usize - 1).copied()?;
        for (i, _) in text.char_indices() {
            if (i as u32) + 1 == column {
                return Some(off + i as u32);
            }
        }
        if column == 1 {
            return Some(off);
        }
        None
    }
}

/// 1-based line / 1-based code-point column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

#[derive(Default, Clone, Debug)]
pub struct Sources {
    files: Vec<SourceFile>,
    by_path: HashMap<String, FileId>,
}

impl Sources {
    pub fn add(&mut self, display_path: String, text: String) -> FileId {
        if let Some(id) = self.by_path.get(&display_path) {
            return *id;
        }
        let id = self.files.len() as FileId;
        self.by_path.insert(display_path.clone(), id);
        self.files.push(SourceFile::new(id, display_path, text));
        id
    }

    pub fn get(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id as usize)
    }

    pub fn id_of_path(&self, path: &str) -> Option<FileId> {
        self.by_path.get(path).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SourceFile> {
        self.files.iter()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn line_col(&self, span: Span) -> (Position, Position) {
        let f = match self.get(span.file) {
            Some(f) => f,
            None => return (Position { line: 1, column: 1 }, Position { line: 1, column: 1 }),
        };
        (self.byte_pos_to_pos(f, f.byte_line_col(span.start)),
         self.byte_pos_to_pos(f, f.byte_line_col(span.end)))
    }

    fn byte_pos_to_pos(&self, f: &SourceFile, (line, byte_col): (u32, u32)) -> Position {
        let text = f.line_text(line);
        let prefix = &text[..(byte_col as usize - 1).min(text.len())];
        Position { line, column: prefix.chars().count() as u32 + 1 }
    }

    /// Column span (1-based code points) of `span`, guaranteed `start <= end`.
    pub fn columns(&self, span: Span) -> (u32, u32) {
        let (a, b) = self.line_col(span);
        if a.line == b.line {
            (a.column, b.column.max(a.column + 1))
        } else {
            let line_len = self.get(span.file).map(|f| f.line_text(a.line).chars().count()).unwrap_or(1);
            (a.column, (line_len as u32) + 1)
        }
    }
}
