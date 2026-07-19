use std::{collections::HashMap, path::Path, time::Duration};

use bollard::{
    Docker,
    container::LogOutput,
    errors::Error as BollardError,
    models::{
        ContainerCpuStats, ContainerCreateBody, ContainerMemoryStats, EndpointSettings, HostConfig,
        NetworkCreateRequest, NetworkingConfig, RestartPolicy, RestartPolicyNameEnum,
    },
    query_parameters::{
        CreateContainerOptionsBuilder, CreateImageOptionsBuilder, LogsOptionsBuilder,
        RemoveContainerOptionsBuilder, StatsOptionsBuilder, StopContainerOptionsBuilder,
        WaitContainerOptionsBuilder,
    },
};
use bytes::Bytes;
use futures_util::{Stream, TryStreamExt};
use serde::Serialize;

use crate::{
    error::{AppError, AppResult},
    models::RunConfig,
};

/// Every run-mode container joins this network - `host_routing.rs` dials a
/// container's IP on it directly instead of publishing host ports.
pub const NETWORK_NAME: &str = "oxde-run";

const DEFAULT_MEMORY_BYTES: i64 = 512 * 1024 * 1024;
const DEFAULT_NANO_CPUS: i64 = 1_000_000_000; // 1 vCPU

/// Deterministic, so startup reconciliation can look containers up by name.
pub fn container_name(app_name: &str, deployment_id: &str) -> String {
    format!("oxde-{app_name}-{deployment_id}")
}

pub fn connect() -> AppResult<Docker> {
    Docker::connect_with_podman_defaults().map_err(|err| unavailable(&err))
}

fn unavailable(err: &BollardError) -> AppError {
    AppError::ContainerUnavailable(err.to_string())
}

fn start_failed(err: &BollardError) -> AppError {
    AppError::ContainerStartFailed(err.to_string())
}

/// Idempotent, including under concurrent callers: a losing racer's
/// `create_network` gets a 409 from Docker/Podman itself (someone else just
/// created it), which counts as success rather than an error.
pub async fn ensure_network(docker: &Docker) -> AppResult<()> {
    match docker.inspect_network(NETWORK_NAME, None).await {
        Ok(_) => Ok(()),
        Err(BollardError::DockerResponseServerError {
            status_code: 404, ..
        }) => match docker
            .create_network(NetworkCreateRequest {
                name: NETWORK_NAME.to_string(),
                ..Default::default()
            })
            .await
        {
            Ok(_)
            | Err(BollardError::DockerResponseServerError {
                status_code: 409, ..
            }) => Ok(()),
            Err(err) => Err(unavailable(&err)),
        },
        Err(err) => Err(unavailable(&err)),
    }
}

async fn ensure_image(docker: &Docker, image: &str) -> AppResult<()> {
    if docker.inspect_image(image).await.is_ok() {
        return Ok(());
    }
    let options = CreateImageOptionsBuilder::new().from_image(image).build();
    docker
        .create_image(Some(options), None, None)
        .try_for_each(|_| async { Ok(()) })
        .await
        .map_err(|err| start_failed(&err))?;
    Ok(())
}

fn networking_config() -> NetworkingConfig {
    let mut endpoints = HashMap::new();
    endpoints.insert(NETWORK_NAME.to_string(), EndpointSettings::default());
    NetworkingConfig {
        endpoints_config: Some(endpoints),
    }
}

fn bind_mount(checkout_dir: &Path) -> String {
    format!("{}:/app", checkout_dir.display())
}

