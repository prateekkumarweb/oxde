import { useDeploymentStats } from "@/lib/queries";

function formatMb(bytes: number): string {
  return `${Math.round(bytes / (1024 * 1024))}MB`;
}

export function DeploymentStats({
  appName,
  deploymentId,
}: {
  appName: string;
  deploymentId: string;
}) {
  const { data: stats } = useDeploymentStats(appName, deploymentId);

  if (!stats) {
    return null;
  }

  return (
    <span className="text-xs text-muted-foreground">
      {stats.cpu_percent.toFixed(0)}% CPU · {formatMb(stats.memory_usage_bytes)} /{" "}
      {formatMb(stats.memory_limit_bytes)}
    </span>
  );
}
