use std::{
    hash::{DefaultHasher, Hash as _, Hasher as _},
    ops::Range,
    sync::Arc,
};

/// An immutable list of lines in one shared buffer. Cloning is two `Arc` bumps,
/// so patch models, highlighters and views share one copy of the text.
#[derive(Clone, Debug)]
pub struct TextLines {
    text: Arc<str>,
    /// Byte offset of each line start, plus one trailing entry at the end.
    starts: Arc<[u32]>,
}

impl Default for TextLines {
    fn default() -> Self {
        Self { text: Arc::from(""), starts: Arc::from([0u32].as_slice()) }
    }
}

impl TextLines {
    /// Builds from separate lines. Lines must not contain `\n`.
    pub fn from_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut text = String::new();
        let mut starts = vec![0u32];
        for line in lines {
            text.push_str(line);
            starts.push(text.len() as u32);
        }
        Self { text: text.into(), starts: starts.into() }
    }

    /// Splits a file's contents on `\n`, dropping a final empty line and `\r`.
    pub fn from_contents(contents: &str) -> Self {
        let contents = contents.strip_suffix('\n').unwrap_or(contents);
        if contents.is_empty() {
            return Self::default();
        }
        Self::from_lines(contents.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)))
    }

    pub fn len(&self) -> usize {
        self.starts.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The text of line `index`, without a newline.
    pub fn line(&self, index: usize) -> &str {
        &self.text[self.byte_range(index)]
    }

    pub fn byte_range(&self, index: usize) -> Range<usize> {
        self.starts[index] as usize..self.starts[index + 1] as usize
    }

    /// A content hash, used to key highlight caches by file version.
    pub fn content_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.text.hash(&mut hasher);
        self.starts.len().hash(&mut hasher);
        hasher.finish()
    }

    /// Bytes held, for memory accounting.
    pub fn heap_size(&self) -> usize {
        self.text.len() + self.starts.len() * 4
    }
}
