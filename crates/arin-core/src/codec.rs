//! JSON lines framing.
//!
//! One JSON object per line, newline delimited, UTF-8. The payload cap is enforced while
//! reading rather than after: a client must not be able to make the daemon allocate
//! arbitrarily just by withholding a newline.

use crate::error::{Error, Result};
use arin_protocol::MAX_PAYLOAD_BYTES;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Reads newline delimited messages with a hard size cap.
#[derive(Debug)]
pub struct LineReader<R> {
    inner: R,
    buf: Vec<u8>,
    max: usize,
}

impl<R: AsyncBufRead + Unpin> LineReader<R> {
    /// Wrap a reader with the protocol's default cap.
    pub fn new(inner: R) -> Self {
        Self::with_limit(inner, MAX_PAYLOAD_BYTES)
    }

    /// Wrap a reader with an explicit cap. Useful in tests.
    pub fn with_limit(inner: R, max: usize) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            max,
        }
    }

    /// Read the next line, excluding its terminator.
    ///
    /// Returns `Ok(None)` at clean end of stream. On [`Error::PayloadTooLarge`] the
    /// stream is left desynchronised part way through an oversized line, so the caller
    /// must close the connection rather than keep reading from it.
    pub async fn next_line(&mut self) -> Result<Option<&str>> {
        self.buf.clear();
        let mut hit_eof = false;

        loop {
            let mut complete = false;
            let consumed;

            {
                // Borrows `self.inner`. The writes below touch `self.buf`, a disjoint
                // field, which is why this compiles without an intermediate copy.
                let available = self.inner.fill_buf().await?;

                if available.is_empty() {
                    hit_eof = true;
                    break;
                }

                let take = match available.iter().position(|&b| b == b'\n') {
                    Some(idx) => {
                        complete = true;
                        idx
                    }
                    None => available.len(),
                };

                // Check before extending, so an oversized line is refused rather than
                // buffered and then rejected.
                if self.buf.len() + take > self.max {
                    return Err(Error::PayloadTooLarge);
                }
                self.buf.extend_from_slice(&available[..take]);

                // Step over the newline as well when there was one.
                consumed = if complete { take + 1 } else { take };
            }

            self.inner.consume(consumed);
            if complete {
                break;
            }
        }

        if hit_eof && self.buf.is_empty() {
            return Ok(None);
        }
        finish(&self.buf).map(Some)
    }
}

/// Trim a trailing carriage return and validate UTF-8.
fn finish(bytes: &[u8]) -> Result<&str> {
    let line = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    std::str::from_utf8(line).map_err(|_| Error::NotUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn collect(input: &[u8], max: usize) -> (Vec<String>, Option<Error>) {
        let mut reader = LineReader::with_limit(input, max);
        let mut lines = Vec::new();
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => lines.push(line.to_owned()),
                Ok(None) => return (lines, None),
                Err(e) => return (lines, Some(e)),
            }
        }
    }

    #[tokio::test]
    async fn splits_on_newlines() {
        let (lines, err) = collect(b"{\"a\":1}\n{\"b\":2}\n", 1024).await;
        assert!(err.is_none());
        assert_eq!(lines, vec![r#"{"a":1}"#, r#"{"b":2}"#]);
    }

    #[tokio::test]
    async fn a_final_line_without_a_newline_still_counts() {
        let (lines, err) = collect(b"{\"a\":1}", 1024).await;
        assert!(err.is_none());
        assert_eq!(lines, vec![r#"{"a":1}"#]);
    }

    #[tokio::test]
    async fn tolerates_crlf() {
        let (lines, err) = collect(b"{\"a\":1}\r\n", 1024).await;
        assert!(err.is_none());
        assert_eq!(lines, vec![r#"{"a":1}"#]);
    }

    #[tokio::test]
    async fn empty_input_ends_cleanly() {
        let (lines, err) = collect(b"", 1024).await;
        assert!(err.is_none());
        assert!(lines.is_empty());
    }

    #[tokio::test]
    async fn oversized_lines_are_refused() {
        let input = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n";
        let (lines, err) = collect(input, 8).await;
        assert!(lines.is_empty());
        assert!(matches!(err, Some(Error::PayloadTooLarge)));
    }

    #[tokio::test]
    async fn a_line_exactly_at_the_cap_is_accepted() {
        let (lines, err) = collect(b"12345678\n", 8).await;
        assert!(err.is_none());
        assert_eq!(lines, vec!["12345678"]);
    }

    #[tokio::test]
    async fn invalid_utf8_is_rejected() {
        let (_, err) = collect(&[0xff, 0xfe, b'\n'], 1024).await;
        assert!(matches!(err, Some(Error::NotUtf8)));
    }
}
