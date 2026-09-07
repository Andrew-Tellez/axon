//! Colours for the output. No dependencies: four ANSI sequences.
//!
//! Blue informs, yellow warns, red blocks. The hierarchy matters more than the
//! colour: in a thirty-line list, what has to be fixed must stand out without
//! reading the whole thing.
//!
//! They turn themselves off when the output is not a terminal —a pipe, a CI
//! log, a file— because there the sequences are noise that dirties the diff.
//! And they honour `NO_COLOR`, which is the convention, and `CLICOLOR_FORCE`
//! for the opposite case.
use std::io::IsTerminal;
use std::sync::OnceLock;

/// Whether the destination accepts colour. Public because whoever highlights
/// text needs to know: with no colour, highlighting must not alter the content.
pub fn enabled() -> bool {
    static A: OnceLock<bool> = OnceLock::new();
    *A.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var_os("CLICOLOR_FORCE").is_some() {
            return true;
        }
        // stderr and stdout are judged together: mixing coloured and plain in
        // the same report reads worse than not colouring at all
        std::io::stdout().is_terminal() && std::io::stderr().is_terminal()
    })
}

fn paint(code: &str, text: &str) -> String {
    if enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Red: fix it before going any further.
pub fn red(t: &str) -> String {
    paint("1;31", t)
}
/// Yellow: worth a look, does not block.
pub fn yellow(t: &str) -> String {
    paint("1;33", t)
}
/// Blue: information and advice.
pub fn blue(t: &str) -> String {
    paint("1;34", t)
}
/// Green: it went well.
pub fn green(t: &str) -> String {
    paint("1;32", t)
}
/// Grey: secondary context, so it does not compete with what matters.
pub fn grey(t: &str) -> String {
    paint("2", t)
}
/// Bold with no colour, to highlight a name inside a sentence.
pub fn bold(t: &str) -> String {
    paint("1", t)
}
