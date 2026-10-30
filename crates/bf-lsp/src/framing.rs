//! `Content-Length` framing of JSON-RPC messages.

/// Accumulates raw bytes from the server and yields complete message bodies.
#[derive(Default)]
pub(crate) struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    /// Appends freshly read bytes.
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Returns the next complete body, `Ok(None)` if more data is needed, or `Err` for a
    /// malformed header (the header is discarded so parsing can resynchronize).
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, String> {
        let Some(header_end) = find(&self.buf, b"\r\n\r\n") else {
            return Ok(None);
        };
        let body_start = header_end + 4;
        let header = String::from_utf8_lossy(&self.buf[..header_end]);
        let length = header.split("\r\n").find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        });
        let Some(length) = length else {
            let header = header.into_owned();
            self.buf.drain(..body_start);
            return Err(format!(
                "missing or invalid Content-Length in header {header:?}"
            ));
        };
        if self.buf.len() < body_start + length {
            return Ok(None);
        }
        let body = self.buf[body_start..body_start + length].to_vec();
        self.buf.drain(..body_start + length);
        Ok(Some(body))
    }
}

/// Wraps a JSON body in a `Content-Length` header.
pub(crate) fn encode(body: &[u8]) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(reader: &mut FrameReader) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(Some(body)) = reader.next_frame() {
            out.push(String::from_utf8(body).unwrap());
        }
        out
    }

    #[test]
    fn multiple_messages_in_one_buffer() {
        let mut data = encode(br#"{"a":1}"#);
        data.extend(encode(br#"{"b":2}"#));
        let mut r = FrameReader::default();
        r.push(&data);
        assert_eq!(frames(&mut r), vec![r#"{"a":1}"#, r#"{"b":2}"#]);
    }

    #[test]
    fn byte_by_byte_partial_reads() {
        let body = r#"{"msg":"grüße ✓"}"#;
        let data = encode(body.as_bytes());
        let mut r = FrameReader::default();
        let mut got = Vec::new();
        for b in &data {
            r.push(std::slice::from_ref(b));
            got.extend(frames(&mut r));
        }
        assert_eq!(got, vec![body]);
    }

    #[test]
    fn split_inside_header_and_body() {
        let mut data = encode(br#"{"first":true}"#);
        data.extend(encode(br#"{"second":true}"#));
        let mut r = FrameReader::default();
        r.push(&data[..7]);
        assert!(frames(&mut r).is_empty());
        r.push(&data[7..40]);
        assert_eq!(frames(&mut r), vec![r#"{"first":true}"#]);
        r.push(&data[40..]);
        assert_eq!(frames(&mut r), vec![r#"{"second":true}"#]);
    }

    #[test]
    fn extra_headers_and_case() {
        let mut r = FrameReader::default();
        r.push(b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}");
        assert_eq!(frames(&mut r), vec!["{}"]);
    }

    #[test]
    fn malformed_header_resyncs() {
        let mut r = FrameReader::default();
        r.push(b"Bogus: 1\r\n\r\n");
        r.push(&encode(b"{}"));
        assert!(r.next_frame().is_err());
        assert_eq!(frames(&mut r), vec!["{}"]);
    }
}
