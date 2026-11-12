//! A command's arguments.

/// The arguments of a command: words split on whitespace, and at most one free-text value, last.
///
/// `Args` is an iterator over the words. [`rest`](Self::rest) takes everything that is left as one value, for a
/// command whose last argument holds spaces. There is no quoting.
#[derive(Clone, Copy, Debug)]
pub struct Args<'a> {
    rest: &'a str,
}

impl<'a> Args<'a> {
    /// The arguments in `text`, which is the command line after the command's name.
    pub const fn new(text: &'a str) -> Self {
        Self { rest: text }
    }

    /// Everything left, with its outer whitespace removed, or `None` when nothing is left. Nothing is left
    /// afterwards.
    pub fn rest(&mut self) -> Option<&'a str> {
        let rest = self.rest.trim();
        self.rest = "";
        if rest.is_empty() { None } else { Some(rest) }
    }

    /// Whether no word is left.
    pub fn is_empty(&self) -> bool {
        self.rest.trim_start().is_empty()
    }
}

impl<'a> Iterator for Args<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let text = self.rest.trim_start();
        if text.is_empty() {
            self.rest = text;
            return None;
        }
        let end = text.find(char::is_whitespace).unwrap_or(text.len());
        let (word, rest) = text.split_at(end);
        self.rest = rest;
        Some(word)
    }
}

/// The name and the value of a `--name=value` flag, or `None` for a word in any other form: without the leading
/// `--`, without an `=`, or with an empty name. The value may be empty and may hold another `=`.
///
/// A command reads its flags from the words after its positional arguments, and refuses a word that is not a
/// flag or a flag it does not know:
///
/// ```
/// let mut msgs = 100u32;
/// for word in mcu_console::Args::new("--msgs=1000") {
///     match mcu_console::flag(word) {
///         Some(("msgs", value)) => msgs = value.parse().unwrap_or(msgs),
///         _ => break,
///     }
/// }
/// assert_eq!(msgs, 1000);
/// ```
pub fn flag(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.strip_prefix("--")?.split_once('=')?;
    (!name.is_empty()).then_some((name, value))
}

/// A count of bytes: digits, then nothing, `B` or `KB`, where `KB` is 1,024 bytes. Returns `None` for any other
/// suffix, a count without digits, or one that does not fit in a `u64`.
///
/// Sizes take no `k`. A console count or rate, such as `net iperf -b 10m`, reads `k` as 1,000, and a size would
/// read it as 1,024, so a bare `k` on a size is refused. A count of something else does not need a helper, because
/// `str::parse::<u32>` refuses one that overflows.
pub fn parse_size(text: &str) -> Option<u64> {
    let (digits, scale) = match text.strip_suffix("KB") {
        Some(digits) => (digits, 1024),
        None => (text.strip_suffix('B').unwrap_or(text), 1),
    };
    // `u64::from_str` takes a leading `+`, which a size does not.
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()?.checked_mul(scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_split_on_runs_of_whitespace() {
        let mut args = Args::new("  a  b\tc ");
        assert_eq!(args.next(), Some("a"));
        assert_eq!(args.next(), Some("b"));
        assert_eq!(args.next(), Some("c"));
        assert_eq!(args.next(), None);
        assert_eq!(args.next(), None);
    }

    #[test]
    fn rest_is_what_is_left_without_its_outer_whitespace() {
        let mut args = Args::new("join  my network   secret ");
        assert_eq!(args.next(), Some("join"));
        assert_eq!(args.rest(), Some("my network   secret"));
        assert_eq!(args.rest(), None);
        assert_eq!(args.next(), None);
    }

    #[test]
    fn rest_of_nothing_is_none() {
        assert_eq!(Args::new("").rest(), None);
        assert_eq!(Args::new("   ").rest(), None);
    }

    #[test]
    fn is_empty_ignores_whitespace() {
        assert!(Args::new("").is_empty());
        assert!(Args::new("  ").is_empty());
        let mut args = Args::new(" a ");
        assert!(!args.is_empty());
        args.next();
        assert!(args.is_empty());
    }
}