async fn container_exists(docker: &Docker, name: &str) -> AppResult<bool> {
    match docker.inspect_container(name, None).await {
        Ok(_) => Ok(true),
        Err(BollardError::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(false),
        Err(err) => Err(unavailable(&err)),
    }
}

/// Idempotent: a container with `name` already existing (running or merely
/// stopped) is started/left alone rather than recreated, since the same
/// deterministic name always means the same deployment and config - this
/// matters for startup reconciliation, where the container may already be
/// running or may have survived a restart in a stopped state. Runs
/// `install_command` (if any) to completion first when a fresh create is
/// needed; on any failure there, nothing named `name` is left running.
pub async fn start(
    docker: &Docker,
    name: &str,
    checkout_dir: &Path,
    config: &RunConfig,
    install_timeout: Duration,
) -> AppResult<()> {
    if is_running(docker, name).await? {
        return Ok(());
    }
    if container_exists(docker, name).await? {
        return docker
            .start_container(name, None)
            .await
            .map_err(|err| start_failed(&err));
    }

    let image = config.image.image_tag();
    ensure_image(docker, image).await?;

    if let Some(install_command) = &config.install_command {
        run_install_command(
            docker,
            name,
            checkout_dir,
            image,
            install_command,
            install_timeout,
        )
        .await?;
    }

    let host_config = HostConfig {
        binds: Some(vec![bind_mount(checkout_dir)]),
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
            maximum_retry_count: None,
        }),
        network_mode: Some(NETWORK_NAME.to_string()),
        memory: Some(DEFAULT_MEMORY_BYTES),
        nano_cpus: Some(DEFAULT_NANO_CPUS),
        ..Default::default()
    };

    let body = ContainerCreateBody {
        image: Some(image.to_string()),
        working_dir: Some("/app".to_string()),
        cmd: Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            config.start_command.clone(),
        ]),
        env: Some(vec![format!("PORT={}", config.container_port)]),
        host_config: Some(host_config),
        networking_config: Some(networking_config()),
        ..Default::default()
    };

    let options = CreateContainerOptionsBuilder::new().name(name).build();
    docker
        .create_container(Some(options), body)
        .await
        .map_err(|err| start_failed(&err))?;
    docker
        .start_container(name, None)
        .await
        .map_err(|err| start_failed(&err))
}

/// Exposed so the logs endpoint can stream from it before `parent_name`
/// itself exists.
pub fn install_container_name(parent_name: &str) -> String {
    format!("{parent_name}-install")
}

/// Doesn't remove the container immediately on exit - `schedule_cleanup`
/// gives an attached log-streaming client a grace period to finish reading.
async fn run_install_command(
    docker: &Docker,
    parent_name: &str,
    checkout_dir: &Path,
    image: &str,
    install_command: &str,
    timeout: Duration,
) -> AppResult<()> {
    let installer_name = install_container_name(parent_name);
    let body = ContainerCreateBody {
        image: Some(image.to_string()),
        working_dir: Some("/app".to_string()),
        cmd: Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            install_command.to_string(),
        ]),
        host_config: Some(HostConfig {
            binds: Some(vec![bind_mount(checkout_dir)]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let options = CreateContainerOptionsBuilder::new()
        .name(&installer_name)
        .build();
    docker
        .create_container(Some(options), body)
        .await
        .map_err(|err| start_failed(&err))?;
    docker
        .start_container(&installer_name, None)
        .await
        .map_err(|err| start_failed(&err))?;

    let wait_options = WaitContainerOptionsBuilder::new().build();
    let mut wait_stream = docker.wait_container(&installer_name, Some(wait_options));
    let wait_result = tokio::time::timeout(timeout, wait_stream.try_next()).await;

    let result = match wait_result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(BollardError::DockerContainerWaitError { error, code })) => Err(
            AppError::ContainerStartFailed(format!("install_command exited {code}: {error}")),
        ),
        Ok(Err(err)) => Err(start_failed(&err)),
        Err(_) => {
            // Stop it explicitly on timeout rather than leaving it running
            // for the whole grace period below.
            docker.stop_container(&installer_name, None).await.ok();
            Err(AppError::ContainerStartFailed(format!(
                "install_command timed out after {}s",
                timeout.as_secs()
            )))
        }
    };

    schedule_cleanup(docker.clone(), installer_name);
    result
}

const INSTALL_CONTAINER_CLEANUP_GRACE: Duration = Duration::from_secs(30);

fn schedule_cleanup(docker: Docker, name: String) {
    tokio::spawn(async move {
        tokio::time::sleep(INSTALL_CONTAINER_CLEANUP_GRACE).await;
        let remove_options = RemoveContainerOptionsBuilder::new().force(true).build();
        docker
            .remove_container(&name, Some(remove_options))
            .await
            .ok();
    });
}

