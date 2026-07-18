import { createFileRoute, Link } from "@tanstack/react-router";
import { useCallback, useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
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
  const [showCreate, setShowCreate] = useState(false);

  const refresh = useCallback(() => {
    api
      .listApps()
      .then(setApps)
      .catch((err) => setError(err instanceof ApiError ? err.message : "Failed to load apps"));
  }, [api]);

  useEffect(refresh, [refresh]);

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center justify-between">
        <h1 className="font-heading text-2xl font-semibold">Apps</h1>
        <Button
          variant={showCreate ? "outline" : "default"}
          onClick={() => setShowCreate((v) => !v)}
        >
          {showCreate ? "Cancel" : "New app"}
        </Button>
      </div>

      {showCreate && (
        <CreateAppForm
          onCreated={() => {
            setShowCreate(false);
            refresh();
          }}
        />
      )}

      {error && <p className="text-sm text-destructive">{error}</p>}

      {apps && apps.length === 0 && !showCreate && (
        <div className="rounded-xl border border-dashed p-12 text-center text-muted-foreground">
          <p>No apps yet.</p>
          <Button className="mt-4" onClick={() => setShowCreate(true)}>
            Create your first app
          </Button>
        </div>
      )}

      {apps && apps.length > 0 && (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
          {apps.map((app) => (
            <Link key={app.name} to="/apps/$name" params={{ name: app.name }}>
              <Card className="h-full transition-colors hover:ring-primary/40">
                <CardHeader>
                  <CardTitle className="flex items-center justify-between gap-2">
                    <span className="truncate">{app.name}</span>
                    {app.source.type === "git" && (
                      <Badge variant="secondary" className="shrink-0">
                        git
                      </Badge>
                    )}
                  </CardTitle>
                </CardHeader>
                <CardContent>
                  {app.active_deployment_id ? (
                    <Badge>live</Badge>
                  ) : (
                    <Badge variant="outline">no active deployment</Badge>
                  )}
                </CardContent>
              </Card>
            </Link>
          ))}
        </div>
      )}
    </div>
  );
}
