import { createFileRoute } from "@tanstack/react-router";
import { Check, Copy } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ApiError } from "@/lib/auth";
import { useApiTokens, useCreateApiToken, useRevokeApiToken } from "@/lib/queries";
import type { ApiTokenView } from "@/lib/types";

export const Route = createFileRoute("/settings")({
  component: SettingsPage,
});

function defaultExpiryLocal(): string {
  const inOneDay = new Date(Date.now() + 24 * 60 * 60 * 1000);
  inOneDay.setSeconds(0, 0);
  // `datetime-local` inputs want local time with no timezone suffix.
  const offsetMs = inOneDay.getTimezoneOffset() * 60 * 1000;
  return new Date(inOneDay.getTime() - offsetMs).toISOString().slice(0, 16);
}

function SettingsPage() {
  const { data: tokens, error: queryError } = useApiTokens();
  const error =
    queryError instanceof ApiError ? queryError.message : queryError && "Failed to load tokens";

  return (
    <div className="flex flex-col gap-6">
      <h1 className="font-heading text-2xl font-semibold">Settings</h1>

      <div className="flex flex-col gap-3">
        <h2 className="text-lg font-medium">API tokens</h2>
        {error && <p className="text-sm text-destructive">{error}</p>}

        <CreateApiTokenForm />

        <div className="flex flex-col gap-3">
          {tokens?.map((token) => (
            <ApiTokenRow key={token.id} token={token} />
          ))}
        </div>
      </div>
    </div>
  );
}

function CreateApiTokenForm() {
  const createToken = useCreateApiToken();
  const [name, setName] = useState("");
  const [expiresAt, setExpiresAt] = useState(defaultExpiryLocal);
  const [plaintextToken, setPlaintextToken] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    createToken.mutate(
      { name, expires_at: Math.floor(new Date(expiresAt).getTime() / 1000) },
      {
        onSuccess: (response) => {
          setName("");
          setExpiresAt(defaultExpiryLocal());
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
        <CardTitle>New token</CardTitle>
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
            <Label htmlFor="token-name">Name</Label>
            <Input
              id="token-name"
              placeholder="laptop, CI script, ..."
              value={name}
              onChange={(event) => setName(event.target.value)}
              required
            />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="token-expires-at">Expires</Label>
            <Input
              id="token-expires-at"
              type="datetime-local"
              value={expiresAt}
              onChange={(event) => setExpiresAt(event.target.value)}
              required
            />
          </div>
          <Button type="submit" disabled={createToken.isPending}>
            {createToken.isPending ? "Creating…" : "Create token"}
          </Button>
          {createToken.error && (
            <p className="w-full text-sm text-destructive">
              {createToken.error instanceof ApiError
                ? createToken.error.message
                : "Failed to create token"}
            </p>
          )}
        </form>
      </CardContent>
    </Card>
  );
}

function ApiTokenRow({ token }: { token: ApiTokenView }) {
  const revokeToken = useRevokeApiToken();
  const [now] = useState(() => Date.now());
  const expired = token.expires_at * 1000 < now;

  return (
    <div className="flex flex-col gap-1 rounded-lg border p-3">
      <div className="flex items-center justify-between gap-4">
        <div className="flex items-center gap-2">
          <span className="font-medium">{token.name}</span>
          {token.revoked ? (
            <Badge variant="destructive">Revoked</Badge>
          ) : expired ? (
            <Badge variant="outline">Expired</Badge>
          ) : (
            <Badge variant="secondary">Active</Badge>
          )}
        </div>
        <Button
          variant="outline"
          size="sm"
          disabled={token.revoked || revokeToken.isPending}
          onClick={() => revokeToken.mutate(token.id)}
        >
          Revoke
        </Button>
      </div>
      <p className="text-sm text-muted-foreground">
        Created {new Date(token.created_at * 1000).toLocaleString()} - Expires{" "}
        {new Date(token.expires_at * 1000).toLocaleString()}
      </p>
      {revokeToken.error && (
        <p className="text-sm text-destructive">
          {revokeToken.error instanceof ApiError ? revokeToken.error.message : "Action failed"}
        </p>
      )}
    </div>
  );
}
