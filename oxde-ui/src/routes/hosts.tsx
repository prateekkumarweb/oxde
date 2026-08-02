import { createFileRoute } from "@tanstack/react-router";
import { Check, ChevronDown, ChevronRight, Copy } from "lucide-react";
import { useState, type FormEvent } from "react";

import type { HostView } from "@/lib/types";

import { Sparkline } from "@/components/sparkline";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ApiError, useAuth } from "@/lib/auth";
import {
  useCreateHost,
  useHostStats,
  useHosts,
  useRevokeHost,
  useUpdateHostIp,
} from "@/lib/queries";
import { useTimeSeries } from "@/lib/use-time-series";

export const Route = createFileRoute("/hosts")({
  component: HostsPage,
});

function HostsPage() {
  const { user } = useAuth();
  const isAdmin = user?.role === "admin";
  const { data: hosts, error: queryError } = useHosts(isAdmin);
  const [expandedHostId, setExpandedHostId] = useState<number | null>(null);
  const error =
    queryError instanceof ApiError ? queryError.message : queryError && "Failed to load hosts";

  if (!isAdmin) {
    return <p className="text-sm text-muted-foreground">Only admins can manage hosts.</p>;
  }

  return (
    <div className="flex flex-col gap-6">
      <h1 className="font-heading text-2xl font-semibold">Hosts</h1>
      <p className="text-sm text-muted-foreground">
        Each host runs its own agent, paired to this hub by a token. Copy a new host's token into
        that machine's <code>oxde-agent.toml</code> as <code>agent_token</code>.
      </p>

      {error && <p className="text-sm text-destructive">{error}</p>}

      <CreateHostForm />

      <div className="flex flex-col gap-3">
        {hosts?.map((host) => (
          <HostRow
            key={host.id}
            host={host}
            expanded={expandedHostId === host.id}
            onToggleExpanded={() => setExpandedHostId(expandedHostId === host.id ? null : host.id)}
          />
        ))}
      </div>
    </div>
  );
}

function CreateHostForm() {
  const createHost = useCreateHost();
  const [name, setName] = useState("");
  const [plaintextToken, setPlaintextToken] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    createHost.mutate(
      { name },
      {
        onSuccess: (response) => {
          setName("");
          setPlaintextToken(response.plaintext_token);
          setCopied(false);
        },
      },
    );
  }

  function handleCopy() {
    if (!plaintextToken) return;
    void navigator.clipboard.writeText(plaintextToken);
    setCopied(true);
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>New host</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {plaintextToken && (
          <div className="flex flex-col gap-2 rounded-lg border border-primary/30 bg-primary/5 p-3">
            <p className="text-sm font-medium">Copy this token now - it won't be shown again.</p>
            <div className="flex items-center gap-2">
              <code className="flex-1 overflow-x-auto text-xs">{plaintextToken}</code>
              <Button type="button" variant="outline" size="sm" onClick={handleCopy}>
                {copied ? <Check /> : <Copy />}
                {copied ? "Copied" : "Copy"}
              </Button>
            </div>
          </div>
        )}

        <form onSubmit={handleSubmit} className="flex flex-wrap items-end gap-3">
          <div className="flex flex-col gap-2">
            <Label htmlFor="host-name">Name</Label>
            <Input
              id="host-name"
              placeholder="raspberry-pi, office-server, ..."
              value={name}
              onChange={(event) => setName(event.target.value)}
              required
            />
          </div>
          <Button type="submit" disabled={createHost.isPending}>
            {createHost.isPending ? "Creating…" : "Create host"}
          </Button>
          {createHost.error && (
            <p className="w-full text-sm text-destructive">
              {createHost.error instanceof ApiError
                ? createHost.error.message
                : "Failed to create host"}
            </p>
          )}
        </form>
      </CardContent>
    </Card>
  );
}

function HostRow({
  host,
  expanded,
  onToggleExpanded,
}: {
  host: HostView;
  expanded: boolean;
  onToggleExpanded: () => void;
}) {
  const revokeHost = useRevokeHost();

  return (
    <div className="flex flex-col gap-3 rounded-lg border p-3">
      <div className="flex items-center justify-between gap-4">
        <button
          type="button"
          onClick={onToggleExpanded}
          className="flex items-center gap-2 text-left"
        >
          {expanded ? (
            <ChevronDown className="size-4 text-muted-foreground" />
          ) : (
            <ChevronRight className="size-4 text-muted-foreground" />
          )}
          <span className="font-medium">{host.name}</span>
          {host.revoked ? (
            <Badge variant="destructive">Revoked</Badge>
          ) : host.connected ? (
            <Badge variant="secondary">Connected</Badge>
          ) : (
            <Badge variant="outline">Disconnected</Badge>
          )}
        </button>
        <Button
          variant="outline"
          size="sm"
          disabled={host.revoked || revokeHost.isPending}
          onClick={() => revokeHost.mutate(host.id)}
        >
          Revoke
        </Button>
      </div>
      <p className="text-sm text-muted-foreground">
        Created {new Date(host.created_at * 1000).toLocaleString()} - Last seen{" "}
        {host.last_seen_at ? new Date(host.last_seen_at * 1000).toLocaleString() : "never"}
      </p>
      {revokeHost.error && (
        <p className="text-sm text-destructive">
          {revokeHost.error instanceof ApiError ? revokeHost.error.message : "Action failed"}
        </p>
      )}

      {expanded && (
        <div className="flex flex-col gap-4 border-t pt-3">
          <HostIpField host={host} />
          {host.connected ? (
            <HostStatsPanel hostId={host.id} />
          ) : (
            <p className="text-sm text-muted-foreground">
              Stats are unavailable while this host is disconnected.
            </p>
          )}
        </div>
      )}
    </div>
  );
}

// Prefilled from `last_connected_ip` until the admin sets one explicitly.
function HostIpField({ host }: { host: HostView }) {
  const updateHostIp = useUpdateHostIp();
  const [ip, setIp] = useState(host.ip ?? host.last_connected_ip ?? "");

  function handleSave() {
    updateHostIp.mutate({ id: host.id, ip: ip.trim() === "" ? null : ip.trim() });
  }

  return (
    <div className="flex flex-col gap-2">
      <Label htmlFor={`host-ip-${host.id}`}>IP address</Label>
      <p className="text-sm text-muted-foreground">
        Tracked for reference only.
        {host.last_connected_ip && host.last_connected_ip !== host.ip && (
          <> Agent last connected from {host.last_connected_ip}.</>
        )}
      </p>
      <div className="flex flex-wrap items-center gap-3">
        <Input
          id={`host-ip-${host.id}`}
          className="max-w-56"
          placeholder="203.0.113.10"
          value={ip}
          onChange={(event) => setIp(event.target.value)}
        />
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={updateHostIp.isPending}
          onClick={handleSave}
        >
          Save
        </Button>
      </div>
      {updateHostIp.error && (
        <p className="text-sm text-destructive">
          {updateHostIp.error instanceof ApiError
            ? updateHostIp.error.message
            : "Failed to update IP"}
        </p>
      )}
    </div>
  );
}

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

function HostStatsPanel({ hostId }: { hostId: number }) {
  const { data: stats, dataUpdatedAt } = useHostStats(hostId, true);

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

  if (!stats) {
    return null;
  }

  return (
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
  );
}
