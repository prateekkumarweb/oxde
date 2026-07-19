import { queryOptions, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApi } from "@/lib/api";
import type { AppSource } from "@/lib/types";

type Api = ReturnType<typeof useApi>;

const appsKey = () => ["apps"] as const;
const appKey = (name: string) => ["apps", name] as const;
const deploymentsKey = (name: string) => ["apps", name, "deployments"] as const;
const deploymentStatsKey = (name: string, id: string) =>
  ["apps", name, "deployments", id, "stats"] as const;

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

export function useCreateApp() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { name: string; source?: AppSource }) => api.createApp(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: appsKey() }),
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
