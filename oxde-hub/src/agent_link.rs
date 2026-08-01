use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::{Stream, StreamExt};
use oxde_proto::hub::v1::{
    GetHostStatsRequest, HostStatsResult, SessionRequest, SessionResponse, session_request,
    session_response,
};
use papaya::HashMap as ConcurrentHashMap;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::error::{AppError, AppResult};

const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound on a streaming reply's channel - the agent is expected to keep
/// pace with the hub draining it, not buffer unboundedly ahead.
const STREAM_CHANNEL_CAPACITY: usize = 16;

/// One agent's live `Session` connection. Cloneable (cheap, `Arc`-backed);
/// every clone shares the same connection state.
#[derive(Clone)]
pub struct AgentLink {
    inner: Arc<Inner>,
}

struct Inner {
    next_request_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<session_request::Payload>>>,
    streams: Mutex<HashMap<u64, mpsc::Sender<session_request::Payload>>>,
    outbound: Mutex<Option<mpsc::Sender<SessionResponse>>>,
}

impl AgentLink {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                next_request_id: AtomicU64::new(1),
                pending: Mutex::new(HashMap::new()),
                streams: Mutex::new(HashMap::new()),
                outbound: Mutex::new(None),
            }),
        }
    }

    pub async fn set_outbound(&self, sender: mpsc::Sender<SessionResponse>) {
        *self.inner.outbound.lock().await = Some(sender);
    }

    /// Routes a `SessionRequest` reply to its matching caller: a stream
    /// (`call_streaming_reply`) keeps taking messages until `end_stream`; a
    /// one-shot (`call`/`call_chunked`) takes exactly one. No match (timed
    /// out or unknown id) just drops it.
    pub async fn resolve(&self, request: SessionRequest) {
        let Some(payload) = request.payload else {
            return;
        };

        let stream_sender = self
            .inner
            .streams
            .lock()
            .await
            .get(&request.request_id)
            .cloned();
        if let Some(sender) = stream_sender {
            drop(sender.send(payload).await);
            return;
        }

        let sender = self.inner.pending.lock().await.remove(&request.request_id);
        if let Some(sender) = sender {
            drop(sender.send(payload));
        }
    }

    /// Removes a stream registered by `call_streaming_reply` - callers do
    /// this once they've seen the reply that marks the stream as finished,
    /// so `resolve` stops holding a channel open for it.
    pub async fn end_stream(&self, request_id: u64) {
        self.inner.streams.lock().await.remove(&request_id);
    }

    fn next_request_id(&self) -> u64 {
        self.inner.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn outbound(&self, request_id: u64) -> AppResult<mpsc::Sender<SessionResponse>> {
        let outbound = self.inner.outbound.lock().await.clone();
        if let Some(outbound) = outbound {
            Ok(outbound)
        } else {
            self.inner.pending.lock().await.remove(&request_id);
            self.inner.streams.lock().await.remove(&request_id);
            Err(AppError::AgentUnavailable)
        }
    }

    async fn send(
        &self,
        outbound: &mpsc::Sender<SessionResponse>,
        request_id: u64,
        payload: session_response::Payload,
    ) -> AppResult<()> {
        let sent = outbound
            .send(SessionResponse {
                request_id,
                payload: Some(payload),
            })
            .await;
        if sent.is_err() {
            self.inner.pending.lock().await.remove(&request_id);
            self.inner.streams.lock().await.remove(&request_id);
            return Err(AppError::AgentUnavailable);
        }
        Ok(())
    }

    async fn call(
        &self,
        payload: session_response::Payload,
    ) -> AppResult<session_request::Payload> {
        self.call_chunked(vec![payload]).await
    }

    /// Sends every payload in `payloads` under one `request_id` before
    /// awaiting a single reply - "hub sends N, agent replies once" (e.g. a
    /// handful of directory-lifecycle ops). Doesn't inspect payload
    /// contents.
    pub async fn call_chunked(
        &self,
        payloads: Vec<session_response::Payload>,
    ) -> AppResult<session_request::Payload> {
        self.call_streamed(futures_util::stream::iter(payloads), CALL_TIMEOUT)
            .await
    }

    /// Same as `call_chunked`, but takes a `Stream` instead of a `Vec` and
    /// a caller-chosen timeout - for transfers too large to buffer as one
    /// `Vec` up front (a multi-hundred-MB upload) and too slow for
    /// `CALL_TIMEOUT`, which is sized for quick RPCs.
    pub async fn call_streamed(
        &self,
        payloads: impl Stream<Item = session_response::Payload>,
        timeout: Duration,
    ) -> AppResult<session_request::Payload> {
        let mut payloads = std::pin::pin!(payloads);
        let request_id = self.next_request_id();
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(request_id, tx);

        let outbound = self.outbound(request_id).await?;
        while let Some(payload) = payloads.next().await {
            self.send(&outbound, request_id, payload).await?;
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(payload)) => Ok(payload),
            Ok(Err(_)) => Err(AppError::AgentUnavailable),
            Err(_) => {
                self.inner.pending.lock().await.remove(&request_id);
                Err(AppError::AgentTimeout)
            }
        }
    }

    /// Sends one payload, then returns the `request_id` (pass to
    /// `end_stream` when done) and a receiver fed by every reply - "hub
    /// sends one, agent replies with N" (e.g. streamed logs). No timeout: a
    /// slow-but-alive stream is expected here, not an error.
    pub async fn call_streaming_reply(
        &self,
        payload: session_response::Payload,
    ) -> AppResult<(u64, mpsc::Receiver<session_request::Payload>)> {
        let request_id = self.next_request_id();
        let (tx, rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        self.inner.streams.lock().await.insert(request_id, tx);

        let outbound = self.outbound(request_id).await?;
        self.send(&outbound, request_id, payload).await?;
        Ok((request_id, rx))
    }

    pub async fn get_host_stats(&self) -> AppResult<HostStatsResult> {
        let payload = self
            .call(session_response::Payload::GetHostStats(
                GetHostStatsRequest {},
            ))
            .await?;
        let session_request::Payload::HostStatsResult(result) = payload else {
            return Err(AppError::AgentError(
                "agent replied to GetHostStats with the wrong payload type".to_string(),
            ));
        };
        Ok(result)
    }
}

