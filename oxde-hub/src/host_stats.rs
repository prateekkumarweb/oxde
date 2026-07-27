use oxde_proto::hub::v1::{self, host_stats_result};
use serde::Serialize;
use ts_rs::TS;

use crate::error::{AppError, AppResult};

#[derive(Serialize, Clone, TS)]
#[ts(export)]
pub struct HostStats {
    pub cpu_percent: f32,
    pub cpu_per_core_percent: Vec<f32>,
    pub memory_usage_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_usage_bytes: u64,
    pub disk_total_bytes: u64,
}

impl From<v1::HostStats> for HostStats {
    fn from(stats: v1::HostStats) -> Self {
        Self {
            cpu_percent: stats.cpu_percent,
            cpu_per_core_percent: stats.cpu_per_core_percent,
            memory_usage_bytes: stats.memory_usage_bytes,
            memory_total_bytes: stats.memory_total_bytes,
            disk_usage_bytes: stats.disk_usage_bytes,
            disk_total_bytes: stats.disk_total_bytes,
        }
    }
}

pub async fn collect(agent_link: &crate::agent_link::AgentLink) -> AppResult<HostStats> {
    let result = agent_link.get_host_stats().await?;
    match result.result {
        Some(host_stats_result::Result::Ok(stats)) => Ok(stats.into()),
        Some(host_stats_result::Result::Error(message)) => Err(AppError::AgentError(message)),
        None => Err(AppError::AgentUnavailable),
    }
}
