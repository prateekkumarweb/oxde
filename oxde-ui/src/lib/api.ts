import { useMemo } from "react";

import type {
  ApiTokenView,
  AppPermission,
  AppSource,
  AppView,
  ContainerStats,
  CreateApiTokenResponse,
  DeploymentView,
  EnvVar,
  HostStats,
  LogKind,
  RunConfig,
  UserView,
} from "@/lib/types";

import { useAuth } from "@/lib/auth";

interface CreateAppInput {
  name: string;
  source?: AppSource;
  env_vars?: EnvVar[];
}

interface CreateUserInput {
  username: string;
  password: string;
  role: string;
}

interface UpdateUserInput {
  role?: string;
  password?: string;
}

interface CreateApiTokenInput {
  name: string;
  /** Epoch seconds. */
  expires_at: number;
}

interface Api {
  listApps: () => Promise<AppView[]>;
  createApp: (input: CreateAppInput) => Promise<AppView>;
  getApp: (name: string) => Promise<AppView>;
  deleteApp: (name: string) => Promise<void>;
  updateAppEnvVars: (name: string, envVars: EnvVar[]) => Promise<AppView>;
  updateAppPermissions: (name: string, permissions: AppPermission[]) => Promise<AppView>;
  listUsers: () => Promise<UserView[]>;
  createUser: (input: CreateUserInput) => Promise<UserView>;
  updateUser: (username: string, input: UpdateUserInput) => Promise<UserView>;
  deleteUser: (username: string) => Promise<void>;
  changeOwnPassword: (currentPassword: string, newPassword: string) => Promise<void>;
  listApiTokens: () => Promise<ApiTokenView[]>;
  createApiToken: (input: CreateApiTokenInput) => Promise<CreateApiTokenResponse>;
  revokeApiToken: (id: number) => Promise<void>;
  listDeployments: (appName: string) => Promise<DeploymentView[]>;
  uploadDeployment: (appName: string, file: File) => Promise<DeploymentView>;
  deployFromGit: (appName: string) => Promise<DeploymentView>;
  activateDeployment: (appName: string, id: string) => Promise<void>;
  deleteDeployment: (appName: string, id: string) => Promise<void>;
  streamLogs: (
    appName: string,
    id: string,
    options: { phase: LogKind; follow: boolean; signal?: AbortSignal },
  ) => Promise<Response>;
  getDeploymentStats: (appName: string, id: string) => Promise<ContainerStats | null>;
  getHostStats: () => Promise<HostStats>;
}

export function useApi(): Api {
  const { request, requestStream } = useAuth();

  return useMemo<Api>(
    () => ({
      listApps: () => request("/apps"),

      createApp: (input) =>
        request("/apps", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(input),
        }),

      getApp: (name) => request(`/apps/${encodeURIComponent(name)}`),

      deleteApp: (name) => request(`/apps/${encodeURIComponent(name)}`, { method: "DELETE" }),

      updateAppEnvVars: (name, envVars) =>
        request(`/apps/${encodeURIComponent(name)}`, {
          method: "PATCH",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ env_vars: envVars }),
        }),

      updateAppPermissions: (name, permissions) =>
        request(`/apps/${encodeURIComponent(name)}/permissions`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ permissions }),
        }),

      listUsers: () => request("/users"),

      createUser: (input) =>
        request("/users", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(input),
        }),

      updateUser: (username, input) =>
        request(`/users/${encodeURIComponent(username)}`, {
          method: "PATCH",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(input),
        }),

      deleteUser: (username) =>
        request(`/users/${encodeURIComponent(username)}`, { method: "DELETE" }),

      changeOwnPassword: (currentPassword, newPassword) =>
        request("/users/me/password", {
          method: "PATCH",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            current_password: currentPassword,
            new_password: newPassword,
          }),
        }),

      listApiTokens: () => request("/users/me/tokens"),

      createApiToken: (input) =>
        request("/users/me/tokens", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(input),
        }),

      revokeApiToken: (id) => request(`/users/me/tokens/${id}`, { method: "DELETE" }),

      listDeployments: (appName) => request(`/apps/${encodeURIComponent(appName)}/deployments`),

      uploadDeployment: (appName, file) => {
        const formData = new FormData();
        formData.append("file", file);
        return request(`/apps/${encodeURIComponent(appName)}/deployments`, {
          method: "POST",
          body: formData,
        });
      },

      deployFromGit: (appName) =>
        request(`/apps/${encodeURIComponent(appName)}/deployments/git`, { method: "POST" }),

      activateDeployment: (appName, id) =>
        request(
          `/apps/${encodeURIComponent(appName)}/deployments/${encodeURIComponent(id)}/activate`,
          {
            method: "POST",
          },
        ),

      deleteDeployment: (appName, id) =>
        request(`/apps/${encodeURIComponent(appName)}/deployments/${encodeURIComponent(id)}`, {
          method: "DELETE",
        }),

      streamLogs: (appName, id, { phase, follow, signal }) =>
        requestStream(
          `/apps/${encodeURIComponent(appName)}/deployments/${encodeURIComponent(id)}/logs?phase=${phase}&follow=${follow}`,
          { signal },
        ),

      getDeploymentStats: (appName, id) =>
        request(`/apps/${encodeURIComponent(appName)}/deployments/${encodeURIComponent(id)}/stats`),

      getHostStats: () => request("/host/stats"),
    }),
    [request, requestStream],
  );
}

export type {
  ApiTokenView,
  AppPermission,
  AppSource,
  AppView,
  ContainerStats,
  CreateApiTokenResponse,
  DeploymentView,
  HostStats,
  RunConfig,
  UserView,
};
