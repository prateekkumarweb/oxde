use std::path::Path;

use crate::layout;

pub fn create_deployment_dir(data_dir: &Path, deployment_id: &str) -> Result<(), String> {
    std::fs::create_dir_all(layout::deployment_dir(data_dir, deployment_id))
        .map_err(|err| err.to_string())
}

/// Missing (already gone) counts as success.
pub fn delete_deployment_dir(data_dir: &Path, deployment_id: &str) -> Result<(), String> {
    let dir = layout::deployment_dir(data_dir, deployment_id);
    let staging = layout::tmp_dir(data_dir).join(format!("deleted-deployment-{deployment_id}"));
    match std::fs::rename(&dir, &staging) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.to_string()),
    }
    std::fs::remove_dir_all(&staging).map_err(|err| err.to_string())
}

pub fn list_deployment_dirs(data_dir: &Path) -> Result<Vec<String>, String> {
    let entries = match std::fs::read_dir(layout::deployments_dir(data_dir)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.to_string()),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        if let Some(name) = entry.file_name().to_str() {
            ids.push(name.to_string());
        }
    }
    Ok(ids)
}

/// Extracts `zip_path` into a scratch dir, then renames that scratch dir
/// into place as `files/` - content is fully in its final location before
/// this returns, matching the "fs commits before the DB row" invariant
/// every create path in this system relies on.
pub fn extract_and_place(
    data_dir: &Path,
    deployment_id: &str,
    zip_path: &Path,
    max_uncompressed_bytes: u64,
) -> Result<u64, String> {
    let staging = layout::upload_staging_dir(data_dir, deployment_id);
    std::fs::remove_dir_all(&staging).ok();
    std::fs::create_dir_all(&staging).map_err(|err| err.to_string())?;

    let zip_file = std::fs::File::open(zip_path).map_err(|err| err.to_string())?;
    let size = crate::zip_extract::unpack_zip(zip_file, &staging, max_uncompressed_bytes)
        .inspect_err(|_| {
            std::fs::remove_dir_all(&staging).ok();
        })?;

    let files_dir = layout::deployment_files_dir(data_dir, deployment_id);
    std::fs::rename(&staging, &files_dir).map_err(|err| {
        std::fs::remove_dir_all(&staging).ok();
        err.to_string()
    })?;
    Ok(size)
}
