import { createFileRoute } from "@tanstack/react-router";
import { Check, Copy } from "lucide-react";
import { useState, type FormEvent } from "react";

import type { HostView } from "@/lib/types";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ApiError, useAuth } from "@/lib/auth";
import { useCreateHost, useHosts, useRevokeHost } from "@/lib/queries";

export const Route = createFileRoute("/hosts")({
  component: HostsPage,
});

function HostsPage() {
  const { user } = useAuth();
  const isAdmin = user?.role === "admin";
  const { data: hosts, error: queryError } = useHosts(isAdmin);
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
          <HostRow key={host.id} host={host} />
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

function HostRow({ host }: { host: HostView }) {
  const revokeHost = useRevokeHost();

  return (
    <div className="flex flex-col gap-1 rounded-lg border p-3">
      <div className="flex items-center justify-between gap-4">
        <div className="flex items-center gap-2">
          <span className="font-medium">{host.name}</span>
          {host.revoked ? (
            <Badge variant="destructive">Revoked</Badge>
          ) : (
            <Badge variant="secondary">Active</Badge>
          )}
        </div>
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
    </div>
  );
}
