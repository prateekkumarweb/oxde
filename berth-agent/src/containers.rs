use std::{collections::HashMap, path::Path, time::Duration};

use berth_models::EnvVar;
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
use futures_util::{Stream, StreamExt, TryStreamExt};
use tokio::sync::mpsc;

fn env_strings(env_vars: &[EnvVar]) -> Vec<String> {
    env_vars
        .iter()
        .map(|env_var| format!("{}={}", env_var.key, env_var.value.as_str()))
        .collect()
}

/// Every run-mode container joins this network - the hub dials a
/// container's IP on it directly instead of publishing host ports.
pub const NETWORK_NAME: &str = "berth-run";

const DEFAULT_MEMORY_BYTES: i64 = 512 * 1024 * 1024;
const DEFAULT_NANO_CPUS: i64 = 1_000_000_000; // 1 vCPU

pub fn connect() -> anyhow::Result<Docker> {
    Docker::connect_with_podman_defaults().map_err(|err| anyhow::anyhow!(unavailable(&err)))
}

fn unavailable(err: &BollardError) -> String {
    err.to_string()
}

fn start_failed(err: &BollardError) -> String {
    err.to_string()
}

/// Idempotent, including under concurrent callers: a losing racer's
/// `create_network` gets a 409 from Docker/Podman itself (someone else just
/// created it), which counts as success rather than an error.
pub async fn ensure_network(docker: &Docker) -> anyhow::Result<()> {
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
            Err(err) => Err(anyhow::anyhow!(unavailable(&err))),
        },
        Err(err) => Err(anyhow::anyhow!(unavailable(&err))),
    }
}

async fn ensure_image(docker: &Docker, image: &str) -> Result<(), String> {
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

async fn container_exists(docker: &Docker, name: &str) -> Result<bool, String> {
    match docker.inspect_container(name, None).await {
        Ok(_) => Ok(true),
        Err(BollardError::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(false),
        Err(err) => Err(unavailable(&err)),
    }
}

/// Failure kind, so the caller (the RPC handler) can pick the right
/// `AgentErrorKind` without string-matching a message.
pub enum StartError {
    Unavailable(String),
    StartFailed(String),
    CommandFailed(String),
}

/// Bundles a run-mode container's config to stay under clippy's arg-count
/// lint.
pub struct RunContainerConfig<'a> {
    pub image: &'a str,
    pub install_command: Option<&'a str>,
    pub start_command: &'a str,
    pub env_vars: &'a [EnvVar],
    pub install_timeout: Duration,
}

/// Idempotent: an existing container (running or stopped) is left alone
/// rather than recreated - matters for startup reconciliation. Runs
/// `install_command` (if any) first; on failure, nothing named `name` is
/// left running. `log_sink` receives install output live, if given.
pub async fn start(
    docker: &Docker,
    name: &str,
    checkout_dir: &Path,
    config: RunContainerConfig<'_>,
    log_sink: Option<mpsc::Sender<Bytes>>,
) -> Result<(), StartError> {
    if is_running(docker, name)
        .await
        .map_err(StartError::Unavailable)?
    {
        return Ok(());
    }
    if container_exists(docker, name)
        .await
        .map_err(StartError::Unavailable)?
    {
        return docker
            .start_container(name, None)
            .await
            .map_err(|err| StartError::StartFailed(start_failed(&err)));
    }

    ensure_image(docker, config.image)
        .await
        .map_err(StartError::StartFailed)?;

    if let Some(install_command) = config.install_command {
        run_command_to_completion(
            docker,
            &install_container_name(name),
            checkout_dir,
            CommandExec {
                image: config.image,
                command: install_command,
                env_vars: config.env_vars,
                timeout: config.install_timeout,
                log_sink,
            },
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
        image: Some(config.image.to_string()),
        working_dir: Some("/app".to_string()),
        cmd: Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            config.start_command.to_string(),
        ]),
        env: Some(env_strings(config.env_vars)),
        host_config: Some(host_config),
        networking_config: Some(networking_config()),
        ..Default::default()
    };

    let options = CreateContainerOptionsBuilder::new().name(name).build();
    docker
        .create_container(Some(options), body)
        .await
        .map_err(|err| StartError::StartFailed(start_failed(&err)))?;
    docker
        .start_container(name, None)
        .await
        .map_err(|err| StartError::StartFailed(start_failed(&err)))
}

pub fn install_container_name(parent_name: &str) -> String {
    format!("{parent_name}-install")
}

fn build_container_name(parent_name: &str) -> String {
    format!("{parent_name}-build")
}

/// Bundles a one-shot command's config to stay under clippy's arg-count
/// lint. `log_sink`, if given, receives the command's output live as it
/// runs.
pub struct CommandExec<'a> {
    pub image: &'a str,
    pub command: &'a str,
    pub env_vars: &'a [EnvVar],
    pub timeout: Duration,
    pub log_sink: Option<mpsc::Sender<Bytes>>,
}

pub async fn run_build_command(
    docker: &Docker,
    parent_name: &str,
    checkout_dir: &Path,
    exec: CommandExec<'_>,
) -> Result<(), StartError> {
    run_command_to_completion(
        docker,
        &build_container_name(parent_name),
        checkout_dir,
        exec,
    )
    .await
}

