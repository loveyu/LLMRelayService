use crate::config::SyncConfigPayload;
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::{Mutex, mpsc};
use tracing::{error, info, warn};

const IPC_SOCKET_PATH: &str = "/tmp/lrs-ipc.sock";
const MAX_FRAME_LENGTH: usize = 16 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TsToRustMessage {
    SyncConfig { payload: Box<SyncConfigPayload> },
    ReloadConfig,
    Ping { timestamp: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RustToTsMessage {
    RequestLog {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "createdAt")]
        created_at: u64,
        method: String,
        #[serde(rename = "routePrefix")]
        route_prefix: String,
        #[serde(rename = "upstreamType")]
        upstream_type: String,
        path: String,
        url: String,
        #[serde(rename = "targetUrl")]
        target_url: String,
        #[serde(rename = "requestModel")]
        request_model: String,
        #[serde(rename = "originalPayload")]
        original_payload: Option<String>,
        #[serde(rename = "forwardedPayload")]
        forwarded_payload: Option<String>,
        #[serde(rename = "originalHeaders")]
        original_headers: serde_json::Value,
        #[serde(rename = "forwardHeaders")]
        forward_headers: serde_json::Value,
        #[serde(rename = "apiKeyId")]
        api_key_id: Option<String>,
        #[serde(rename = "apiKeyName")]
        api_key_name: Option<String>,
    },
    ResponseLog {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "responseStatus")]
        response_status: u16,
        #[serde(rename = "responseStatusText")]
        response_status_text: String,
        #[serde(rename = "responseHeaders")]
        response_headers: serde_json::Value,
        #[serde(rename = "responseBodyBytes")]
        response_body_bytes: u64,
        #[serde(rename = "firstChunkAt")]
        first_chunk_at: Option<u64>,
        #[serde(rename = "firstTokenAt")]
        first_token_at: Option<u64>,
        #[serde(rename = "completedAt")]
        completed_at: Option<u64>,
        #[serde(rename = "hasStreamingContent")]
        has_streaming_content: bool,
        #[serde(rename = "responseModel")]
        response_model: Option<String>,
        #[serde(rename = "stopReason")]
        stop_reason: Option<String>,
        #[serde(rename = "inputTokens")]
        input_tokens: Option<u32>,
        #[serde(rename = "outputTokens")]
        output_tokens: Option<u32>,
        #[serde(rename = "totalTokens")]
        total_tokens: Option<u32>,
        #[serde(rename = "cacheCreationInputTokens")]
        cache_creation_input_tokens: Option<u32>,
        #[serde(rename = "cacheReadInputTokens")]
        cache_read_input_tokens: Option<u32>,
        #[serde(rename = "cachedInputTokens")]
        cached_input_tokens: Option<u32>,
        #[serde(rename = "responsePayload")]
        response_payload: Option<String>,
    },
    RequestConfigSync,
    Pong {
        timestamp: u64,
    },
}

struct FrameCodec;

impl FrameCodec {
    fn decode(buf: &[u8]) -> Result<Option<(Vec<u8>, usize)>, String> {
        if buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if len > MAX_FRAME_LENGTH {
            return Err(format!("frame too large: {len} bytes"));
        }
        if buf.len() < 4 + len {
            return Ok(None);
        }
        Ok(Some((buf[4..4 + len].to_vec(), 4 + len)))
    }

    fn encode(payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(4 + payload.len());
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }
}

pub struct IpcSender {
    tx: mpsc::UnboundedSender<RustToTsMessage>,
}

impl IpcSender {
    pub fn send(&self, msg: RustToTsMessage) {
        if let Err(e) = self.tx.send(msg) {
            warn!("Failed to queue IPC message: {e}");
        }
    }
}

pub struct IpcBridge {
    pub sender: IpcSender,
    pub config_rx: mpsc::UnboundedReceiver<SyncConfigPayload>,
    #[expect(dead_code)]
    pub reload_rx: mpsc::UnboundedReceiver<()>,
}

impl IpcBridge {
    pub async fn start() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let socket_path =
            std::env::var("LRS_IPC_SOCKET").unwrap_or_else(|_| IPC_SOCKET_PATH.to_string());
        let path = PathBuf::from(&socket_path);

        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let listener = UnixListener::bind(&path)?;
        info!("IPC listening on {socket_path}");

