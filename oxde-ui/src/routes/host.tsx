import { createFileRoute } from "@tanstack/react-router";

import { Sparkline } from "@/components/sparkline";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
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
  children,
}: {
  title: string;
  currentLabel: string;
  points: ReturnType<typeof useTimeSeries>;
  valueLabel: (v: number) => string;
  children?: React.ReactNode;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center justify-between text-base">
          <span>{title}</span>
          <span className="text-sm font-normal text-muted-foreground">{currentLabel}</span>
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <Sparkline points={points} valueLabel={valueLabel} heightClassName="h-32" />
        {children}
      </CardContent>
    </Card>
  );
}

function CoreBars({ perCorePercent }: { perCorePercent: number[] }) {
  return (
    <div className="grid grid-cols-1 gap-x-6 gap-y-1.5 sm:grid-cols-2">
      {perCorePercent.map((percent, i) => (
        <div key={i} className="flex items-center gap-2 text-xs">
          <span className="w-14 shrink-0 text-muted-foreground">core {i}</span>
          <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-primary/15">
            <div
              className="h-full rounded-full bg-primary"
              style={{ width: `${Math.min(100, Math.max(0, percent))}%` }}
            />
          </div>
          <span className="w-10 shrink-0 text-right font-mono text-muted-foreground tabular-nums">
            {percent.toFixed(0)}%
          </span>
        </div>
      ))}
    </div>
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
        Resource usage for the machine this instance runs on, not any single app. Last 15 minutes.
      </p>

      <div className="flex flex-col gap-4">
        <MetricCard
          title="CPU"
          currentLabel={`${stats.cpu_percent.toFixed(0)}% avg`}
          points={cpuHistory}
          valueLabel={(v) => `${v.toFixed(0)}%`}
        >
          <CoreBars perCorePercent={stats.cpu_per_core_percent} />
        </MetricCard>
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
