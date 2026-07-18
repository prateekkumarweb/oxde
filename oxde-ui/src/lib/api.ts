import { useMemo } from "react";
import { useAuth } from "@/lib/auth";
import type { AppSource, AppView, ContainerStats, DeploymentView, RunConfig } from "@/lib/types";

interface CreateAppInput {
  name: string;
  source?: AppSource;
}

interface Api {
  listApps: () => Promise<AppView[]>;
  createApp: (input: CreateAppInput) => Promise<AppView>;
  getApp: (name: string) => Promise<AppView>;
  deleteApp: (name: string) => Promise<void>;
  listDeployments: (appName: string) => Promise<DeploymentView[]>;
  uploadDeployment: (appName: string, file: File) => Promise<DeploymentView>;
  deployFromGit: (appName: string) => Promise<DeploymentView>;
  activateDeployment: (appName: string, id: string) => Promise<void>;
  deleteDeployment: (appName: string, id: string) => Promise<void>;
  streamLogs: (
    appName: string,
    id: string,
    options: { follow: boolean; signal?: AbortSignal },
  ) => Promise<Response>;
  getDeploymentStats: (appName: string, id: string) => Promise<ContainerStats | null>;
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

      streamLogs: (appName, id, { follow, signal }) =>
        requestStream(
          `/apps/${encodeURIComponent(appName)}/deployments/${encodeURIComponent(id)}/logs?follow=${follow}`,
          { signal },
        ),

      getDeploymentStats: (appName, id) =>
        request(`/apps/${encodeURIComponent(appName)}/deployments/${encodeURIComponent(id)}/stats`),
    }),
    [request, requestStream],
  );
}

export type { AppSource, AppView, ContainerStats, DeploymentView, RunConfig };
