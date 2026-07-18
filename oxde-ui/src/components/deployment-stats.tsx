import { useEffect, useState } from "react";
import { useApi } from "@/lib/api";
import type { ContainerStats } from "@/lib/types";

const POLL_INTERVAL_MS = 5000;

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
  const api = useApi();
  const [stats, setStats] = useState<ContainerStats | null>(null);

  useEffect(() => {
    let cancelled = false;

    function poll() {
      api
        .getDeploymentStats(appName, deploymentId)
        .then((result) => {
          if (!cancelled) {
            setStats(result);
          }
        })
        .catch(() => {
          if (!cancelled) {
            setStats(null);
          }
        });
    }

    poll();
    const interval = setInterval(poll, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [api, appName, deploymentId]);

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
