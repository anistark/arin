//! JSON lines framing.
//!
//! One JSON object per line, newline delimited, UTF-8. The payload cap is enforced while
//! reading rather than after: a client must not be able to make the daemon allocate
//! arbitrarily just by withholding a newline.

use crate::error::{Error, Result};
use arin_protocol::MAX_PAYLOAD_BYTES;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Reads newline delimited messages with a hard size cap.
///
/// # Cancellation
///
/// [`LineReader::next_line`] is cancellation safe, which the socket server depends on: it
/// reads inside a `select!` alongside the invalidations it pushes out, so the read future
/// is dropped every time an announcement wins the race.
///
/// That is only true because the buffer survives between calls. Bytes are consumed from
/// the underlying reader as they are seen, so a partial line that was thrown away could
/// not be re-read from anywhere, and clearing on entry would silently truncate any
/// message that spanned a read boundary. The buffer is therefore cleared once a line has
/// been handed out, not when the next one is asked for.
#[derive(Debug)]
pub struct LineReader<R> {
    inner: R,
    buf: Vec<u8>,
    max: usize,
    /// Whether `buf` holds a line already returned, rather than one part way through.
    delivered: bool,
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
            delivered: false,
        }
    }

    /// Read the next line, excluding its terminator.
    ///
    /// Returns `Ok(None)` at clean end of stream. On [`Error::PayloadTooLarge`] the
    /// stream is left desynchronised part way through an oversized line, so the caller
    /// must close the connection rather than keep reading from it.
    ///
    /// Cancellation safe: dropping the returned future keeps whatever was read so far,
    /// and the next call carries on from there.
    pub async fn next_line(&mut self) -> Result<Option<&str>> {
        // Only the previous line is discarded here. Anything read since is part way
        // through the next one and has already been consumed from the reader.
        if self.delivered {
            self.buf.clear();
            self.delivered = false;
        }
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
        self.delivered = true;
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

    /// The property the socket server rests on. It reads inside a `select!` against the
    /// invalidations it pushes, so a read that has seen half a line is dropped whenever an
    /// announcement wins the race. Those bytes are already consumed from the reader and
    /// exist nowhere else, so losing them truncates the message and desynchronises the
    /// stream. Nothing above this notices, because the truncated JSON simply fails to
    /// parse and is answered as a schema error.
    #[tokio::test]
    async fn a_cancelled_read_keeps_what_it_had() {
        use tokio::io::{AsyncWriteExt, BufReader, duplex};

        let (mut writer, read_half) = duplex(64);
        let mut reader = LineReader::new(BufReader::new(read_half));

        writer.write_all(br#"{"first":"#).await.unwrap();

        // No newline yet, so this cannot finish. Dropping the future is exactly what
        // `select!` does to the losing branch.
        let cancelled =
            tokio::time::timeout(std::time::Duration::from_millis(20), reader.next_line()).await;
        assert!(cancelled.is_err(), "the read should not have completed yet");

        writer.write_all(b"1}\n").await.unwrap();

        let line = reader.next_line().await.unwrap().unwrap();
        assert_eq!(line, r#"{"first":1}"#, "the first half was dropped");
    }

    /// And the line after a resumed one must not carry any of it.
    #[tokio::test]
    async fn a_resumed_line_does_not_leak_into_the_next() {
        use tokio::io::{AsyncWriteExt, BufReader, duplex};

        let (mut writer, read_half) = duplex(64);
        let mut reader = LineReader::new(BufReader::new(read_half));

        writer.write_all(b"alpha").await.unwrap();
        let _ =
            tokio::time::timeout(std::time::Duration::from_millis(20), reader.next_line()).await;
        writer.write_all(b"-beta\ngamma\n").await.unwrap();

        assert_eq!(reader.next_line().await.unwrap().unwrap(), "alpha-beta");
        assert_eq!(reader.next_line().await.unwrap().unwrap(), "gamma");
    }
}
