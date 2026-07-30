use std::path::Path;

use crate::layout;

pub fn create_deployment_dir(data_dir: &Path, deployment_id: &str) -> Result<(), String> {
    std::fs::create_dir_all(layout::deployment_dir(data_dir, deployment_id))
        .map_err(|err| err.to_string())
}

/// Missing (already gone) counts as success.
pub fn delete_deployment_dir(data_dir: &Path, deployment_id: &str) -> Result<(), String> {
    let dir = layout::deployment_dir(data_dir, deployment_id);
    // Otherwise a missing `tmp/` and a missing `dir` both fail `rename`
    // with `NotFound`, indistinguishable below from "already deleted".
    std::fs::create_dir_all(layout::tmp_dir(data_dir)).map_err(|err| err.to_string())?;
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

#[cfg(test)]
mod tests {
    use std::{io::Cursor, path::PathBuf, time::SystemTime};

    use super::*;

    fn test_data_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "oxde-agent-test-fs-ops-{label}-{}-{nanos}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).expect("create test data dir");
        dir
    }

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, contents) in entries {
            writer.start_file(*name, options).expect("start_file");
            std::io::Write::write_all(&mut writer, contents).expect("write contents");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    #[test]
    fn create_deployment_dir_creates_the_directory() {
        let data_dir = test_data_dir("create");
        create_deployment_dir(&data_dir, "dep-1").expect("create");
        assert!(layout::deployment_dir(&data_dir, "dep-1").is_dir());
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn delete_deployment_dir_on_missing_dir_is_success() {
        let data_dir = test_data_dir("delete-missing");
        delete_deployment_dir(&data_dir, "never-existed").expect("missing dir is success");
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn delete_deployment_dir_removes_an_existing_dir() {
        let data_dir = test_data_dir("delete-existing");
        create_deployment_dir(&data_dir, "dep-1").expect("create");
        delete_deployment_dir(&data_dir, "dep-1").expect("delete");
        assert!(!layout::deployment_dir(&data_dir, "dep-1").exists());
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn list_deployment_dirs_is_empty_when_deployments_dir_is_missing() {
        let data_dir = test_data_dir("list-missing");
        assert_eq!(
            list_deployment_dirs(&data_dir).expect("list"),
            Vec::<String>::new()
        );
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn list_deployment_dirs_lists_created_deployments() {
        let data_dir = test_data_dir("list-present");
        create_deployment_dir(&data_dir, "dep-a").expect("create a");
        create_deployment_dir(&data_dir, "dep-b").expect("create b");
        let mut ids = list_deployment_dirs(&data_dir).expect("list");
        ids.sort();
        assert_eq!(ids, vec!["dep-a".to_string(), "dep-b".to_string()]);
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn extract_and_place_moves_content_into_files_dir() {
        let data_dir = test_data_dir("extract-ok");
        // Real callers always create the deployment dir first.
        create_deployment_dir(&data_dir, "dep-1").expect("create deployment dir");
        let zip_path = data_dir.join("upload.zip");
        std::fs::write(&zip_path, build_zip(&[("index.html", b"hi")])).expect("write zip");

        let size = extract_and_place(&data_dir, "dep-1", &zip_path, 10_000).expect("extract");

        assert_eq!(size, 2);
        assert_eq!(
            std::fs::read_to_string(
                layout::deployment_files_dir(&data_dir, "dep-1").join("index.html")
            )
            .expect("read index.html"),
            "hi"
        );
        assert!(
            !layout::upload_staging_dir(&data_dir, "dep-1").exists(),
            "staging dir must not survive a successful extract"
        );
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn extract_and_place_leaves_no_content_on_a_rejected_archive() {
        let data_dir = test_data_dir("extract-reject");
        let zip_path = data_dir.join("upload.zip");
        std::fs::write(&zip_path, build_zip(&[("big.txt", &vec![b'a'; 1000])])).expect("write zip");

        let result = extract_and_place(&data_dir, "dep-1", &zip_path, 10);

        assert!(result.is_err(), "oversized archive must be rejected");
        assert!(!layout::deployment_files_dir(&data_dir, "dep-1").exists());
        assert!(!layout::upload_staging_dir(&data_dir, "dep-1").exists());
        std::fs::remove_dir_all(&data_dir).ok();
    }
}
