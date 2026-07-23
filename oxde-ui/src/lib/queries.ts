import { queryOptions, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import type { AppPermission, AppSource, EnvVar } from "@/lib/types";

import { useApi } from "@/lib/api";

type Api = ReturnType<typeof useApi>;

const appsKey = () => ["apps"] as const;
const appKey = (name: string) => ["apps", name] as const;
const deploymentsKey = (name: string) => ["apps", name, "deployments"] as const;
const deploymentStatsKey = (name: string, id: string) =>
  ["apps", name, "deployments", id, "stats"] as const;
const usersKey = () => ["users"] as const;
const apiTokensKey = () => ["apiTokens"] as const;
const hostStatsKey = () => ["host", "stats"] as const;

function appsOptions(api: Api) {
  return queryOptions({ queryKey: appsKey(), queryFn: api.listApps });
}

export function useApps() {
  return useQuery(appsOptions(useApi()));
}

function appOptions(api: Api, name: string) {
  return queryOptions({ queryKey: appKey(name), queryFn: () => api.getApp(name) });
}

export function useApp(name: string) {
  return useQuery(appOptions(useApi(), name));
}

function deploymentsOptions(api: Api, name: string) {
  return queryOptions({
    queryKey: deploymentsKey(name),
    queryFn: () => api.listDeployments(name),
    refetchInterval: (query) =>
      query.state.data?.some((deployment) => deployment.status.state === "pending") ? 2000 : false,
  });
}

export function useDeployments(name: string) {
  return useQuery(deploymentsOptions(useApi(), name));
}

function deploymentStatsOptions(api: Api, name: string, deploymentId: string) {
  return queryOptions({
    queryKey: deploymentStatsKey(name, deploymentId),
    queryFn: () => api.getDeploymentStats(name, deploymentId),
    refetchInterval: 5000,
  });
}

export function useDeploymentStats(name: string, deploymentId: string) {
  return useQuery(deploymentStatsOptions(useApi(), name, deploymentId));
}

function hostStatsOptions(api: Api) {
  return queryOptions({
    queryKey: hostStatsKey(),
    queryFn: api.getHostStats,
    refetchInterval: 2000,
  });
}

export function useHostStats(enabled: boolean) {
  return useQuery({ ...hostStatsOptions(useApi()), enabled });
}

export function useCreateApp() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { name: string; source?: AppSource; env_vars?: EnvVar[] }) =>
      api.createApp(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: appsKey() }),
  });
}

export function useUpdateAppEnvVars(name: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (envVars: EnvVar[]) => api.updateAppEnvVars(name, envVars),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: appKey(name) }),
  });
}

export function useUpdateAppPermissions(name: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (permissions: AppPermission[]) => api.updateAppPermissions(name, permissions),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: appKey(name) }),
  });
}

function usersOptions(api: Api) {
  return queryOptions({ queryKey: usersKey(), queryFn: api.listUsers });
}

export function useUsers() {
  return useQuery(usersOptions(useApi()));
}

export function useCreateUser() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { username: string; password: string; role: string }) =>
      api.createUser(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: usersKey() }),
  });
}

export function useUpdateUser() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ username, ...input }: { username: string; role?: string; password?: string }) =>
      api.updateUser(username, input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: usersKey() }),
  });
}

export function useDeleteUser() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (username: string) => api.deleteUser(username),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: usersKey() }),
  });
}

function apiTokensOptions(api: Api) {
  return queryOptions({ queryKey: apiTokensKey(), queryFn: api.listApiTokens });
}

export function useApiTokens() {
  return useQuery(apiTokensOptions(useApi()));
}

export function useCreateApiToken() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { name: string; expires_at: number }) => api.createApiToken(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: apiTokensKey() }),
  });
}

export function useRevokeApiToken() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => api.revokeApiToken(id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: apiTokensKey() }),
  });
}

export function useChangeOwnPassword() {
  const api = useApi();
  return useMutation({
    mutationFn: ({
      currentPassword,
      newPassword,
    }: {
      currentPassword: string;
      newPassword: string;
    }) => api.changeOwnPassword(currentPassword, newPassword),
  });
}

export function useDeleteApp(name: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => api.deleteApp(name),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: appsKey() }),
  });
}

/** Invalidates the app + its deployments after any deployment-mutating action. */
function useInvalidateDeployments(name: string) {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({ queryKey: appKey(name) });
    void queryClient.invalidateQueries({ queryKey: deploymentsKey(name) });
  };
}

export function useUploadDeployment(name: string) {
  const api = useApi();
  const invalidate = useInvalidateDeployments(name);
  return useMutation({
    mutationFn: (file: File) => api.uploadDeployment(name, file),
    onSuccess: invalidate,
  });
}

export function useDeployFromGit(name: string) {
  const api = useApi();
  const invalidate = useInvalidateDeployments(name);
  return useMutation({
    mutationFn: () => api.deployFromGit(name),
    onSuccess: invalidate,
  });
}

export function useActivateDeployment(name: string) {
  const api = useApi();
  const invalidate = useInvalidateDeployments(name);
  return useMutation({
    mutationFn: (deploymentId: string) => api.activateDeployment(name, deploymentId),
    onSuccess: invalidate,
  });
}

export function useDeleteDeployment(name: string) {
  const api = useApi();
  const invalidate = useInvalidateDeployments(name);
  return useMutation({
    mutationFn: (deploymentId: string) => api.deleteDeployment(name, deploymentId),
    onSuccess: invalidate,
  });
}
