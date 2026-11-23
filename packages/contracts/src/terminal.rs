//! Helpers for terminal input (`terminal.ts`).

/// The longest `data` one `terminal.write` accepts, in UTF-16 code units.
pub const TERMINAL_WRITE_MAX_LENGTH: usize = 65_536;

/// Splits terminal input into pieces `terminal.write` accepts (`chunkTerminalWrite`), never
/// splitting a character.
pub fn chunk_terminal_write(data: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let (mut start, mut units) = (0, 0);
    for (index, ch) in data.char_indices() {
        if units + ch.len_utf16() > TERMINAL_WRITE_MAX_LENGTH {
            chunks.push(&data[start..index]);
            (start, units) = (index, 0);
        }
        units += ch.len_utf16();
    }
    if start < data.len() {
        chunks.push(&data[start..]);
    }
    chunks
}
