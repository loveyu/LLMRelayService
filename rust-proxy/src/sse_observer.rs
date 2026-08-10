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
        parse_sse_usage(&body)
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
                    // OpenAI Chat Completions: first non-empty content delta lives in
                    // choices[].delta.content (role-only first chunk has no content).
                    // OpenAI Responses API: output_text.delta carries the first text.
                    _ => {
                        if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
                            for c in choices {
                                let Some(delta) = c.get("delta") else {
                                    continue;
                                };
                                // content (normal) or reasoning_content (deepseek/glm
                                // reasoning models) — either counts as the first token.
                                let text_of = |k: &str| {
                                    delta
                                        .get(k)
                                        .and_then(|x| x.as_str())
                                        .is_some_and(|t| !t.is_empty())
                                };
                                if text_of("content") || text_of("reasoning_content") {
                                    return true;
                                }
                            }
                        }
                        if v.get("type").and_then(|t| t.as_str()) == Some("output_text.delta")
                            && v.get("delta")
                                .and_then(|d| d.as_str())
                                .is_some_and(|t| !t.is_empty())
                        {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

pub fn parse_sse_usage(body: &str) -> SseUsage {
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
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            // Anthropic Messages: usage nests under `message` (message_start);
            // stop_reason rides on `delta.stop_reason` (message_delta).
            match event_type {
                "message_start" => {
                    if let Some(msg) = v.get("message") {
                        extract_usage_values(msg, &mut usage);
                    }
                }
                "message_delta" => {
                    if let Some(delta) = v.get("delta")
                        && let Some(sr) = delta.get("stop_reason").and_then(|x| x.as_str())
                    {
                        usage.stop_reason = Some(sr.to_string());
                    }
                }
                _ => {}
            }
            // Provider-generic: a `usage` object may sit at the root of any frame —
            //   OpenAI Chat Completions final frame: { ..., "usage": {...} }
            //   Anthropic message_delta also carries root-level usage.
            extract_usage_values(&v, &mut usage);
            // OpenAI Responses API nests usage under response.usage on completed.
            if let Some(resp) = v.get("response") {
                extract_usage_values(resp, &mut usage);
                if usage.stop_reason.is_none()
                    && let Some(sr) = resp.get("status").and_then(|x| x.as_str())
                {
                    usage.stop_reason = Some(sr.to_string());
                }
            }
            // OpenAI Chat Completions stop signal: choices[].finish_reason.
            if usage.stop_reason.is_none()
                && let Some(choices) = v.get("choices").and_then(|c| c.as_array())
            {
                for c in choices {
                    if let Some(fr) =
                        c.get("finish_reason").and_then(|x| x.as_str()).filter(|s| !s.is_empty())
                    {
                        usage.stop_reason = Some(fr.to_string());
                        break;
                    }
                }
            }
        }
    }
    // Fallback: some upstreams answer with a single JSON body (e.g. an error
    // envelope) despite an event-stream content-type — parse it whole.
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
        // OpenAI nests cache breakdown under prompt_tokens_details.cached_tokens;
        // only fill when not already provided flat by the provider above.
        if usage.cached_input_tokens.is_none()
            && let Some(cached) = u
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64())
        {
            usage.cached_input_tokens = Some(cached as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn observing_stream_captures_full_openai_sse_body_and_usage() {
        // Drives the real ObservingSseStream (the wrapper now also used by the
        // model-rewrite / conversion SSE branches) over an OpenAI Chat Completions
        // stream and asserts the observer ends up with the full body, parsed usage,
        // and a first-token timestamp — the exact data Path B logs to the console.
        let handle = SseObserverHandle::new(1_000);
        let frames = [
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"pong\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":1,\"total_tokens\":6}}\n\n",
            "data: [DONE]\n\n",
        ];
        let chunks: Vec<Result<Bytes, std::io::Error>> =
            frames.iter().map(|f| Ok(Bytes::copy_from_slice(f.as_bytes()))).collect();
        let mut stream = ObservingSseStream::new(futures::stream::iter(chunks), handle.clone());
        while stream.next().await.is_some() {}
        drop(stream);
        // Drop fires notify_one; Path B awaits this in production.
        handle.notify.notified().await;

        let guard = handle.observer.lock().await;
        let obs = guard.as_ref().expect("observer retained");
        assert!(obs.body_text().contains("\"content\":\"pong\""));
        assert!(obs.body_text().contains("[DONE]"));
        // Role-only first chunk must not count as a token; the "pong" delta must.
        assert!(obs.first_token_at_ms.is_some());
        assert!(obs.first_chunk_at_ms.is_some());
        let usage = obs.parse_usage();
        assert_eq!(usage.input_tokens, Some(5));
        assert_eq!(usage.output_tokens, Some(1));
        assert_eq!(usage.total_tokens, Some(6));
        assert_eq!(usage.stop_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn parses_openai_chat_completions_sse_usage() {
        // Final frame of an OpenAI Chat Completions stream carries root-level usage
        // and finish_reason on the last choice.
        let body = "\
data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\
\n\
data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\
\n\
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":7,\"total_tokens\":49,\"prompt_tokens_details\":{\"cached_tokens\":12}}}\n\
\n\
data: [DONE]\n\
\n";
        let usage = parse_sse_usage(body);
        assert_eq!(usage.input_tokens, Some(42));
        assert_eq!(usage.output_tokens, Some(7));
        assert_eq!(usage.total_tokens, Some(49));
        assert_eq!(usage.cached_input_tokens, Some(12));
        assert_eq!(usage.stop_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn detects_openai_first_token_but_not_role_chunk() {
        assert!(!detect_first_sse_token(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n"
        ));
        assert!(detect_first_sse_token(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n"
        ));
        // Reasoning models (deepseek/glm) stream reasoning_content before content.
        assert!(detect_first_sse_token(
            "data: {\"choices\":[{\"delta\":{\"content\":null,\"reasoning_content\":\" The\"}}]}\n\n"
        ));
    }

    #[test]
    fn parses_anthropic_messages_sse_usage() {
        let body = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":100,\"output_tokens\":1}}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":23}}\n\
\n";
        let usage = parse_sse_usage(body);
        assert_eq!(usage.input_tokens, Some(100));
        // output_tokens: starts at 1 (message_start), overwritten by 23 (message_delta).
        assert_eq!(usage.output_tokens, Some(23));
        assert_eq!(usage.stop_reason.as_deref(), Some("end_turn"));
        // Anthropic first-token detection still works.
        assert!(detect_first_sse_token(
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"x\"}}\n\n"
        ));
    }

    #[test]
    fn parses_openai_responses_sse_usage() {
        // OpenAI Responses API: response.completed nests usage under response.usage.
        let body = "\
event: response.output_text.delta\n\
data: {\"type\":\"output_text.delta\",\"delta\":\"hi\"}\n\
\n\
event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":8,\"output_tokens\":4,\"total_tokens\":12}}}\n\
\n";
        let usage = parse_sse_usage(body);
        assert_eq!(usage.input_tokens, Some(8));
        assert_eq!(usage.output_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(12));
        assert_eq!(usage.stop_reason.as_deref(), Some("completed"));
        assert!(detect_first_sse_token(
            "event: response.output_text.delta\ndata: {\"type\":\"output_text.delta\",\"delta\":\"hi\"}\n\n"
        ));
    }

    #[test]
    fn empty_body_yields_no_usage() {
        let usage = parse_sse_usage("");
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.stop_reason, None);
    }
}
