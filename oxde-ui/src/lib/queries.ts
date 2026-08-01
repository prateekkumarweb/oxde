import { queryOptions, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import type { AppPermission, AppSource, EnvVar } from "@/lib/types";

import { useApi } from "@/lib/api";

type Api = ReturnType<typeof useApi>;

const appsKey = () => ["apps"] as const;
const appKey = (id: string) => ["apps", id] as const;
const deploymentsKey = (id: string) => ["apps", id, "deployments"] as const;
const deploymentStatsKey = (id: string, deploymentId: string) =>
  ["apps", id, "deployments", deploymentId, "stats"] as const;
const usersKey = () => ["users"] as const;
const apiTokensKey = () => ["apiTokens"] as const;
const hostStatsKey = (hostId: number) => ["hosts", hostId, "stats"] as const;
const hostsKey = () => ["hosts"] as const;

function appsOptions(api: Api) {
  return queryOptions({ queryKey: appsKey(), queryFn: api.listApps });
}

export function useApps() {
  return useQuery(appsOptions(useApi()));
}

// Resolves an app's `id` from the already-cached apps list by `name` - the
// dashboard's URLs stay name-keyed while the API is id-keyed, so every
// detail-page hook needs this instead of a name-keyed API call.
export function useAppIdByName(name: string): string | undefined {
  const { data: apps } = useApps();
  return apps?.find((app) => app.name === name)?.id;
}

function appOptions(api: Api, id: string) {
  return queryOptions({
    queryKey: appKey(id),
    queryFn: () => api.getApp(id),
    enabled: id !== "",
  });
}

export function useApp(id: string | undefined) {
  return useQuery(appOptions(useApi(), id ?? ""));
}

function deploymentsOptions(api: Api, id: string) {
  return queryOptions({
    queryKey: deploymentsKey(id),
    queryFn: () => api.listDeployments(id),
    enabled: id !== "",
    refetchInterval: (query) =>
      query.state.data?.some((deployment) => deployment.status.state === "pending") ? 2000 : false,
  });
}

export function useDeployments(id: string | undefined) {
  return useQuery(deploymentsOptions(useApi(), id ?? ""));
}

function deploymentStatsOptions(api: Api, id: string, deploymentId: string) {
  return queryOptions({
    queryKey: deploymentStatsKey(id, deploymentId),
    queryFn: () => api.getDeploymentStats(id, deploymentId),
    refetchInterval: 5000,
  });
}

export function useDeploymentStats(id: string, deploymentId: string) {
  return useQuery(deploymentStatsOptions(useApi(), id, deploymentId));
}

function hostStatsOptions(api: Api, hostId: number) {
  return queryOptions({
    queryKey: hostStatsKey(hostId),
    queryFn: () => api.getHostStats(hostId),
    refetchInterval: 2000,
  });
}

export function useHostStats(hostId: number, enabled: boolean) {
  return useQuery({ ...hostStatsOptions(useApi(), hostId), enabled });
}

export function useCreateApp() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: {
      name: string;
      host_id: number;
      source?: AppSource;
      env_vars?: EnvVar[];
    }) => api.createApp(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: appsKey() }),
  });
}

export function useUpdateAppEnvVars(id: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (envVars: EnvVar[]) => api.updateApp(id, { env_vars: envVars }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: appKey(id) }),
  });
}

// Renaming also affects the apps list (subdomain/dashboard URL derive from
// `name`) and the app's own detail query, so both get invalidated.
export function useRenameApp(id: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => api.updateApp(id, { name }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: appsKey() });
      void queryClient.invalidateQueries({ queryKey: appKey(id) });
    },
  });
}

export function useUpdateAppHost(id: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (hostId: number) => api.updateApp(id, { host_id: hostId }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: appKey(id) }),
  });
}

export function useUpdateAppPermissions(id: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (permissions: AppPermission[]) => api.updateAppPermissions(id, permissions),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: appKey(id) }),
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

function hostsOptions(api: Api) {
  return queryOptions({ queryKey: hostsKey(), queryFn: api.listHosts });
}

export function useHosts(enabled: boolean) {
  return useQuery({ ...hostsOptions(useApi()), enabled });
}

export function useCreateHost() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { name: string }) => api.createHost(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: hostsKey() }),
  });
}

export function useRevokeHost() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => api.revokeHost(id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: hostsKey() }),
  });
}

export function useUpdateHostIp() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ip }: { id: number; ip: string | null }) => api.updateHostIp(id, ip),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: hostsKey() }),
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

export function useDeleteApp(id: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => api.deleteApp(id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: appsKey() }),
  });
}

// Invalidates the app + its deployments after any deployment-mutating action.
function useInvalidateDeployments(id: string) {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({ queryKey: appKey(id) });
    void queryClient.invalidateQueries({ queryKey: deploymentsKey(id) });
  };
}

export function useUploadDeployment(id: string) {
  const api = useApi();
  const invalidate = useInvalidateDeployments(id);
  return useMutation({
    mutationFn: (file: File) => api.uploadDeployment(id, file),
    onSuccess: invalidate,
  });
}

export function useDeployFromGit(id: string) {
  const api = useApi();
  const invalidate = useInvalidateDeployments(id);
  return useMutation({
    mutationFn: () => api.deployFromGit(id),
    onSuccess: invalidate,
  });
}

export function useActivateDeployment(id: string) {
  const api = useApi();
  const invalidate = useInvalidateDeployments(id);
  return useMutation({
    mutationFn: (deploymentId: string) => api.activateDeployment(id, deploymentId),
    onSuccess: invalidate,
  });
}

export function useDeleteDeployment(id: string) {
  const api = useApi();
  const invalidate = useInvalidateDeployments(id);
  return useMutation({
    mutationFn: (deploymentId: string) => api.deleteDeployment(id, deploymentId),
    onSuccess: invalidate,
  });
}
