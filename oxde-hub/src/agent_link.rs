use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use oxde_proto::hub::v1::{
    GetHostStatsRequest, HostStatsResult, SessionRequest, SessionResponse, session_request,
    session_response,
};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::error::{AppError, AppResult};

const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Single-agent for now - `set_outbound` replaces whatever was there before,
/// matching Milestone A's one-host scope. Cloneable, shared via `AppState`.
#[derive(Clone)]
pub struct AgentLink {
    inner: Arc<Inner>,
}

struct Inner {
    next_request_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<session_request::Payload>>>,
    outbound: Mutex<Option<mpsc::Sender<SessionResponse>>>,
}

impl AgentLink {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                next_request_id: AtomicU64::new(1),
                pending: Mutex::new(HashMap::new()),
                outbound: Mutex::new(None),
            }),
        }
    }

    pub async fn set_outbound(&self, sender: mpsc::Sender<SessionResponse>) {
        *self.inner.outbound.lock().await = Some(sender);
    }

    pub async fn clear_outbound(&self) {
        *self.inner.outbound.lock().await = None;
    }

    /// Resolves the pending call a `SessionRequest` from the agent answers,
    /// if any is still waiting (a late reply past `CALL_TIMEOUT` has none).
    pub async fn resolve(&self, request: SessionRequest) {
        let Some(payload) = request.payload else {
            return;
        };
        let sender = self.inner.pending.lock().await.remove(&request.request_id);
        if let Some(sender) = sender {
            drop(sender.send(payload));
        }
    }

    async fn call(
        &self,
        payload: session_response::Payload,
    ) -> AppResult<session_request::Payload> {
        let request_id = self.inner.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(request_id, tx);

        let outbound = self.inner.outbound.lock().await.clone();
        let Some(outbound) = outbound else {
            self.inner.pending.lock().await.remove(&request_id);
            return Err(AppError::AgentUnavailable);
        };

        let sent = outbound
            .send(SessionResponse {
                request_id,
                payload: Some(payload),
            })
            .await;
        if sent.is_err() {
            self.inner.pending.lock().await.remove(&request_id);
            return Err(AppError::AgentUnavailable);
        }

        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(payload)) => Ok(payload),
            Ok(Err(_)) => Err(AppError::AgentUnavailable),
            Err(_) => {
                self.inner.pending.lock().await.remove(&request_id);
                Err(AppError::AgentTimeout)
            }
        }
    }

    pub async fn get_host_stats(&self) -> AppResult<HostStatsResult> {
        let payload = self
            .call(session_response::Payload::GetHostStats(
                GetHostStatsRequest {},
            ))
            .await?;
        let session_request::Payload::HostStatsResult(result) = payload;
        Ok(result)
    }
}

impl Default for AgentLink {
    fn default() -> Self {
        Self::new()
    }
}
