//! A fixed buffer for one output line.

/// A buffer of `N` bytes. Formatting into it cuts what does not fit instead of failing, because a body line
/// that is cut is better than a command that cannot reply.
pub(crate) struct LineBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> LineBuf<N> {
    pub(crate) const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    /// The contents as text. A UTF-8 sequence that the cut broke is dropped.
    pub(crate) fn as_str(&self) -> &str {
        let bytes = self.buf.get(..self.len).unwrap_or(&[]);
        match core::str::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => bytes
                .get(..error.valid_up_to())
                .and_then(|valid| core::str::from_utf8(valid).ok())
                .unwrap_or(""),
        }
    }
}

impl<const N: usize> core::fmt::Write for LineBuf<N> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let room = N.saturating_sub(self.len);
        let bytes = text.as_bytes();
        let bytes = bytes.get(..room.min(bytes.len())).unwrap_or(&[]);
        if let Some(slot) = self
            .buf
            .get_mut(self.len..self.len.saturating_add(bytes.len()))
        {
            slot.copy_from_slice(bytes);
            self.len = self.len.saturating_add(bytes.len());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write as _;

    #[test]
    fn text_that_fits_is_kept_whole() {
        let mut buf = LineBuf::<8>::new();
        write!(buf, "a{}c", 1).unwrap();
        assert_eq!(buf.as_str(), "a1c");
    }

    #[test]
    fn text_that_does_not_fit_is_cut() {
        let mut buf = LineBuf::<4>::new();
        write!(buf, "abcdef").unwrap();
        assert_eq!(buf.as_str(), "abcd");
        write!(buf, "g").unwrap();
        assert_eq!(buf.as_str(), "abcd");
    }

    #[test]
    fn a_utf8_sequence_broken_by_the_cut_is_dropped() {
        let mut buf = LineBuf::<3>::new();
        write!(buf, "aé").unwrap();
        assert_eq!(buf.as_str(), "aé");
        let mut buf = LineBuf::<2>::new();
        write!(buf, "aé").unwrap();
        assert_eq!(buf.as_str(), "a");
    }
}
