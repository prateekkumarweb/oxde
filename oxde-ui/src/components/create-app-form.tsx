import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useCreateApp } from "@/lib/queries";
import { ApiError } from "@/lib/auth";
import { EnvVarEditor } from "@/components/env-var-editor";
import type { AppSource, EnvVar, GitDeployMode, RunImage } from "@/lib/types";

type GitMode = GitDeployMode["type"];

export function CreateAppForm({ onCreated }: { onCreated: () => void }) {
  const createApp = useCreateApp();
  const [name, setName] = useState("");
  const [source, setSource] = useState<"upload" | "git">("upload");
  const [repoUrl, setRepoUrl] = useState("");
  const [branch, setBranch] = useState("");
  const [gitMode, setGitMode] = useState<GitMode>("static");

  const [publishDir, setPublishDir] = useState("");

  const [buildImage, setBuildImage] = useState<RunImage>("node24");
  const [buildCommand, setBuildCommand] = useState("");
  const [outputDir, setOutputDir] = useState("");

  const [runImage, setRunImage] = useState<RunImage>("node24");
  const [installCommand, setInstallCommand] = useState("");
  const [startCommand, setStartCommand] = useState("");
  const [containerPort, setContainerPort] = useState("");

  const [envVars, setEnvVars] = useState<EnvVar[]>([]);

  const pending = createApp.isPending;
  const error =
    createApp.error instanceof ApiError
      ? createApp.error.message
      : createApp.error && "Failed to create app";

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    let appSource: AppSource | undefined;
    if (source === "git") {
      let mode: GitDeployMode;
      if (gitMode === "run") {
        mode = {
          type: "run",
          image: runImage,
          install_command: installCommand.trim() || null,
          start_command: startCommand.trim(),
          container_port: Number(containerPort),
        };
      } else if (gitMode === "build") {
        mode = {
          type: "build",
          image: buildImage,
          command: buildCommand.trim(),
          output_dir: outputDir.trim(),
        };
      } else {
        mode = { type: "static", publish_dir: publishDir.trim() || null };
      }
      appSource = {
        type: "git",
        repo_url: repoUrl,
        branch: branch.trim() || "main",
        mode,
      };
    }
    const trimmedEnvVars = envVars
      .map((envVar) => ({ key: envVar.key.trim(), value: envVar.value }))
      .filter((envVar) => envVar.key !== "");
    try {
      await createApp.mutateAsync({ name, source: appSource, env_vars: trimmedEnvVars });
    } catch {
      return;
    }
    setName("");
    setRepoUrl("");
    setBranch("");
    setGitMode("static");
    setPublishDir("");
    setBuildCommand("");
    setOutputDir("");
    setInstallCommand("");
    setStartCommand("");
    setContainerPort("");
    setEnvVars([]);
    onCreated();
  }

  return (
    <Card className="max-w-xl">
      <CardHeader>
        <CardTitle>Create app</CardTitle>
      </CardHeader>
      <CardContent>
        <form onSubmit={handleSubmit} className="flex flex-col gap-4">
          <div className="flex flex-col gap-2">
            <Label htmlFor="app-name">App name</Label>
            <Input
              id="app-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="app-name"
              pattern="[a-z0-9-]+"
              required
            />
          </div>

          <div className="flex flex-col gap-2">
            <Label>Source</Label>
            <Select value={source} onValueChange={(value) => setSource(value as "upload" | "git")}>
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="upload">Upload zip</SelectItem>
                <SelectItem value="git">Git repo</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {source === "git" && (
            <>
              <div className="flex flex-col gap-2">
                <Label htmlFor="repo-url">Repo URL</Label>
                <Input
                  id="repo-url"
                  value={repoUrl}
                  onChange={(event) => setRepoUrl(event.target.value)}
                  placeholder="https://github.com/user/repo.git"
                  required
                />
              </div>
              <div className="flex flex-col gap-2">
                <Label htmlFor="branch">Branch</Label>
                <Input
                  id="branch"
                  value={branch}
                  onChange={(event) => setBranch(event.target.value)}
                  placeholder="main"
                />
              </div>

              <div className="flex flex-col gap-2">
                <Label>Deploy mode</Label>
                <Select value={gitMode} onValueChange={(value) => setGitMode(value as GitMode)}>
                  <SelectTrigger className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="static">Static (repo files are already the site)</SelectItem>
                    <SelectItem value="build">Build (run a command, serve its output)</SelectItem>
                    <SelectItem value="run">Run (long-lived process, not static files)</SelectItem>
                  </SelectContent>
                </Select>
              </div>

              {gitMode === "static" && (
                <div className="flex flex-col gap-2">
                  <Label htmlFor="publish-dir">Publish dir</Label>
                  <Input
                    id="publish-dir"
                    value={publishDir}
                    onChange={(event) => setPublishDir(event.target.value)}
                    placeholder="optional"
                  />
                </div>
              )}

              {gitMode === "build" && (
                <>
                  <div className="flex flex-col gap-2">
                    <Label>Image</Label>
                    <Select
                      value={buildImage}
                      onValueChange={(value) => setBuildImage(value as RunImage)}
                    >
                      <SelectTrigger className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="node24">node:24</SelectItem>
                        <SelectItem value="python314">python:3.14</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="flex flex-col gap-2">
                    <Label htmlFor="build-command">Build command</Label>
                    <Input
                      id="build-command"
                      value={buildCommand}
                      onChange={(event) => setBuildCommand(event.target.value)}
                      placeholder="e.g. npm run build"
                      required
                    />
                  </div>
                  <div className="flex flex-col gap-2">
                    <Label htmlFor="output-dir">Output dir</Label>
                    <Input
                      id="output-dir"
                      value={outputDir}
                      onChange={(event) => setOutputDir(event.target.value)}
                      placeholder="e.g. dist"
                      required
                    />
                  </div>
                </>
              )}

              {gitMode === "run" && (
                <>
                  <div className="flex flex-col gap-2">
                    <Label>Image</Label>
                    <Select
                      value={runImage}
                      onValueChange={(value) => setRunImage(value as RunImage)}
                    >
                      <SelectTrigger className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="node24">node:24</SelectItem>
                        <SelectItem value="python314">python:3.14</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="flex flex-col gap-2">
                    <Label htmlFor="install-command">Install command</Label>
                    <Input
                      id="install-command"
                      value={installCommand}
                      onChange={(event) => setInstallCommand(event.target.value)}
                      placeholder="optional, e.g. npm install"
                    />
                  </div>
                  <div className="flex flex-col gap-2">
                    <Label htmlFor="start-command">Start command</Label>
                    <Input
                      id="start-command"
                      value={startCommand}
                      onChange={(event) => setStartCommand(event.target.value)}
                      placeholder="e.g. npm start"
                      required
                    />
                  </div>
                  <div className="flex flex-col gap-2">
                    <Label htmlFor="container-port">Container port</Label>
                    <Input
                      id="container-port"
                      type="number"
                      min={1}
                      max={65535}
                      value={containerPort}
                      onChange={(event) => setContainerPort(event.target.value)}
                      placeholder="3000"
                      required
                    />
                  </div>
                </>
              )}

              {(gitMode === "run" || gitMode === "build") && (
                <EnvVarEditor envVars={envVars} onChange={setEnvVars} />
              )}
            </>
          )}

          {error && <p className="text-sm text-destructive">{error}</p>}
          <Button type="submit" disabled={pending} className="self-start">
            {pending ? "Creating…" : "Create app"}
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}
