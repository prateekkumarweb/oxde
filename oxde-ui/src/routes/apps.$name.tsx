import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { Fragment, type ReactNode, useRef, useState } from "react";

import type { AppPermission, EnvVar, RunImage } from "@/lib/types";

import { DeploymentLogs } from "@/components/deployment-logs";
import { DeploymentStats } from "@/components/deployment-stats";
import { EnvVarEditor } from "@/components/env-var-editor";
import { PermissionsEditor } from "@/components/permissions-editor";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ApiError, useAuth } from "@/lib/auth";
import {
  useActivateDeployment,
  useApp,
  useAppIdByName,
  useApps,
  useDeleteApp,
  useDeleteDeployment,
  useDeployFromGit,
  useDeployments,
  useRenameApp,
  useUpdateAppEnvVars,
  useUpdateAppPermissions,
  useUploadDeployment,
} from "@/lib/queries";

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

function Section({ title, children }: { title: ReactNode; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-3">
      <h2 className="border-b pb-2 font-heading text-lg font-medium">{title}</h2>
      {children}
    </section>
  );
}

function AppDetail() {
  const { name } = Route.useParams();
  const navigate = useNavigate();

  const appId = useAppIdByName(name);
  const { data: apps } = useApps();
  const notFound = apps !== undefined && appId === undefined;

  const { data: app, error: appError } = useApp(appId);
  const { data: deployments } = useDeployments(appId);
  const deleteApp = useDeleteApp(appId ?? "");
  const deployFromGit = useDeployFromGit(appId ?? "");
  const uploadDeployment = useUploadDeployment(appId ?? "");
  const activateDeployment = useActivateDeployment(appId ?? "");
  const deleteDeployment = useDeleteDeployment(appId ?? "");
  const updateAppEnvVars = useUpdateAppEnvVars(appId ?? "");
  const updateAppPermissions = useUpdateAppPermissions(appId ?? "");
  const renameApp = useRenameApp(appId ?? "");
  const { user } = useAuth();

  const [actionError, setActionError] = useState<string | null>(null);
  const [logsFor, setLogsFor] = useState<string | null>(null);
  const [localEnvVars, setLocalEnvVars] = useState<EnvVar[] | null>(null);
  const [localPermissions, setLocalPermissions] = useState<AppPermission[] | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [renameValue, setRenameValue] = useState("");
  const fileInputRef = useRef<HTMLInputElement>(null);

  const busy =
    deleteApp.isPending ||
    deployFromGit.isPending ||
    uploadDeployment.isPending ||
    activateDeployment.isPending ||
    deleteDeployment.isPending;

  async function runAction(action: () => Promise<unknown>) {
    setActionError(null);
    try {
      await action();
    } catch (err) {
      setActionError(err instanceof ApiError ? err.message : "Action failed");
    }
  }

  async function handleDeleteApp() {
    if (!confirm("Delete this app and all its deployments?")) {
      return;
    }
    await runAction(async () => {
      await deleteApp.mutateAsync();
      await navigate({ to: "/" });
    });
  }

  async function handleDeployFromGit() {
    await runAction(async () => {
      const deployment = await deployFromGit.mutateAsync();
      if (gitSource && gitSource.mode.type !== "static") {
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
    await runAction(() => uploadDeployment.mutateAsync(file));
    if (fileInputRef.current) {
      fileInputRef.current.value = "";
    }
  }

  async function handleSaveEnvVars(envVars: EnvVar[]) {
    const trimmed = envVars
      .map((envVar) => ({ key: envVar.key.trim(), value: envVar.value }))
      .filter((envVar) => envVar.key !== "");
    await runAction(async () => {
      await updateAppEnvVars.mutateAsync(trimmed);
      setLocalEnvVars(null);
    });
  }

  async function handleSavePermissions(permissions: AppPermission[]) {
    const trimmed = permissions
      .map((grant) => ({ ...grant, username: grant.username.trim() }))
      .filter((grant) => grant.username !== "");
    await runAction(async () => {
      await updateAppPermissions.mutateAsync(trimmed);
      setLocalPermissions(null);
    });
  }

  async function handleStartRename() {
    if (!app) {
      return;
    }
    setRenameValue(app.name);
    setRenaming(true);
  }

  async function handleSubmitRename(event: React.FormEvent) {
    event.preventDefault();
    const trimmed = renameValue.trim();
    if (trimmed === "" || trimmed === app?.name) {
      setRenaming(false);
      return;
    }
    await runAction(async () => {
      const renamed = await renameApp.mutateAsync(trimmed);
      setRenaming(false);
      await navigate({ to: "/apps/$name", params: { name: renamed.name } });
    });
  }

  const error =
    actionError ??
    (appError instanceof ApiError ? appError.message : appError && "Failed to load app");

  if (notFound) {
    return <p className="text-sm text-destructive">App "{name}" not found.</p>;
  }
  if (error && !app) {
    return <p className="text-sm text-destructive">{error}</p>;
  }
  if (!app) {
    return <p className="text-muted-foreground">Loading…</p>;
  }

  const backendPort = import.meta.env.DEV ? "8080" : window.location.port;
  const appHost = `${app.name}.${window.location.hostname}${backendPort ? `:${backendPort}` : ""}`;
  const gitSource = app.source.type === "git" ? app.source : null;
  const runConfig = gitSource?.mode.type === "run" ? gitSource.mode : null;
  const buildConfig = gitSource?.mode.type === "build" ? gitSource.mode : null;
  const publishDir = gitSource?.mode.type === "static" ? gitSource.mode.publish_dir : null;
  const envVars = localEnvVars ?? app.env_vars;
  const envVarsDirty = localEnvVars !== null;
  const permissions = localPermissions ?? app.permissions;
  const permissionsDirty = localPermissions !== null;

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-start justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h1 className="font-heading text-2xl font-semibold">{app.name}</h1>
          <a
            href={`${window.location.protocol}//${appHost}/`}
            target="_blank"
            rel="noopener noreferrer"
            className="font-mono text-sm text-muted-foreground hover:text-foreground hover:underline"
          >
            {appHost}
          </a>
        </div>
      </div>

      {error && <p className="text-sm text-destructive">{error}</p>}

      <Tabs defaultValue="general">
        <TabsList>
          <TabsTrigger value="general">Info</TabsTrigger>
          <TabsTrigger value="deployments">Deployments</TabsTrigger>
          <TabsTrigger value="settings">Settings</TabsTrigger>
        </TabsList>

        <TabsContent value="general">
          <Section
            title={
              <div className="flex items-center gap-2">
                Source
                <Badge variant="secondary">{gitSource ? "git" : "upload"}</Badge>
                {gitSource && <Badge variant="outline">{gitSource.mode.type}</Badge>}
              </div>
            }
          >
            {gitSource ? (
              <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 text-sm">
                <dt className="text-muted-foreground">Repo</dt>
                <dd className="truncate">{gitSource.repo_url}</dd>
                <dt className="text-muted-foreground">Branch</dt>
                <dd>{gitSource.branch}</dd>
                {runConfig && (
                  <>
                    <dt className="text-muted-foreground">Image</dt>
                    <dd>
                      <code className="rounded bg-muted px-1.5 py-0.5 text-xs">
                        {RUN_IMAGE_TAGS[runConfig.image]}
                      </code>
                    </dd>
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
                )}
                {buildConfig && (
                  <>
                    <dt className="text-muted-foreground">Image</dt>
                    <dd>
                      <code className="rounded bg-muted px-1.5 py-0.5 text-xs">
                        {RUN_IMAGE_TAGS[buildConfig.image]}
                      </code>
                    </dd>
                    <dt className="text-muted-foreground">Build</dt>
                    <dd>
                      <code className="rounded bg-muted px-1.5 py-0.5 text-xs">
                        {buildConfig.command}
                      </code>
                    </dd>
                    <dt className="text-muted-foreground">Output dir</dt>
                    <dd>
                      <code className="rounded bg-muted px-1.5 py-0.5 text-xs">
                        {buildConfig.output_dir}
                      </code>
                    </dd>
                  </>
                )}
                {publishDir && (
                  <>
                    <dt className="text-muted-foreground">Publish dir</dt>
                    <dd>{publishDir}</dd>
                  </>
                )}
              </dl>
            ) : (
              <p className="text-sm text-muted-foreground">
                Deployments are uploaded as zip files.
              </p>
            )}
          </Section>
        </TabsContent>

        <TabsContent value="deployments" className="flex flex-col gap-6">
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

          <div className="flex flex-col gap-3">
            <h2 className="border-b pb-2 font-heading text-lg font-medium">Deployments</h2>
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
                                <DeploymentStats appId={app.id} deploymentId={deployment.id} />
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
                            {(runConfig ||
                              (buildConfig && deployment.status.state !== "ready")) && (
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
                                    runAction(() => activateDeployment.mutateAsync(deployment.id))
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
                                      void runAction(() =>
                                        deleteDeployment.mutateAsync(deployment.id),
                                      );
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
                                    void runAction(() =>
                                      deleteDeployment.mutateAsync(deployment.id),
                                    );
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
                              appId={app.id}
                              deploymentId={deployment.id}
                              source={app.source}
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
        </TabsContent>

        <TabsContent value="settings" className="flex flex-col gap-6">
          <Section title="General">
            <div className="flex flex-col gap-2">
              <Label>App name</Label>
              {renaming ? (
                <form onSubmit={handleSubmitRename} className="flex items-center gap-2">
                  <input
                    autoFocus
                    value={renameValue}
                    onChange={(event) => setRenameValue(event.target.value)}
                    className="rounded border bg-background px-2 py-1 text-sm"
                  />
                  <Button type="submit" size="sm" disabled={renameApp.isPending}>
                    Save
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => setRenaming(false)}
                    disabled={renameApp.isPending}
                  >
                    Cancel
                  </Button>
                </form>
              ) : (
                <div className="flex items-center gap-2">
                  <span className="text-sm">{app.name}</span>
                  <Button variant="outline" size="sm" onClick={handleStartRename}>
                    Rename
                  </Button>
                </div>
              )}
            </div>
          </Section>

          {(runConfig || buildConfig) && (
            <Section title="Environment variables">
              <EnvVarEditor envVars={envVars} onChange={setLocalEnvVars} />
              <Button
                onClick={() => handleSaveEnvVars(envVars)}
                disabled={busy || updateAppEnvVars.isPending || !envVarsDirty}
                className="self-start"
              >
                Save
              </Button>
            </Section>
          )}

          {user?.role === "admin" && (
            <Section title="Collaborators">
              <PermissionsEditor permissions={permissions} onChange={setLocalPermissions} />
              <Button
                onClick={() => handleSavePermissions(permissions)}
                disabled={busy || updateAppPermissions.isPending || !permissionsDirty}
                className="self-start"
              >
                Save
              </Button>
            </Section>
          )}

          <Section title="Danger zone">
            <Button
              variant="destructive"
              onClick={handleDeleteApp}
              disabled={busy}
              className="self-start"
            >
              Delete app
            </Button>
          </Section>
        </TabsContent>
      </Tabs>
    </div>
  );
}
