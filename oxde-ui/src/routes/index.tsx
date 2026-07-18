import { createFileRoute, Link } from "@tanstack/react-router";
import { useCallback, useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { CreateAppForm } from "@/components/create-app-form";
import { useApi } from "@/lib/api";
import { ApiError } from "@/lib/auth";
import type { AppView } from "@/lib/types";

export const Route = createFileRoute("/")({
  component: AppsList,
});

function AppsList() {
  const api = useApi();
  const [apps, setApps] = useState<AppView[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api
      .listApps()
      .then(setApps)
      .catch((err) => setError(err instanceof ApiError ? err.message : "Failed to load apps"));
  }, [api]);

  useEffect(refresh, [refresh]);

  return (
    <div className="flex flex-col gap-6">
      <h1 className="text-2xl font-semibold">Apps</h1>

      <CreateAppForm onCreated={refresh} />

      {error && <p className="text-sm text-destructive">{error}</p>}

      {apps && apps.length === 0 && <p className="text-muted-foreground">No apps yet.</p>}

      {apps && apps.length > 0 && (
        <ul className="flex flex-col gap-2">
          {apps.map((app) => (
            <li key={app.name} className="flex items-center gap-2 rounded-lg border p-3">
              <Link
                to="/apps/$name"
                params={{ name: app.name }}
                className="font-medium hover:underline"
              >
                {app.name}
              </Link>
              {app.active_deployment_id ? (
                <Badge>live: {app.active_deployment_id}</Badge>
              ) : (
                <Badge variant="outline">no active deployment</Badge>
              )}
              {app.source.type === "git" && <Badge variant="secondary">git</Badge>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
