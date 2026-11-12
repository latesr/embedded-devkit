//! Where a secret argument starts on the line being typed.

use crate::{Commands, Help};

/// The offset in `line` where the secret argument of one of `commands` starts, or `None` when the line is not
/// a command with a secret argument, or the argument has not started.
///
/// A [`Help`] entry with `secret` marks its last placeholder. `wifi join <ssid> <password>` opens with the words
/// `wifi join`, and the secret is the second argument. Every byte from the first byte of that argument is
/// secret, spaces included, because a free-text argument runs to the end of the line.
pub(crate) fn secret_start<C: Commands>(commands: &C, line: &[u8]) -> Option<usize> {
    commands
        .help()
        .filter(|help| help.secret)
        .find_map(|help| secret_start_for(help, line))
}

fn secret_start_for(help: &Help, line: &[u8]) -> Option<usize> {
    let mut words = help.usage.split_ascii_whitespace().peekable();
    let mut at = 0;
    let mut any_word = false;
    while let Some(word) = words.next_if(|word| !word.starts_with('<')) {
        any_word = true;
        at = after_spaces(line, at);
        let end = at.checked_add(word.len())?;
        if line.get(at..end)? != word.as_bytes() {
            return None;
        }
        if line.get(end).is_some_and(|byte| *byte != b' ') {
            return None;
        }
        at = end;
    }
    let placeholders = words.count();
    if !any_word || placeholders == 0 {
        return None;
    }
    for _ in 1..placeholders {
        let start = after_spaces(line, at);
        if start == at {
            return None;
        }
        at = start;
        while line.get(at).is_some_and(|byte| *byte != b' ') {
            at = at.checked_add(1)?;
        }
    }
    let start = after_spaces(line, at);
    (start > at).then_some(start)
}

/// The offset of the first byte at or after `at` that is not a space, or the end of `line`.
fn after_spaces(line: &[u8], mut at: usize) -> usize {
    while line.get(at).is_some_and(|byte| *byte == b' ') {
        at = at.saturating_add(1);
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn help(usage: &'static str) -> Help {
        Help {
            usage,
            summary: "",
            secret: true,
        }
    }

    #[test]
    fn the_secret_starts_at_the_first_byte_of_its_argument() {
        let join = help("join <ssid> <password>");
        assert_eq!(secret_start_for(&join, b"join home "), Some(10));
        assert_eq!(secret_start_for(&join, b"join home pw"), Some(10));
        assert_eq!(
            secret_start_for(&join, b"join home pw with spaces"),
            Some(10)
        );
        assert_eq!(secret_start_for(&join, b"join  home   p"), Some(13));
    }

    #[test]
    fn nothing_is_secret_before_the_argument_starts() {
        let join = help("join <ssid> <password>");
        assert_eq!(secret_start_for(&join, b""), None);
        assert_eq!(secret_start_for(&join, b"join"), None);
        assert_eq!(secret_start_for(&join, b"join "), None);
        assert_eq!(secret_start_for(&join, b"join home"), None);
    }

    #[test]
    fn another_command_or_a_longer_word_is_not_the_command() {
        let join = help("join <ssid> <password>");
        assert_eq!(secret_start_for(&join, b"help home pw"), None);
        assert_eq!(secret_start_for(&join, b"joint home pw"), None);
        assert_eq!(secret_start_for(&join, b"jo home pw"), None);
    }

    #[test]
    fn several_fixed_words_and_a_single_placeholder_work() {
        let set = help("wifi password <password>");
        assert_eq!(secret_start_for(&set, b"wifi password"), None);
        assert_eq!(secret_start_for(&set, b"wifi password "), Some(14));
        assert_eq!(secret_start_for(&set, b"wifi password x"), Some(14));
        assert_eq!(secret_start_for(&set, b"wifi  password  x"), Some(16));
        assert_eq!(secret_start_for(&set, b"wifi passwords x"), None);
    }

    #[test]
    fn a_usage_without_a_placeholder_has_no_secret() {
        assert_eq!(secret_start_for(&help("reset"), b"reset x"), None);
        assert_eq!(secret_start_for(&help("<x>"), b"a b"), None);
    }
}
