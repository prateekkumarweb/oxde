use std::{path::Path, time::Duration};

use oxde_proto::hub::v1::{
    AgentErrorKind, Chunk, CreateDeploymentDirRequest, DeleteDeploymentDirRequest,
    ListDeploymentDirsRequest, UploadZipAndExtractRequest, command_result,
    list_deployment_dirs_result, session_request, session_response, upload_zip_and_extract_result,
};
use tokio::io::AsyncReadExt;

use crate::{
    agent_link::AgentLink,
    error::{AppError, AppResult},
};

const UPLOAD_CHUNK_BYTES: usize = 64 * 1024;
/// Generous relative to `CALL_TIMEOUT`: a multi-hundred-MB transfer plus
/// agent-side extraction genuinely takes longer than a quick RPC.
const UPLOAD_TIMEOUT: Duration = Duration::from_mins(5);

fn fs_agent_error(err: oxde_proto::hub::v1::AgentError) -> AppError {
    match AgentErrorKind::try_from(err.kind).unwrap_or(AgentErrorKind::Unspecified) {
        AgentErrorKind::CommandFailed => AppError::CommandFailed(err.message),
        AgentErrorKind::StartFailed | AgentErrorKind::Unavailable | AgentErrorKind::Unspecified => {
            AppError::AgentOperationFailed(err.message)
        }
    }
}

fn command_result_to_app_result(result: oxde_proto::hub::v1::CommandResult) -> AppResult<()> {
    match result.result {
        Some(command_result::Result::Ok(_)) => Ok(()),
        Some(command_result::Result::Error(err)) => Err(fs_agent_error(err)),
        None => Err(AppError::AgentError(
            "agent sent an empty command result".to_string(),
        )),
    }
}

pub async fn create_deployment_dir(agent_link: &AgentLink, deployment_id: &str) -> AppResult<()> {
    let payload = session_response::Payload::CreateDeploymentDir(CreateDeploymentDirRequest {
        deployment_id: deployment_id.to_string(),
    });
    let reply = agent_link.call_chunked(vec![payload]).await?;
    let session_request::Payload::CreateDeploymentDirResult(result) = reply else {
        return Err(AppError::AgentError(
            "agent replied to CreateDeploymentDir with the wrong payload type".to_string(),
        ));
    };
    command_result_to_app_result(result)
}

/// Missing (already gone) counts as success.
pub async fn delete_deployment_dir(agent_link: &AgentLink, deployment_id: &str) -> AppResult<()> {
    let payload = session_response::Payload::DeleteDeploymentDir(DeleteDeploymentDirRequest {
        deployment_id: deployment_id.to_string(),
    });
    let reply = agent_link.call_chunked(vec![payload]).await?;
    let session_request::Payload::DeleteDeploymentDirResult(result) = reply else {
        return Err(AppError::AgentError(
            "agent replied to DeleteDeploymentDir with the wrong payload type".to_string(),
        ));
    };
    command_result_to_app_result(result)
}

pub async fn list_deployment_dirs(agent_link: &AgentLink) -> AppResult<Vec<String>> {
    let payload = session_response::Payload::ListDeploymentDirs(ListDeploymentDirsRequest {});
    let reply = agent_link.call_chunked(vec![payload]).await?;
    let session_request::Payload::ListDeploymentDirsResult(result) = reply else {
        return Err(AppError::AgentError(
            "agent replied to ListDeploymentDirs with the wrong payload type".to_string(),
        ));
    };
    match result.result {
        Some(list_deployment_dirs_result::Result::Ok(list)) => Ok(list.deployment_ids),
        Some(list_deployment_dirs_result::Result::Error(err)) => Err(fs_agent_error(err)),
        None => Err(AppError::AgentError(
            "agent sent an empty ListDeploymentDirs result".to_string(),
        )),
    }
}

enum UploadState {
    Reading(tokio::fs::File),
    Finished,
}