        let (global_tx, mut global_rx) = mpsc::unbounded_channel::<RustToTsMessage>();
        let (config_tx, config_rx) = mpsc::unbounded_channel::<SyncConfigPayload>();
        let (reload_tx, reload_rx) = mpsc::unbounded_channel::<()>();

        let active_writers: Arc<Mutex<Vec<mpsc::UnboundedSender<RustToTsMessage>>>> =
            Arc::new(Mutex::new(Vec::new()));

        // Accept TS connections — spawn reader/writer per connection.
        // Wrapped in catch_unwind to recover from panics in the accept loop.
        tokio::spawn({
            let config_tx = config_tx.clone();
            let reload_tx = reload_tx.clone();
            let active_writers = active_writers.clone();
            async move {
                loop {
                    let result = std::panic::AssertUnwindSafe(async {
                        loop {
                            match listener.accept().await {
                                Ok((stream, _)) => {
                                    info!("TS IPC connected");
                                    let (reader, writer) = stream.into_split();
                                    let (write_tx, write_rx) = mpsc::unbounded_channel();

                                    active_writers.lock().await.push(write_tx);

                                    tokio::spawn(read_frames(
                                        reader,
                                        config_tx.clone(),
                                        reload_tx.clone(),
                                    ));
                                    tokio::spawn(write_frames(writer, write_rx));
                                }
                                Err(e) => {
                                    error!("IPC accept error: {e}");
                                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                                }
                            }
                        }
                    })
                    .catch_unwind()
                    .await;

                    match result {
                        Ok(()) => return,
                        Err(_panic) => {
                            error!("IPC accept loop panicked, restarting in 1s");
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        }
                    }
                }
            }
        });

        // Forward global messages → all active connections
        tokio::spawn(async move {
            while let Some(msg) = global_rx.recv().await {
                let mut writers = active_writers.lock().await;
                // Keep only writers whose channels are still open
                writers.retain(|tx| !tx.is_closed());
                if writers.is_empty() {
                    warn!("IPC message dropped: no active TS connections");
                    continue;
                }
                for tx in writers.iter() {
                    if tx.send(msg.clone()).is_err() {
                        // Channel closed, will be cleaned up on next iteration
                    }
                }
            }
        });

        Ok(IpcBridge { sender: IpcSender { tx: global_tx }, config_rx, reload_rx })
    }
}

async fn read_frames(
    mut reader: impl AsyncRead + Unpin,
    config_tx: mpsc::UnboundedSender<SyncConfigPayload>,
    reload_tx: mpsc::UnboundedSender<()>,
) {
    let mut buf = Vec::with_capacity(65536);
    let mut tmp = vec![0u8; 65536];

    loop {
        match reader.read(&mut tmp).await {
            Ok(0) => {
                info!("TS IPC disconnected");
                return;
            }
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                loop {
                    match FrameCodec::decode(&buf) {
                        Ok(Some((payload, consumed))) => {
                            buf.drain(..consumed);
                            match serde_json::from_slice::<TsToRustMessage>(&payload) {
                                Ok(msg) => match msg {
                                    TsToRustMessage::SyncConfig { payload } => {
                                        info!(
                                            "Config sync: {} providers, {} aliases, {} keys",
                                            payload.providers.len(),
                                            payload.aliases.len(),
                                            payload.api_keys.len(),
                                        );
                                        let _ = config_tx.send(*payload);
                                    }
                                    TsToRustMessage::ReloadConfig => {
                                        info!("TS requested config reload");
                                        let _ = reload_tx.send(());
                                    }
                                    TsToRustMessage::Ping { .. } => {}
                                },
                                Err(e) => warn!("Bad IPC frame: {e}"),
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            warn!("Frame decode error: {e}");
                            buf.clear();
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                error!("IPC read error: {e}");
                return;
            }
        }
    }
}

async fn write_frames(
    mut writer: impl AsyncWrite + Unpin,
    mut rx: mpsc::UnboundedReceiver<RustToTsMessage>,
) {
    while let Some(msg) = rx.recv().await {
        match serde_json::to_vec(&msg) {
            Ok(payload) => {
                let frame = FrameCodec::encode(&payload);
                if let Err(e) = writer.write_all(&frame).await {
                    error!("IPC write error: {e}");
                    return;
                }
            }
            Err(e) => warn!("Serialize error: {e}"),
        }
    }
}