/// Doesn't remove the container immediately on exit - `schedule_cleanup`
/// gives a grace period before force-removing it.
async fn run_command_to_completion(
    docker: &Docker,
    container_name: &str,
    checkout_dir: &Path,
    exec: CommandExec<'_>,
) -> Result<(), StartError> {
    ensure_image(docker, exec.image)
        .await
        .map_err(StartError::StartFailed)?;

    let body = ContainerCreateBody {
        image: Some(exec.image.to_string()),
        working_dir: Some("/app".to_string()),
        cmd: Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            exec.command.to_string(),
        ]),
        env: Some(env_strings(exec.env_vars)),
        host_config: Some(HostConfig {
            binds: Some(vec![bind_mount(checkout_dir)]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let options = CreateContainerOptionsBuilder::new()
        .name(container_name)
        .build();
    docker
        .create_container(Some(options), body)
        .await
        .map_err(|err| StartError::StartFailed(start_failed(&err)))?;
    docker
        .start_container(container_name, None)
        .await
        .map_err(|err| StartError::StartFailed(start_failed(&err)))?;

    let pump_handle = exec.log_sink.map(|sink| {
        let source = logs(docker, container_name, true);
        tokio::spawn(forward_logs(source, sink))
    });

    let wait_options = WaitContainerOptionsBuilder::new().build();
    let mut wait_stream = docker.wait_container(container_name, Some(wait_options));
    let wait_result = tokio::time::timeout(exec.timeout, wait_stream.try_next()).await;

    let result = match wait_result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(BollardError::DockerContainerWaitError { error, code })) => Err(
            StartError::CommandFailed(format!("command exited {code}: {error}")),
        ),
        Ok(Err(err)) => Err(StartError::StartFailed(start_failed(&err))),
        Err(_) => {
            // Stop it explicitly on timeout rather than leaving it running
            // for the whole grace period below.
            docker.stop_container(container_name, None).await.ok();
            Err(StartError::CommandFailed(format!(
                "command timed out after {}s",
                exec.timeout.as_secs()
            )))
        }
    };

    // Bounded wait so the caller's final result always follows every chunk
    // of output that led up to it.
    if let Some(handle) = pump_handle {
        let _ = tokio::time::timeout(PUMP_JOIN_TIMEOUT, handle).await;
    }

    schedule_cleanup(docker.clone(), container_name.to_string());
    result
}

const PUMP_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

async fn forward_logs(
    mut source: impl Stream<Item = Result<Bytes, String>> + Unpin,
    sink: mpsc::Sender<Bytes>,
) {
    while let Some(chunk) = source.next().await {
        let Ok(chunk) = chunk else { break };
        if sink.send(chunk).await.is_err() {
            break;
        }
    }
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
pub async fn stop_and_remove(docker: &Docker, name: &str) -> Result<(), String> {
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

pub async fn container_ip(docker: &Docker, name: &str) -> Result<Option<String>, String> {
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
/// drops it or the container stops.
pub fn logs(
    docker: &Docker,
    name: &str,
    follow: bool,
) -> impl Stream<Item = Result<Bytes, String>> + use<> {
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

pub struct ContainerStats {
    pub cpu_percent: f64,
    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,
}

/// `stream(false)`/`one_shot(false)`: a single request, but Podman still
/// waits to gather two samples internally so `cpu_stats`/`precpu_stats`
/// are both populated - needed for the CPU% delta below.
pub async fn stats(docker: &Docker, name: &str) -> Result<ContainerStats, String> {
    let options = StatsOptionsBuilder::new()
        .stream(false)
        .one_shot(false)
        .build();
    let response = docker
        .stats(name, Some(options))
        .try_next()
        .await
        .map_err(|err| unavailable(&err))?
        .ok_or_else(|| format!("no stats for {name}"))?;

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

pub async fn is_running(docker: &Docker, name: &str) -> Result<bool, String> {
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
mod live_tests {
    use super::*;

    fn temp_checkout(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "berth-agent-containers-live-{label}-{}",
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
        let name = "berth-live-test-lifecycle";
        stop_and_remove(&docker, name).await.ok();

        start(
            &docker,
            name,
            &checkout,
            RunContainerConfig {
                image: "docker.io/library/node:24",
                install_command: None,
                start_command:
                    "node -e \"require('http').createServer((_, res) => res.end('ok')).listen(3000)\"",
                env_vars: &[],
                install_timeout: Duration::from_mins(1),
            },
            None,
        )
        .await
        .ok()
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
            let addr = format!("{ip}:3000");
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
        let name = "berth-live-test-install-fail";
        let install_name = install_container_name(name);
        stop_and_remove(&docker, name).await.ok();
        stop_and_remove(&docker, &install_name).await.ok();

        let result = start(
            &docker,
            name,
            &checkout,
            RunContainerConfig {
                image: "docker.io/library/node:24",
                install_command: Some("exit 1"),
                start_command: "node -e \"1\"",
                env_vars: &[],
                install_timeout: Duration::from_mins(1),
            },
            None,
        )
        .await;
        assert!(result.is_err());
        assert!(!is_running(&docker, name).await.expect("is_running"));
        std::fs::remove_dir_all(&checkout).ok();
        stop_and_remove(&docker, &install_name).await.ok();
    }

    #[tokio::test]
    async fn install_command_timeout_leaves_nothing_running() {
        let docker = connect().expect("connect to podman");
        ensure_network(&docker).await.expect("ensure_network");
        let checkout = temp_checkout("install-timeout");
        let name = "berth-live-test-install-timeout";
        let install_name = install_container_name(name);
        stop_and_remove(&docker, name).await.ok();
        stop_and_remove(&docker, &install_name).await.ok();

        let result = start(
            &docker,
            name,
            &checkout,
            RunContainerConfig {
                image: "docker.io/library/node:24",
                install_command: Some("sleep 5"),
                start_command: "node -e \"1\"",
                env_vars: &[],
                install_timeout: Duration::from_millis(200),
            },
            None,
        )
        .await;
        assert!(matches!(result, Err(StartError::CommandFailed(_))));
        assert!(!is_running(&docker, name).await.expect("is_running"));
        std::fs::remove_dir_all(&checkout).ok();
        stop_and_remove(&docker, &install_name).await.ok();
    }
}
