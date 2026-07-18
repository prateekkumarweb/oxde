import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useApi } from "@/lib/api";
import { ApiError } from "@/lib/auth";
import type { AppSource, RunImage } from "@/lib/types";

export function CreateAppForm({ onCreated }: { onCreated: () => void }) {
  const api = useApi();
  const [name, setName] = useState("");
  const [source, setSource] = useState<"upload" | "git">("upload");
  const [repoUrl, setRepoUrl] = useState("");
  const [branch, setBranch] = useState("");
  const [publishDir, setPublishDir] = useState("");
  const [runEnabled, setRunEnabled] = useState(false);
  const [runImage, setRunImage] = useState<RunImage>("node24");
  const [installCommand, setInstallCommand] = useState("");
  const [startCommand, setStartCommand] = useState("");
  const [containerPort, setContainerPort] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setError(null);
    setPending(true);
    try {
      let appSource: AppSource | undefined;
      if (source === "git") {
        appSource = {
          type: "git",
          repo_url: repoUrl,
          branch: branch.trim() || "main",
          publish_dir: publishDir.trim() || null,
          run: runEnabled
            ? {
                image: runImage,
                install_command: installCommand.trim() || null,
                start_command: startCommand.trim(),
                container_port: Number(containerPort),
              }
            : null,
        };
      }
      await api.createApp({ name, source: appSource });
      setName("");
      setRepoUrl("");
      setBranch("");
      setPublishDir("");
      setRunEnabled(false);
      setInstallCommand("");
      setStartCommand("");
      setContainerPort("");
      onCreated();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Failed to create app");
    } finally {
      setPending(false);
    }
  }

  return (
    <Card>
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

              <div className="flex items-center gap-2">
                <Checkbox
                  id="run-enabled"
                  checked={runEnabled}
                  onCheckedChange={(checked) => setRunEnabled(checked === true)}
                />
                <Label htmlFor="run-enabled">Run mode (long-lived process, not static files)</Label>
              </div>

              {runEnabled ? (
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
              ) : (
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
            </>
          )}

          {error && <p className="text-sm text-destructive">{error}</p>}
          <Button type="submit" disabled={pending}>
            {pending ? "Creating…" : "Create app"}
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}
