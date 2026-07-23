import { createFileRoute } from "@tanstack/react-router";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Sparkline } from "@/components/sparkline";
import { useAuth } from "@/lib/auth";
import { useHostStats } from "@/lib/queries";
import { useTimeSeries } from "@/lib/use-time-series";

export const Route = createFileRoute("/host")({
  component: HostPage,
});

function formatGb(bytes: number): string {
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)}GB`;
}

function MetricCard({
  title,
  currentLabel,
  points,
  valueLabel,
}: {
  title: string;
  currentLabel: string;
  points: ReturnType<typeof useTimeSeries>;
  valueLabel: (v: number) => string;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center justify-between text-base">
          <span>{title}</span>
          <span className="text-sm font-normal text-muted-foreground">{currentLabel}</span>
        </CardTitle>
      </CardHeader>
      <CardContent>
        <Sparkline points={points} valueLabel={valueLabel} />
      </CardContent>
    </Card>
  );
}

function HostPage() {
  const { user } = useAuth();
  const isAdmin = user?.role === "admin";
  const { data: stats, dataUpdatedAt } = useHostStats(isAdmin);

  const memoryPercent =
    stats && stats.memory_total_bytes > 0
      ? (stats.memory_usage_bytes / stats.memory_total_bytes) * 100
      : undefined;
  const diskPercent =
    stats && stats.disk_total_bytes > 0
      ? (stats.disk_usage_bytes / stats.disk_total_bytes) * 100
      : undefined;

  const cpuHistory = useTimeSeries(stats?.cpu_percent, dataUpdatedAt);
  const memoryHistory = useTimeSeries(memoryPercent, dataUpdatedAt);
  const diskHistory = useTimeSeries(diskPercent, dataUpdatedAt);

  if (!isAdmin) {
    return <p className="text-sm text-muted-foreground">Only admins can view host stats.</p>;
  }

  if (!stats) {
    return null;
  }

  return (
    <div className="flex flex-col gap-6">
      <h1 className="font-heading text-2xl font-semibold">Host</h1>
      <p className="text-sm text-muted-foreground">
        Resource usage for the machine this instance runs on, not any single app. Last 5 minutes.
      </p>

      <div className="grid grid-cols-1 gap-4 md:grid-cols-3">
        <MetricCard
          title="CPU"
          currentLabel={`${stats.cpu_percent.toFixed(0)}%`}
          points={cpuHistory}
          valueLabel={(v) => `${v.toFixed(0)}%`}
        />
        <MetricCard
          title="Memory"
          currentLabel={`${formatGb(stats.memory_usage_bytes)} / ${formatGb(stats.memory_total_bytes)}`}
          points={memoryHistory}
          valueLabel={(v) => `${v.toFixed(0)}%`}
        />
        <MetricCard
          title="Disk"
          currentLabel={`${formatGb(stats.disk_usage_bytes)} / ${formatGb(stats.disk_total_bytes)}`}
          points={diskHistory}
          valueLabel={(v) => `${v.toFixed(0)}%`}
        />
      </div>
    </div>
  );
}
