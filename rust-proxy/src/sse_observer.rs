use bytes::Bytes;
use futures::Stream;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::{Mutex, Notify};
use tokio::time::Instant;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_DURATION_SECS: u64 = 300;

#[derive(Debug, Clone, Default)]
pub struct SseUsage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub cache_creation_input_tokens: Option<u32>,
    pub cache_read_input_tokens: Option<u32>,
    pub cached_input_tokens: Option<u32>,
    pub stop_reason: Option<String>,
}

pub(crate) struct PassthroughObserver {
    buffer: Vec<u8>,
    pub first_chunk_at_ms: Option<u64>,
    pub first_token_at_ms: Option<u64>,
    pub total_bytes: u64,
    truncated: bool,
    created_at_ms: u64,
    start: Instant,
    deadline: Instant,
}

impl PassthroughObserver {
    pub fn new(created_at_ms: u64) -> Self {
        let start = Instant::now();
        Self {
            buffer: Vec::with_capacity(65_536),
            first_chunk_at_ms: None,
            first_token_at_ms: None,
            total_bytes: 0,
            truncated: false,
            created_at_ms,
            start,
            deadline: start + std::time::Duration::from_secs(MAX_DURATION_SECS),
        }
    }

    pub fn observe_chunk(&mut self, chunk: &[u8]) {
        if self.truncated {
            return;
        }
        let now = self.start.elapsed().as_millis() as u64;
        if self.first_chunk_at_ms.is_none() {
            self.first_chunk_at_ms = self.created_at_ms.checked_add(now);
        }
        if self.total_bytes as usize + chunk.len() > MAX_BYTES {
            let remaining = MAX_BYTES.saturating_sub(self.buffer.len());
            if remaining > 0 {
                self.buffer.extend_from_slice(&chunk[..remaining]);
            }
            self.total_bytes = MAX_BYTES as u64;
            self.truncated = true;
        } else {
            self.buffer.extend_from_slice(chunk);
            self.total_bytes += chunk.len() as u64;
        }
        if self.first_token_at_ms.is_none() {
            let text = String::from_utf8_lossy(chunk);
            if detect_first_sse_token(&text) {
                self.first_token_at_ms = self.created_at_ms.checked_add(now);
            }
        }
    }

    pub fn check_timeout(&self) -> bool {
        Instant::now() >= self.deadline
    }

    pub fn parse_usage(&self) -> SseUsage {
        let body = String::from_utf8_lossy(&self.buffer);
        parse_anthropic_sse_usage(&body)
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.buffer).to_string()
    }
}

#[derive(Clone)]
pub struct SseObserverHandle {
    pub observer: Arc<Mutex<Option<PassthroughObserver>>>,
    pub notify: Arc<Notify>,
}

impl SseObserverHandle {
    pub fn new(created_at_ms: u64) -> Self {
        Self {
            observer: Arc::new(Mutex::new(Some(PassthroughObserver::new(created_at_ms)))),
            notify: Arc::new(Notify::new()),
        }
    }
}

pub struct ObservingSseStream<S> {
    inner: S,
    observer: Arc<Mutex<Option<PassthroughObserver>>>,
    notify: Arc<Notify>,
}

impl<S> ObservingSseStream<S> {
    pub fn new(inner: S, handle: SseObserverHandle) -> Self {
        Self { inner, observer: handle.observer, notify: handle.notify }
    }
}

impl<S> Drop for ObservingSseStream<S> {
    fn drop(&mut self) {
        self.notify.notify_one();
    }
}

impl<S, E> Stream for ObservingSseStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if let Ok(mut guard) = self.observer.try_lock()
                    && let Some(ref mut obs) = *guard
                {
                    if !obs.check_timeout() {
                        obs.observe_chunk(&chunk);
                    } else if !obs.truncated {
                        obs.truncated = true;
                    }
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(axum::Error::new(format!("{e:?}"))))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn detect_first_sse_token(text: &str) -> bool {
    let events = text.split("\n\n");
    for event in events {
        let trimmed = event.trim();
        if trimmed.is_empty() {
            continue;
        }
        for line in trimmed.lines() {
            if let Some(data) = line.strip_prefix("data: ")
                && let Ok(v) = serde_json::from_str::<Value>(data)
            {
                let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match ty {
                    "content_block_start" => {
                        if let Some(block) = v.get("content_block")
                            && block.get("type").and_then(|t| t.as_str()) == Some("text")
                        {
                            return true;
                        }
                    }
                    "content_block_delta" => {
                        if let Some(delta) = v.get("delta")
                            && delta.get("type").and_then(|t| t.as_str()) == Some("text_delta")
                            && !delta.get("text").and_then(|t| t.as_str()).unwrap_or("").is_empty()
                        {
                            return true;
                        }
                    }
                    "message_start" => {
                        if let Some(msg) = v.get("message")
                            && msg
                                .get("content")
                                .and_then(|c| c.as_array())
                                .is_some_and(|arr| !arr.is_empty())
                        {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    false
}

pub fn parse_anthropic_sse_usage(body: &str) -> SseUsage {
    let mut usage = SseUsage::default();
    let events = body.split("\n\n");
    for event in events {
        let trimmed = event.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut event_type = "";
        for line in trimmed.lines() {
            if let Some(et) = line.strip_prefix("event: ") {
                event_type = et.trim();
            }
            if let Some(data) = line.strip_prefix("data: ")
                && let Ok(v) = serde_json::from_str::<Value>(data)
            {
                match event_type {
                    "message_start" => {
                        if let Some(msg) = v.get("message") {
                            extract_usage_values(msg, &mut usage);
                        }
                    }
                    "message_delta" => {
                        if let Some(delta) = v.get("delta")
                            && let Some(sr) = delta.get("stop_reason").and_then(|v| v.as_str())
                        {
                            usage.stop_reason = Some(sr.to_string());
                        }
                        extract_usage_values(&v, &mut usage);
                    }
                    _ => {}
                }
            }
        }
    }
    if usage.input_tokens.is_none()
        && usage.output_tokens.is_none()
        && let Ok(v) = serde_json::from_str::<Value>(body)
    {
        extract_usage_values(&v, &mut usage);
        if usage.stop_reason.is_none()
            && let Some(sr) = v.get("stop_reason").and_then(|v| v.as_str())
        {
            usage.stop_reason = Some(sr.to_string());
        }
    }
    usage
}

fn extract_usage_values(v: &Value, usage: &mut SseUsage) {
    if let Some(u) = v.get("usage") {
        let extract = |keys: &[&str]| -> Option<u32> {
            for k in keys {
                if let Some(val) = u.get(k).and_then(|v| v.as_u64()) {
                    return Some(val as u32);
                }
            }
            None
        };
        let update = |current: &mut Option<u32>, new: Option<u32>| {
            if let Some(n) = new
                && (!current.is_some_and(|c| c > 0) || n > 0)
            {
                *current = Some(n);
            }
        };
        update(&mut usage.input_tokens, extract(&["input_tokens", "prompt_tokens"]));
        update(&mut usage.output_tokens, extract(&["output_tokens", "completion_tokens"]));
        update(&mut usage.total_tokens, extract(&["total_tokens"]));
        update(&mut usage.cache_creation_input_tokens, extract(&["cache_creation_input_tokens"]));
        update(&mut usage.cache_read_input_tokens, extract(&["cache_read_input_tokens"]));
        update(&mut usage.cached_input_tokens, extract(&["cached_tokens"]));
    }
}
