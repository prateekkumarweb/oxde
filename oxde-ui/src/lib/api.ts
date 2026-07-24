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

interface UpdateAppInput {
  name?: string;
  env_vars?: EnvVar[];
}

interface Api {
  listApps: () => Promise<AppView[]>;
  createApp: (input: CreateAppInput) => Promise<AppView>;
  getApp: (id: string) => Promise<AppView>;
  deleteApp: (id: string) => Promise<void>;
  updateApp: (id: string, input: UpdateAppInput) => Promise<AppView>;
  updateAppPermissions: (id: string, permissions: AppPermission[]) => Promise<AppView>;
  listUsers: () => Promise<UserView[]>;
  createUser: (input: CreateUserInput) => Promise<UserView>;
  updateUser: (username: string, input: UpdateUserInput) => Promise<UserView>;
  deleteUser: (username: string) => Promise<void>;
  changeOwnPassword: (currentPassword: string, newPassword: string) => Promise<void>;
  listApiTokens: () => Promise<ApiTokenView[]>;
  createApiToken: (input: CreateApiTokenInput) => Promise<CreateApiTokenResponse>;
  revokeApiToken: (id: number) => Promise<void>;
  listDeployments: (appId: string) => Promise<DeploymentView[]>;
  uploadDeployment: (appId: string, file: File) => Promise<DeploymentView>;
  deployFromGit: (appId: string) => Promise<DeploymentView>;
  activateDeployment: (appId: string, id: string) => Promise<void>;
  deleteDeployment: (appId: string, id: string) => Promise<void>;
  streamLogs: (
    appId: string,
    id: string,
    options: { phase: LogKind; follow: boolean; signal?: AbortSignal },
  ) => Promise<Response>;
  getDeploymentStats: (appId: string, id: string) => Promise<ContainerStats | null>;
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

      getApp: (id) => request(`/apps/${encodeURIComponent(id)}`),

      deleteApp: (id) => request(`/apps/${encodeURIComponent(id)}`, { method: "DELETE" }),

      updateApp: (id, input) =>
        request(`/apps/${encodeURIComponent(id)}`, {
          method: "PATCH",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(input),
        }),

      updateAppPermissions: (id, permissions) =>
        request(`/apps/${encodeURIComponent(id)}/permissions`, {
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

      listDeployments: (appId) => request(`/apps/${encodeURIComponent(appId)}/deployments`),

      uploadDeployment: (appId, file) => {
        const formData = new FormData();
        formData.append("file", file);
        return request(`/apps/${encodeURIComponent(appId)}/deployments`, {
          method: "POST",
          body: formData,
        });
      },

      deployFromGit: (appId) =>
        request(`/apps/${encodeURIComponent(appId)}/deployments/git`, { method: "POST" }),

      activateDeployment: (appId, id) =>
        request(
          `/apps/${encodeURIComponent(appId)}/deployments/${encodeURIComponent(id)}/activate`,
          {
            method: "POST",
          },
        ),

      deleteDeployment: (appId, id) =>
        request(`/apps/${encodeURIComponent(appId)}/deployments/${encodeURIComponent(id)}`, {
          method: "DELETE",
        }),

      streamLogs: (appId, id, { phase, follow, signal }) =>
        requestStream(
          `/apps/${encodeURIComponent(appId)}/deployments/${encodeURIComponent(id)}/logs?phase=${phase}&follow=${follow}`,
          { signal },
        ),

      getDeploymentStats: (appId, id) =>
        request(`/apps/${encodeURIComponent(appId)}/deployments/${encodeURIComponent(id)}/stats`),

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
