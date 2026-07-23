use std::path::Path;

use serde::Serialize;
use sysinfo::{Disks, System};
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

/// Runs on a blocking thread: CPU% needs two samples a fixed interval apart.
pub async fn collect(data_dir: &Path) -> AppResult<HostStats> {
    let data_dir = data_dir.to_owned();
    let stats = tokio::task::spawn_blocking(move || collect_blocking(&data_dir))
        .await
        .map_err(|err| AppError::Io(std::io::Error::other(err)))?;
    Ok(stats)
}

fn collect_blocking(data_dir: &Path) -> HostStats {
    let mut sys = System::new_all();
    sys.refresh_cpu_usage();
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let (disk_usage_bytes, disk_total_bytes) = disk_usage(data_dir);
    let cpu_per_core_percent = sys.cpus().iter().map(sysinfo::Cpu::cpu_usage).collect();

    HostStats {
        cpu_percent: sys.global_cpu_usage(),
        cpu_per_core_percent,
        memory_usage_bytes: sys.used_memory(),
        memory_total_bytes: sys.total_memory(),
        disk_usage_bytes,
        disk_total_bytes,
    }
}

/// Finds the disk whose mount point is the longest prefix of `data_dir` -
/// the filesystem actually backing it, not just the first disk listed.
fn disk_usage(data_dir: &Path) -> (u64, u64) {
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|disk| data_dir.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map_or((0, 0), |disk| {
            let total = disk.total_space();
            let available = disk.available_space();
            (total.saturating_sub(available), total)
        })
}