/// Streams `zip_path` to the agent in fixed-size chunks, never buffering
/// the whole file in memory. Reused for both a browser upload and a
/// run-mode deployment's `files/`, zipped locally first.
pub async fn upload_zip_and_extract(
    agent_link: &AgentLink,
    deployment_id: &str,
    zip_path: &Path,
    max_uncompressed_bytes: u64,
) -> AppResult<u64> {
    let file = tokio::fs::File::open(zip_path).await?;
    let deployment_id = deployment_id.to_string();

    let payloads = futures_util::stream::unfold(UploadState::Reading(file), move |state| {
        let deployment_id = deployment_id.clone();
        async move {
            let UploadState::Reading(mut file) = state else {
                return None;
            };
            let mut buf = vec![0u8; UPLOAD_CHUNK_BYTES];
            match file.read(&mut buf).await {
                Ok(0) => {
                    let payload =
                        wrap_chunk(deployment_id, max_uncompressed_bytes, Vec::new(), true);
                    Some((payload, UploadState::Finished))
                }
                Ok(n) => {
                    buf.truncate(n);
                    let payload = wrap_chunk(deployment_id, max_uncompressed_bytes, buf, false);
                    Some((payload, UploadState::Reading(file)))
                }
                Err(_) => None,
            }
        }
    });

    let reply = agent_link.call_streamed(payloads, UPLOAD_TIMEOUT).await?;
    let session_request::Payload::UploadZipAndExtractResult(result) = reply else {
        return Err(AppError::AgentError(
            "agent replied to UploadZipAndExtract with the wrong payload type".to_string(),
        ));
    };
    match result.result {
        Some(upload_zip_and_extract_result::Result::ContentSizeBytes(size)) => Ok(size),
        Some(upload_zip_and_extract_result::Result::Error(err)) => Err(fs_agent_error(err)),
        None => Err(AppError::AgentError(
            "agent sent an empty UploadZipAndExtract result".to_string(),
        )),
    }
}

/// Zips `source_dir`'s contents (relative paths, no leading directory
/// entry for `source_dir` itself) into `zip_path` - used to ship a
/// run-mode deployment's already-resolved `files/` to the agent, reusing
/// `upload_zip_and_extract` rather than a second wire format.
pub fn zip_dir(source_dir: &Path, zip_path: &Path) -> AppResult<()> {
    let file = std::fs::File::create(zip_path)?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    add_dir_to_zip(&mut writer, source_dir, source_dir, options)?;
    writer.finish()?;
    Ok(())
}

fn add_dir_to_zip(
    writer: &mut zip::ZipWriter<std::fs::File>,
    base: &Path,
    dir: &Path,
    options: zip::write::SimpleFileOptions,
) -> AppResult<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // Always succeeds: `path` comes from `read_dir(dir)`, and `dir` is
        // always `base` or one of its descendants.
        let Ok(relative) = path.strip_prefix(base) else {
            continue;
        };
        let relative = relative.to_string_lossy();
        if path.is_dir() {
            writer.add_directory(relative, options)?;
            add_dir_to_zip(writer, base, &path, options)?;
        } else {
            writer.start_file(relative, options)?;
            std::io::copy(&mut std::fs::File::open(&path)?, writer)?;
        }
    }
    Ok(())
}

const fn wrap_chunk(
    deployment_id: String,
    max_uncompressed_bytes: u64,
    data: Vec<u8>,
    is_final: bool,
) -> session_response::Payload {
    session_response::Payload::UploadZipAndExtract(UploadZipAndExtractRequest {
        deployment_id,
        max_uncompressed_bytes,
        chunk: Some(Chunk { data, is_final }),
    })
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn test_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxde-hub-test-agent-fs-{label}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn zip_dir_includes_nested_files_with_relative_paths() {
        let source = test_dir("zip-source");
        std::fs::create_dir_all(source.join("css")).expect("create css dir");
        std::fs::write(source.join("index.html"), b"<h1>hi</h1>").expect("write index.html");
        std::fs::write(source.join("css/site.css"), b"body {}").expect("write site.css");
        let zip_path = source.join("../zip-dir-out.zip");

        zip_dir(&source, &zip_path).expect("zip_dir");

        let file = std::fs::File::open(&zip_path).expect("open zip");
        let mut archive = zip::ZipArchive::new(file).expect("read archive");
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).expect("entry").name().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "css/".to_string(),
                "css/site.css".to_string(),
                "index.html".to_string(),
            ]
        );

        let mut contents = String::new();
        archive
            .by_name("index.html")
            .expect("index.html entry")
            .read_to_string(&mut contents)
            .expect("read index.html");
        assert_eq!(contents, "<h1>hi</h1>");

        std::fs::remove_dir_all(&source).ok();
        std::fs::remove_file(&zip_path).ok();
    }
}
