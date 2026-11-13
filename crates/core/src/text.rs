//! Text as the editor holds it.

/// `text` with every `\r\n` and lone `\r` turned into `\n`, as the editor
/// buffer normalizes text it is given.
pub fn normalize_line_endings(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_string()
    }
}
