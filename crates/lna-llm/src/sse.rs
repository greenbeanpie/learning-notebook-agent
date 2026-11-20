use crate::types::SseFrame;
use lna_types::{ErrorCode, LnaError, LnaResult};

const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024; // 8 MB

/// Incremental UTF-8 safe SSE parser supporting \r\n, multi-line data:, and :heartbeat comments.
pub struct SseParser {
    buf: Vec<u8>,
    current_event: Option<String>,
    data_lines: Vec<String>,
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
            current_event: None,
            data_lines: Vec::new(),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> LnaResult<Vec<SseFrame>> {
        if self.buf.len() + chunk.len() > MAX_FRAME_BYTES {
            return Err(LnaError::new(
                ErrorCode::ModelStreamInterrupted,
                "SSE frame exceeded 8 MB limit",
            ));
        }
        self.buf.extend_from_slice(chunk);

        let mut frames = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line_bytes: Vec<u8> = self.buf.drain(..=pos).collect();
            if line_bytes.ends_with(b"\n") {
                line_bytes.pop();
            }
            if line_bytes.ends_with(b"\r") {
                line_bytes.pop();
            }
            let line = String::from_utf8_lossy(&line_bytes);
            if line.is_empty() {
                if !self.data_lines.is_empty() || self.current_event.is_some() {
                    let data = self.data_lines.join("\n");
                    self.data_lines.clear();
                    let event = self.current_event.take();
                    if !data.is_empty() {
                        frames.push(SseFrame { event, data });
                    }
                }
                continue;
            }
            if line.starts_with(':') {
                // Heartbeat / comment
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                self.current_event = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                let val = rest.strip_prefix(' ').unwrap_or(rest);
                self.data_lines.push(val.to_string());
            }
        }
        Ok(frames)
    }

    pub fn finish(&mut self) -> Option<SseFrame> {
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&self.buf)
                .trim_end_matches('\r')
                .to_string();
            self.buf.clear();
            if let Some(rest) = line.strip_prefix("data:") {
                let val = rest.strip_prefix(' ').unwrap_or(rest);
                self.data_lines.push(val.to_string());
            }
        }
        if !self.data_lines.is_empty() {
            let data = self.data_lines.join("\n");
            self.data_lines.clear();
            return Some(SseFrame {
                event: self.current_event.take(),
                data,
            });
        }
        None
    }
}