impl Default for AgentLink {
    fn default() -> Self {
        Self::new()
    }
}

/// Every connected agent's `AgentLink`, keyed by `Host.id`. Cloneable
/// (cheap, `Arc`-backed), shared via `AppState`.
#[derive(Clone, Default)]
pub struct AgentRegistry {
    hosts: Arc<ConcurrentHashMap<i64, AgentLink>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a fresh `AgentLink` for `host_id`, unless one is already
    /// connected - `None` means the caller should reject this connection
    /// rather than silently taking over an already-live one.
    pub fn connect(&self, host_id: i64) -> Option<AgentLink> {
        let link = AgentLink::new();
        self.hosts
            .pin()
            .try_insert(host_id, link.clone())
            .ok()
            .map(|_| link)
    }

    pub fn disconnect(&self, host_id: i64) {
        self.hosts.pin().remove(&host_id);
    }

    pub fn is_connected(&self, host_id: i64) -> bool {
        self.hosts.pin().contains_key(&host_id)
    }

    /// `App.host_id` resolved to a link - a disconnected host falls back
    /// to the same stand-in `any()` returns for "nothing connected".
    pub fn for_host(&self, host_id: i64) -> AgentLink {
        self.hosts
            .pin()
            .get(&host_id)
            .cloned()
            .unwrap_or_else(AgentLink::new)
    }

    /// A snapshot of every currently-connected `(host_id, AgentLink)` pair,
    /// for operations that must reach every connected agent rather than
    /// one app's (e.g. the orphan sweep).
    pub fn connected(&self) -> Vec<(i64, AgentLink)> {
        self.hosts
            .pin()
            .iter()
            .map(|(id, link)| (*id, link.clone()))
            .collect()
    }
}
