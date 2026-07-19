export type RunImage = "node24" | "python314";

export interface RunConfig {
  image: RunImage;
  install_command: string | null;
  start_command: string;
  container_port: number;
}

export interface BuildConfig {
  image: RunImage;
  command: string;
  output_dir: string;
}

export type GitDeployMode =
  | { type: "static"; publish_dir: string | null }
  | ({ type: "build" } & BuildConfig)
  | ({ type: "run" } & RunConfig);

export type AppSource =
  | { type: "upload" }
  | {
      type: "git";
      repo_url: string;
      branch: string;
      mode: GitDeployMode;
    };

export interface EnvVar {
  key: string;
  value: string;
}

export interface AppView {
  name: string;
  created_at: string;
  active_deployment_id: string | null;
  source: AppSource;
  env_vars: EnvVar[];
}

export type ContainerStatus = "running" | "stopped" | "unknown";

export type DeploymentStatus =
  | { state: "pending" }
  | { state: "ready" }
  | { state: "failed"; error: string };

export interface DeploymentView {
  id: string;
  app: string;
  created_at: string;
  original_filename: string | null;
  upload_size_bytes: number;
  git: { commit_sha: string; branch: string } | null;
  build_info: { image: RunImage; command: string } | null;
  container_name: string | null;
  status: DeploymentStatus;
  is_active: boolean;
  container_status: ContainerStatus | null;
}

export interface ContainerStats {
  cpu_percent: number;
  memory_usage_bytes: number;
  memory_limit_bytes: number;
}
