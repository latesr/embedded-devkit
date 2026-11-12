//! One command line, assembled a byte at a time.

/// What one input byte did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Nothing to write. The byte was ignored, or it did not fit the line.
    None,
    /// The byte joined the line. The driver echoes it.
    Echo(u8),
    /// Backspace removed the last byte. The driver erases one character on the terminal.
    Erase,
    /// Ctrl-C abandoned the line. The driver writes a new prompt.
    Cancel,
    /// The line ended. The driver calls [`Input::take`].
    Line,
}

/// Why a completed line cannot be run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineError {
    /// The line had more bytes than the buffer holds.
    TooLong,
    /// The line is not UTF-8.
    NotUtf8,
}

/// A line buffer of `N` bytes that assembles one command line from input bytes.
///
/// The driver calls [`push`](Self::push) for each byte it reads and acts on the [`Action`]. When the action is
/// [`Action::Line`], it calls [`take`](Self::take) for the line.
#[derive(Debug)]
pub struct Input<const N: usize> {
    buf: [u8; N],
    len: usize,
    /// Set when a byte did not fit. The line is refused when it ends.
    overflow: bool,
    /// The last byte was CR, so an LF that follows is the same line end.
    last_cr: bool,
}

impl<const N: usize> Default for Input<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Input<N> {
    /// An empty line buffer.
    pub const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
            overflow: false,
            last_cr: false,
        }
    }

    /// Takes one input byte and reports what it did.
    pub fn push(&mut self, byte: u8) -> Action {
        let after_cr = self.last_cr;
        self.last_cr = byte == b'\r';
        match byte {
            b'\n' if after_cr => Action::None,
            b'\r' | b'\n' => Action::Line,
            0x08 | 0x7f => {
                if self.len == 0 || self.overflow {
                    return Action::None;
                }
                self.len = self.len.saturating_sub(1);
                Action::Erase
            }
            0x03 => {
                self.clear();
                Action::Cancel
            }
            b'\t' | 0x20..=0x7e | 0x80..=0xff => {
                let byte = if byte == b'\t' { b' ' } else { byte };
                match self.buf.get_mut(self.len) {
                    Some(slot) => {
                        *slot = byte;
                        self.len = self.len.saturating_add(1);
                        Action::Echo(byte)
                    }
                    None => {
                        self.overflow = true;
                        Action::None
                    }
                }
            }
            _ => Action::None,
        }
    }

    /// The bytes of the line so far.
    pub fn line(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or(&[])
    }

    /// The completed line, and the buffer emptied for the next one.
    ///
    /// Fails with [`LineError::TooLong`] when a byte did not fit, and with [`LineError::NotUtf8`] when the
    /// bytes are not UTF-8. The buffer is emptied in every case.
    pub fn take(&mut self) -> Result<&str, LineError> {
        let len = self.len;
        let overflow = self.overflow;
        self.clear();
        if overflow {
            return Err(LineError::TooLong);
        }
        let bytes = self.buf.get(..len).unwrap_or(&[]);
        core::str::from_utf8(bytes).map_err(|_| LineError::NotUtf8)
    }

    fn clear(&mut self) {
        self.len = 0;
        self.overflow = false;
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn push_all<const N: usize>(input: &mut Input<N>, bytes: &[u8]) -> Vec<Action> {
        bytes.iter().map(|&byte| input.push(byte)).collect()
    }

    #[test]
    fn a_line_ends_at_lf_and_each_byte_echoes() {
        let mut input = Input::<8>::new();
        assert_eq!(
            push_all(&mut input, b"ab\n"),
            [Action::Echo(b'a'), Action::Echo(b'b'), Action::Line]
        );
        assert_eq!(input.take(), Ok("ab"));
    }

    #[test]
    fn cr_ends_a_line_and_an_lf_after_it_is_the_same_line_end() {
        let mut input = Input::<8>::new();
        assert_eq!(
            push_all(&mut input, b"a\r\nb\r"),
            [
                Action::Echo(b'a'),
                Action::Line,
                Action::None,
                Action::Echo(b'b'),
                Action::Line
            ]
        );
        assert_eq!(input.take(), Ok("ab"));
    }

    #[test]
    fn a_line_is_taken_once_and_the_next_line_starts_empty() {
        let mut input = Input::<8>::new();
        push_all(&mut input, b"a\n");
        assert_eq!(input.take(), Ok("a"));
        push_all(&mut input, b"b\n");
        assert_eq!(input.take(), Ok("b"));
    }

    #[test]
    fn a_tab_is_a_space() {
        let mut input = Input::<8>::new();
        assert_eq!(input.push(b'\t'), Action::Echo(b' '));
        input.push(b'\n');
        assert_eq!(input.take(), Ok(" "));
    }

    #[test]
    fn backspace_deletes_the_last_byte_and_does_nothing_on_an_empty_line() {
        let mut input = Input::<8>::new();
        assert_eq!(input.push(0x7f), Action::None);
        assert_eq!(
            push_all(&mut input, b"ab\x7fc\x08\n"),
            [
                Action::Echo(b'a'),
                Action::Echo(b'b'),
                Action::Erase,
                Action::Echo(b'c'),
                Action::Erase,
                Action::Line
            ]
        );
        assert_eq!(input.take(), Ok("a"));
    }

    #[test]
    fn ctrl_c_abandons_the_line() {
        let mut input = Input::<8>::new();
        assert_eq!(
            push_all(&mut input, b"ab\x03c\n"),
            [
                Action::Echo(b'a'),
                Action::Echo(b'b'),
                Action::Cancel,
                Action::Echo(b'c'),
                Action::Line
            ]
        );
        assert_eq!(input.take(), Ok("c"));
    }

    #[test]
    fn other_control_bytes_are_ignored() {
        let mut input = Input::<8>::new();
        assert_eq!(
            push_all(&mut input, b"\x01\x1b\x00a\n"),
            [
                Action::None,
                Action::None,
                Action::None,
                Action::Echo(b'a'),
                Action::Line
            ]
        );
        assert_eq!(input.take(), Ok("a"));
    }

    #[test]
    fn a_line_longer_than_the_buffer_is_refused_whole() {
        let mut input = Input::<4>::new();
        assert_eq!(
            push_all(&mut input, b"abcde\x7f\n"),
            [
                Action::Echo(b'a'),
                Action::Echo(b'b'),
                Action::Echo(b'c'),
                Action::Echo(b'd'),
                Action::None,
                Action::None,
                Action::Line
            ]
        );
        assert_eq!(input.take(), Err(LineError::TooLong));
        push_all(&mut input, b"x\n");
        assert_eq!(input.take(), Ok("x"));
    }

    #[test]
    fn a_line_that_fills_the_buffer_exactly_is_accepted() {
        let mut input = Input::<4>::new();
        push_all(&mut input, b"abcd\n");
        assert_eq!(input.take(), Ok("abcd"));
    }

    #[test]
    fn a_line_that_is_not_utf8_is_refused() {
        let mut input = Input::<8>::new();
        assert_eq!(
            push_all(&mut input, b"a\xff\n"),
            [Action::Echo(b'a'), Action::Echo(0xff), Action::Line]
        );
        assert_eq!(input.take(), Err(LineError::NotUtf8));
        push_all(&mut input, "é\n".as_bytes());
        assert_eq!(input.take(), Ok("é"));
    }

    #[test]
    fn an_empty_line_is_empty() {
        let mut input = Input::<8>::new();
        assert_eq!(input.push(b'\n'), Action::Line);
        assert_eq!(input.take(), Ok(""));
    }

    #[test]
    fn line_shows_the_bytes_so_far() {
        let mut input = Input::<8>::new();
        push_all(&mut input, b"ab");
        assert_eq!(input.line(), b"ab");
        input.push(b'\n');
        input.take().unwrap();
        assert_eq!(input.line(), b"");
    }

    proptest! {
        #[test]
        fn any_input_never_panics_and_every_line_fits(bytes: Vec<u8>) {
            let mut input = Input::<16>::new();
            for &byte in &bytes {
                prop_assert!(input.line().len() <= 16);
                if input.push(byte) == Action::Line {
                    match input.take() {
                        Ok(line) => prop_assert!(line.len() <= 16),
                        Err(LineError::TooLong) => {}
                        Err(LineError::NotUtf8) => {}
                    }
                    prop_assert_eq!(input.line(), b"");
                }
            }
        }

        #[test]
        fn a_line_of_printable_bytes_is_taken_as_typed(line in "[ -~]{0,16}") {
            let mut input = Input::<16>::new();
            for byte in line.bytes() {
                prop_assert_eq!(input.push(byte), Action::Echo(byte));
            }
            prop_assert_eq!(input.push(b'\n'), Action::Line);
            prop_assert_eq!(input.take(), Ok(line.as_str()));
        }
    }
}
