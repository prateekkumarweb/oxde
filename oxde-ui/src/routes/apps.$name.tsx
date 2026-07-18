import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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

function AppDetail() {
  const { name } = Route.useParams();
  const navigate = useNavigate();
  const api = useApi();

  const [app, setApp] = useState<AppView | null>(null);
  const [deployments, setDeployments] = useState<DeploymentView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
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
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">{name}</h1>
        <Button variant="destructive" onClick={handleDeleteApp} disabled={busy}>
          Delete app
        </Button>
      </div>

      <p>
        Live at:{" "}
        <a
          href={`http://${appHost}/`}
          target="_blank"
          rel="noopener noreferrer"
          className="underline"
        >
          {appHost}
        </a>
      </p>

      {error && <p className="text-sm text-destructive">{error}</p>}

      {gitSource ? (
        <div className="flex flex-col gap-2 rounded-lg border p-4">
          <h2 className="font-medium">Git source</h2>
          <p className="text-sm">
            Repo: {gitSource.repo_url} (branch: {gitSource.branch})
          </p>
          {runConfig ? (
            <>
              <p className="text-sm">
                Mode: run ({RUN_IMAGE_TAGS[runConfig.image]}, port {runConfig.container_port})
              </p>
              <p className="text-sm">Start command: {runConfig.start_command}</p>
              {runConfig.install_command && (
                <p className="text-sm">Install command: {runConfig.install_command}</p>
              )}
            </>
          ) : (
            <p className="text-sm">
              Mode: static{gitSource.publish_dir && ` (publish dir: ${gitSource.publish_dir})`}
            </p>
          )}
          <Button
            onClick={() => runAction(() => api.deployFromGit(name))}
            disabled={busy}
            className="w-fit"
          >
            Pull latest &amp; deploy
          </Button>
        </div>
      ) : (
        <form onSubmit={handleUpload} className="flex items-center gap-2">
          <input ref={fileInputRef} type="file" accept=".zip" required className="text-sm" />
          <Button type="submit" disabled={busy}>
            Upload
          </Button>
        </form>
      )}

      <div>
        <h2 className="mb-2 font-medium">Deployments</h2>
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
                <TableHead />
              </TableRow>
            </TableHeader>
            <TableBody>
              {deployments.map((deployment) => (
                <TableRow key={deployment.id}>
                  <TableCell>{deployment.id}</TableCell>
                  <TableCell>{new Date(deployment.created_at).toLocaleString()}</TableCell>
                  <TableCell>{deployment.upload_size_bytes} bytes</TableCell>
                  <TableCell>{deployment.git?.commit_sha ?? "—"}</TableCell>
                  {runConfig && (
                    <TableCell>{deployment.container_status ?? "not started"}</TableCell>
                  )}
                  <TableCell className="flex gap-2">
                    {deployment.is_active ? (
                      <Badge>active</Badge>
                    ) : (
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
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </div>
    </div>
  );
}
