import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { DeploymentLogs } from "@/components/deployment-logs";
import { DeploymentStats } from "@/components/deployment-stats";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { useApi } from "@/lib/api";
import { ApiError } from "@/lib/auth";
import type { AppView, DeploymentView, RunImage } from "@/lib/types";

export const Route = createFileRoute("/apps/$name")({
  component: AppDetail,
});

const RUN_IMAGE_TAGS: Record<RunImage, string> = {
  node24: "node:24",
  python314: "python:3.14",
};

const SIZE_UNITS = ["B", "KB", "MB", "GB"];

function formatBytes(bytes: number): string {
  let value = bytes;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < SIZE_UNITS.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const precision = unitIndex === 0 ? 0 : 2;
  return `${value.toFixed(precision)} ${SIZE_UNITS[unitIndex]}`;
}

function AppDetail() {
  const { name } = Route.useParams();
  const navigate = useNavigate();
  const api = useApi();

  const [app, setApp] = useState<AppView | null>(null);
  const [deployments, setDeployments] = useState<DeploymentView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [logsFor, setLogsFor] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const refresh = useCallback(() => {
    Promise.all([api.getApp(name), api.listDeployments(name)])
      .then(([appResult, deploymentsResult]) => {
        setApp(appResult);
        setDeployments(deploymentsResult);
      })
      .catch((err) => setError(err instanceof ApiError ? err.message : "Failed to load app"));
  }, [api, name]);

  useEffect(refresh, [refresh]);

  useEffect(() => {
    if (!deployments?.some((deployment) => deployment.status.state === "pending")) {
      return;
    }
    const interval = setInterval(refresh, 2000);
    return () => clearInterval(interval);
  }, [deployments, refresh]);

  async function runAction(action: () => Promise<unknown>) {
    setError(null);
    setBusy(true);
    try {
      await action();
      refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Action failed");
    } finally {
      setBusy(false);
    }
  }

  async function handleDeleteApp() {
    if (!confirm("Delete this app and all its deployments?")) {
      return;
    }
    await runAction(async () => {
      await api.deleteApp(name);
      await navigate({ to: "/" });
    });
  }

  async function handleDeployFromGit() {
    await runAction(async () => {
      const deployment = await api.deployFromGit(name);
      if (runConfig) {
        setLogsFor(deployment.id);
      }
    });
  }

  async function handleUpload(event: React.FormEvent) {
    event.preventDefault();
    const file = fileInputRef.current?.files?.[0];
    if (!file) {
      return;
    }
    await runAction(() => api.uploadDeployment(name, file));
    if (fileInputRef.current) {
      fileInputRef.current.value = "";
    }
  }

  if (error && !app) {
    return <p className="text-sm text-destructive">{error}</p>;
  }
  if (!app) {
    return <p className="text-muted-foreground">Loading…</p>;
  }

  const backendPort = import.meta.env.DEV ? "3000" : window.location.port;
  const appHost = `${name}.${window.location.hostname}${backendPort ? `:${backendPort}` : ""}`;
  const gitSource = app.source.type === "git" ? app.source : null;
  const runConfig = gitSource?.run ?? null;

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-start justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h1 className="font-heading text-2xl font-semibold">{name}</h1>
          <a
            href={`http://${appHost}/`}
            target="_blank"
            rel="noopener noreferrer"
            className="font-mono text-sm text-muted-foreground hover:text-foreground hover:underline"
          >
            {appHost}
          </a>
        </div>
        <Button variant="destructive" onClick={handleDeleteApp} disabled={busy}>
          Delete app
        </Button>
      </div>

      {error && <p className="text-sm text-destructive">{error}</p>}

      <Card className="max-w-2xl">
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            Source
            <Badge variant="secondary">{gitSource ? "git" : "upload"}</Badge>
            {gitSource && <Badge variant="outline">{runConfig ? "run" : "static"}</Badge>}
          </CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {gitSource ? (
            <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 text-sm">
              <dt className="text-muted-foreground">Repo</dt>
              <dd className="truncate">{gitSource.repo_url}</dd>
              <dt className="text-muted-foreground">Branch</dt>
              <dd>{gitSource.branch}</dd>
              {runConfig ? (
                <>
                  <dt className="text-muted-foreground">Image</dt>
                  <dd>{RUN_IMAGE_TAGS[runConfig.image]}</dd>
                  <dt className="text-muted-foreground">Port</dt>
                  <dd>{runConfig.container_port}</dd>
                  {runConfig.install_command && (
                    <>
                      <dt className="text-muted-foreground">Install</dt>
                      <dd>
                        <code className="rounded bg-muted px-1.5 py-0.5 text-xs">
                          {runConfig.install_command}
                        </code>
                      </dd>
                    </>
                  )}
                  <dt className="text-muted-foreground">Start</dt>
                  <dd>
                    <code className="rounded bg-muted px-1.5 py-0.5 text-xs">
                      {runConfig.start_command}
                    </code>
                  </dd>
                </>
              ) : (
                gitSource.publish_dir && (
                  <>
                    <dt className="text-muted-foreground">Publish dir</dt>
                    <dd>{gitSource.publish_dir}</dd>
                  </>
                )
              )}
            </dl>
          ) : (
            <p className="text-sm text-muted-foreground">Deployments are uploaded as zip files.</p>
          )}

          {gitSource ? (
            <Button onClick={handleDeployFromGit} disabled={busy} className="self-start">
              Pull latest &amp; deploy
            </Button>
          ) : (
            <form onSubmit={handleUpload} className="flex items-center gap-2">
              <input ref={fileInputRef} type="file" accept=".zip" required className="text-sm" />
              <Button type="submit" disabled={busy}>
                Upload
              </Button>
            </form>
          )}
        </CardContent>
      </Card>

      <div>
        <h2 className="mb-2 font-heading text-lg font-medium">Deployments</h2>
        {deployments && deployments.length === 0 && (
          <p className="text-muted-foreground">No deployments yet.</p>
        )}
        {deployments && deployments.length > 0 && (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>ID</TableHead>
                <TableHead>Created</TableHead>
                <TableHead>Size</TableHead>
                <TableHead>Commit</TableHead>
                {runConfig && <TableHead>Container</TableHead>}
                <TableHead>Status</TableHead>
                <TableHead className="text-right">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {deployments.map((deployment) => (
                <Fragment key={deployment.id}>
                  <TableRow>
                    <TableCell className="font-mono text-xs">{deployment.id}</TableCell>
                    <TableCell>{new Date(deployment.created_at).toLocaleString()}</TableCell>
                    <TableCell>{formatBytes(deployment.upload_size_bytes)}</TableCell>
                    <TableCell className="font-mono text-xs">
                      {deployment.git?.commit_sha ?? "—"}
                    </TableCell>
                    {runConfig && (
                      <TableCell>
                        <div className="flex items-center gap-2">
                          <Badge variant="outline">
                            {deployment.container_status ?? "not started"}
                          </Badge>
                          {deployment.container_status === "running" && (
                            <DeploymentStats appName={name} deploymentId={deployment.id} />
                          )}
                        </div>
                      </TableCell>
                    )}
                    <TableCell>
                      <div className="flex items-center gap-2">
                        {deployment.status.state === "pending" && (
                          <Badge variant="outline">deploying…</Badge>
                        )}
                        {deployment.status.state === "failed" && (
                          <Badge variant="destructive" title={deployment.status.error}>
                            failed
                          </Badge>
                        )}
                        {deployment.is_active && <Badge>active</Badge>}
                      </div>
                    </TableCell>
                    <TableCell className="text-right">
                      <div className="flex justify-end gap-2">
                        {runConfig && (
                          <Button
                            size="sm"
                            variant="outline"
                            onClick={() =>
                              setLogsFor(logsFor === deployment.id ? null : deployment.id)
                            }
                          >
                            Logs
                          </Button>
                        )}
                        {!deployment.is_active && deployment.status.state === "ready" && (
                          <>
                            <Button
                              size="sm"
                              variant="outline"
                              disabled={busy}
                              onClick={() =>
                                runAction(() => api.activateDeployment(name, deployment.id))
                              }
                            >
                              Activate
                            </Button>
                            <Button
                              size="sm"
                              variant="destructive"
                              disabled={busy}
                              onClick={() => {
                                if (confirm("Delete this deployment?")) {
                                  void runAction(() => api.deleteDeployment(name, deployment.id));
                                }
                              }}
                            >
                              Delete
                            </Button>
                          </>
                        )}
                        {!deployment.is_active && deployment.status.state === "failed" && (
                          <Button
                            size="sm"
                            variant="destructive"
                            disabled={busy}
                            onClick={() => {
                              if (confirm("Delete this deployment?")) {
                                void runAction(() => api.deleteDeployment(name, deployment.id));
                              }
                            }}
                          >
                            Delete
                          </Button>
                        )}
                      </div>
                    </TableCell>
                  </TableRow>
                  {logsFor === deployment.id && (
                    <TableRow>
                      <TableCell colSpan={runConfig ? 7 : 6}>
                        <DeploymentLogs
                          appName={name}
                          deploymentId={deployment.id}
                          onClose={() => setLogsFor(null)}
                        />
                      </TableCell>
                    </TableRow>
                  )}
                </Fragment>
              ))}
            </TableBody>
          </Table>
        )}
      </div>
    </div>
  );
}
