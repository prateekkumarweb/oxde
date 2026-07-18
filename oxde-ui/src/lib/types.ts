export type RunImage = "node24" | "python314";

export interface RunConfig {
  image: RunImage;
  install_command: string | null;
  start_command: string;
  container_port: number;
}

export type AppSource =
  | { type: "upload" }
  | {
      type: "git";
      repo_url: string;
      branch: string;
      publish_dir: string | null;
      run: RunConfig | null;
    };

export interface AppView {
  name: string;
  created_at: string;
  active_deployment_id: string | null;
  source: AppSource;
}

export type ContainerStatus = "running" | "stopped" | "unknown";

export interface DeploymentView {
  id: string;
  app: string;
  created_at: string;
  original_filename: string | null;
  upload_size_bytes: number;
  git: { commit_sha: string; branch: string } | null;
  container_name: string | null;
  is_active: boolean;
  container_status: ContainerStatus | null;
}
