pub(crate) mod check;
pub(crate) mod list;
pub(crate) mod load;
pub(crate) mod lsp;
pub(crate) mod online;
pub(crate) mod view;

/// `n` of `word`, like "1 file" or "2 files".
pub(crate) fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}