/// Missing (already gone) counts as success.
pub async fn stop_and_remove(docker: &Docker, name: &str) -> AppResult<()> {
    let stop_options = StopContainerOptionsBuilder::new().build();
    match docker.stop_container(name, Some(stop_options)).await {
        Ok(()) => {}
        Err(BollardError::DockerResponseServerError {
            status_code: 404, ..
        }) => return Ok(()),
        Err(err) => return Err(unavailable(&err)),
    }

    let remove_options = RemoveContainerOptionsBuilder::new().force(true).build();
    match docker.remove_container(name, Some(remove_options)).await {
        Ok(())
        | Err(BollardError::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(()),
        Err(err) => Err(unavailable(&err)),
    }
}

pub async fn container_ip(docker: &Docker, name: &str) -> AppResult<Option<String>> {
    let inspect = docker
        .inspect_container(name, None)
        .await
        .map_err(|err| unavailable(&err))?;
    let ip = inspect
        .network_settings
        .and_then(|settings| settings.networks)
        .and_then(|mut networks| networks.remove(NETWORK_NAME))
        .and_then(|endpoint| endpoint.ip_address)
        .filter(|ip| !ip.is_empty());
    Ok(ip)
}

const TAIL_LINES: &str = "256";

/// `follow = false` returns the last `TAIL_LINES` lines and ends;
/// `follow = true` returns the same backlog, then keeps the stream open,
/// yielding new lines as the container produces them, until the caller
/// drops it.
pub fn logs(
    docker: &Docker,
    name: &str,
    follow: bool,
) -> impl Stream<Item = AppResult<Bytes>> + use<> {
    let options = LogsOptionsBuilder::new()
        .follow(follow)
        .stdout(true)
        .stderr(true)
        .tail(TAIL_LINES)
        .build();
    docker
        .logs(name, Some(options))
        .map_ok(log_output_bytes)
        .map_err(|err| unavailable(&err))
}

fn log_output_bytes(output: LogOutput) -> Bytes {
    match output {
        LogOutput::StdErr { message }
        | LogOutput::StdOut { message }
        | LogOutput::StdIn { message }
        | LogOutput::Console { message } => message,
    }
}

#[derive(Serialize, Clone, Copy)]
pub struct ContainerStats {
    pub cpu_percent: f64,
    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,
}

/// `stream(false)`/`one_shot(false)`: a single request, but Podman still
/// waits to gather two samples internally so `cpu_stats`/`precpu_stats`
/// are both populated - needed for the CPU% delta below.
pub async fn stats(docker: &Docker, name: &str) -> AppResult<ContainerStats> {
    let options = StatsOptionsBuilder::new()
        .stream(false)
        .one_shot(false)
        .build();
    let response = docker
        .stats(name, Some(options))
        .try_next()
        .await
        .map_err(|err| unavailable(&err))?
        .ok_or_else(|| AppError::ContainerUnavailable(format!("no stats for {name}")))?;

    let cpu_percent = cpu_percent(response.cpu_stats.as_ref(), response.precpu_stats.as_ref());
    let (memory_usage_bytes, memory_limit_bytes) = memory_usage(response.memory_stats.as_ref());

    Ok(ContainerStats {
        cpu_percent,
        memory_usage_bytes,
        memory_limit_bytes,
    })
}

#[allow(clippy::cast_precision_loss)]
fn cpu_percent(
    cpu_stats: Option<&ContainerCpuStats>,
    precpu_stats: Option<&ContainerCpuStats>,
) -> f64 {
    let (Some(cpu_stats), Some(precpu_stats)) = (cpu_stats, precpu_stats) else {
        return 0.0;
    };
    let (Some(cpu_usage), Some(precpu_usage)) = (&cpu_stats.cpu_usage, &precpu_stats.cpu_usage)
    else {
        return 0.0;
    };
    let (Some(total), Some(pretotal), Some(system), Some(presystem), Some(online_cpus)) = (
        cpu_usage.total_usage,
        precpu_usage.total_usage,
        cpu_stats.system_cpu_usage,
        precpu_stats.system_cpu_usage,
        cpu_stats.online_cpus,
    ) else {
        return 0.0;
    };

    let cpu_delta = total.saturating_sub(pretotal) as f64;
    let system_delta = system.saturating_sub(presystem) as f64;
    if system_delta <= 0.0 {
        return 0.0;
    }
    (cpu_delta / system_delta) * f64::from(online_cpus) * 100.0
}

fn memory_usage(memory_stats: Option<&ContainerMemoryStats>) -> (u64, u64) {
    let Some(memory_stats) = memory_stats else {
        return (0, 0);
    };
    (
        memory_stats.usage.unwrap_or(0),
        memory_stats.limit.unwrap_or(0),
    )
}

pub async fn is_running(docker: &Docker, name: &str) -> AppResult<bool> {
    match docker.inspect_container(name, None).await {
        Ok(inspect) => Ok(inspect
            .state
            .and_then(|state| state.running)
            .unwrap_or(false)),
        Err(BollardError::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(false),
        Err(err) => Err(unavailable(&err)),
    }
}

/// Requires a real Podman socket - these fail (not skip) if one isn't
/// reachable, so a missing Podman shows up as a test failure.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod live_tests {
    use super::*;
    use crate::models::{RunConfig, RunImage};

    fn temp_checkout(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxde-containers-live-{label}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create checkout dir");
        dir
    }

    #[tokio::test]
    async fn network_bootstrap_is_idempotent() {
        let docker = connect().expect("connect to podman");
        ensure_network(&docker).await.expect("ensure_network 1");
        ensure_network(&docker).await.expect("ensure_network 2");
    }

    #[tokio::test]
    async fn container_lifecycle_start_ip_stop() {
        let docker = connect().expect("connect to podman");
        ensure_network(&docker).await.expect("ensure_network");
        let checkout = temp_checkout("lifecycle");
        let name = "oxde-live-test-lifecycle";
        stop_and_remove(&docker, name).await.ok();

        let config = RunConfig {
            image: RunImage::Node24,
            install_command: None,
            start_command:
                "node -e \"require('http').createServer((_, res) => res.end('ok')).listen(process.env.PORT)\""
                    .to_string(),
            container_port: 3000,
        };
        start(&docker, name, &checkout, &config, Duration::from_secs(60))
            .await
            .expect("start container");
        assert!(is_running(&docker, name).await.expect("is_running"));

        let ip = container_ip(&docker, name)
            .await
            .expect("container_ip")
            .expect("container has an ip");
        assert!(!ip.is_empty());

        // Confirms the host can dial a container IP directly rather than
        // needing published ports. Retries since the node process needs a
        // moment to start listening after `start_container` returns.
        let mut reachable = false;
        for _ in 0..10 {
            let addr = format!("{ip}:{}", config.container_port);
            if tokio::time::timeout(
                std::time::Duration::from_millis(500),
                tokio::net::TcpStream::connect(&addr),
            )
            .await
            .is_ok_and(|result| result.is_ok())
            {
                reachable = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        assert!(
            reachable,
            "host could not dial the container's IP directly on the shared network"
        );

        stop_and_remove(&docker, name)
            .await
            .expect("stop_and_remove");
        assert!(
            !is_running(&docker, name)
                .await
                .expect("is_running after stop")
        );
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[tokio::test]
    async fn install_command_failure_leaves_nothing_running() {
        let docker = connect().expect("connect to podman");
        ensure_network(&docker).await.expect("ensure_network");
        let checkout = temp_checkout("install-fail");
        let name = "oxde-live-test-install-fail";
        stop_and_remove(&docker, name).await.ok();

        let config = RunConfig {
            image: RunImage::Node24,
            install_command: Some("exit 1".to_string()),
            start_command: "node -e \"1\"".to_string(),
            container_port: 3000,
        };
        let result = start(&docker, name, &checkout, &config, Duration::from_secs(60)).await;
        assert!(result.is_err());
        assert!(!is_running(&docker, name).await.expect("is_running"));
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[tokio::test]
    async fn install_command_timeout_leaves_nothing_running() {
        let docker = connect().expect("connect to podman");
        ensure_network(&docker).await.expect("ensure_network");
        let checkout = temp_checkout("install-timeout");
        let name = "oxde-live-test-install-timeout";
        stop_and_remove(&docker, name).await.ok();

        let config = RunConfig {
            image: RunImage::Node24,
            install_command: Some("sleep 5".to_string()),
            start_command: "node -e \"1\"".to_string(),
            container_port: 3000,
        };
        let result = start(
            &docker,
            name,
            &checkout,
            &config,
            Duration::from_millis(200),
        )
        .await;
        assert!(matches!(result, Err(AppError::ContainerStartFailed(_))));
        assert!(!is_running(&docker, name).await.expect("is_running"));
        std::fs::remove_dir_all(&checkout).ok();
    }
}
