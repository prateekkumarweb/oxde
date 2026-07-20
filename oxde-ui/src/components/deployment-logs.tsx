import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useApi } from "@/lib/api";
import { ApiError } from "@/lib/auth";
import type { AppSource, LogKind } from "@/lib/types";

const PHASE_LABELS: Record<LogKind, string> = {
  clone: "Clone",
  install: "Install",
  build: "Build",
  run: "Run",
};

// Which log tabs apply to a deployment, derived from the app's source.
function relevantPhases(source: AppSource): LogKind[] {
  if (source.type !== "git") {
    return ["clone"];
  }
  switch (source.mode.type) {
    case "static":
      return ["clone"];
    case "build":
      return ["clone", "build"];
    case "run":
      return source.mode.install_command ? ["clone", "install", "run"] : ["clone", "run"];
    default:
      return ["clone"];
  }
}

interface DeploymentLogsProps {
  appName: string;
  deploymentId: string;
  source: AppSource;
  onClose: () => void;
}

export function DeploymentLogs({ appName, deploymentId, source, onClose }: DeploymentLogsProps) {
  const phases = relevantPhases(source);
  const defaultPhase = phases[phases.length - 1];

  return (
    <div className="flex flex-col gap-2 rounded-lg border p-3">
      <div className="flex items-center justify-between">
        <span className="text-sm font-medium">Logs - {deploymentId}</span>
        <Button size="sm" variant="outline" onClick={onClose}>
          Close
        </Button>
      </div>
      <Tabs defaultValue={defaultPhase}>
        <TabsList>
          {phases.map((phase) => (
            <TabsTrigger key={phase} value={phase}>
              {PHASE_LABELS[phase]}
            </TabsTrigger>
          ))}
        </TabsList>
        {phases.map((phase) => (
          <TabsContent key={phase} value={phase}>
            <DeploymentLogPane appName={appName} deploymentId={deploymentId} phase={phase} />
          </TabsContent>
        ))}
      </Tabs>
    </div>
  );
}

interface DeploymentLogPaneProps {
  appName: string;
  deploymentId: string;
  phase: LogKind;
}

type StreamState = "connecting" | "streaming" | "closed";

function DeploymentLogPane({ appName, deploymentId, phase }: DeploymentLogPaneProps) {
  const api = useApi();
  const [following, setFollowing] = useState(false);
  const [lines, setLines] = useState("");
  const [state, setState] = useState<StreamState>("connecting");
  const [error, setError] = useState<string | null>(null);
  const logRef = useRef<HTMLPreElement>(null);

  useEffect(() => {
    const controller = new AbortController();

    async function read() {
      setLines("");
      setError(null);
      setState("connecting");

      try {
        const response = await api.streamLogs(appName, deploymentId, {
          phase,
          follow: following,
          signal: controller.signal,
        });
        const reader = response.body?.getReader();
        if (!reader) {
          setState("closed");
          return;
        }
        const decoder = new TextDecoder();
        while (true) {
          // eslint-disable-next-line no-await-in-loop -- sequential stream reads, nothing to parallelize
          const { done, value } = await reader.read();
          if (done) {
            break;
          }
          setState("streaming");
          setLines((prev) => prev + decoder.decode(value, { stream: true }));
        }
        setState("closed");
      } catch (err) {
        if (controller.signal.aborted) {
          return;
        }
        setError(err instanceof ApiError ? err.message : "Failed to load logs");
        setState("closed");
      }
    }

    void read();
    return () => controller.abort();
  }, [api, appName, deploymentId, phase, following]);

  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [lines]);

  let placeholder: string;
  if (state === "connecting") {
    placeholder = "Connecting…";
  } else if (state === "streaming") {
    placeholder = following ? "Connected, waiting for new output…" : "";
  } else {
    placeholder = following ? "Stream closed." : "Stream closed - no output was returned.";
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex justify-end">
        <Button size="sm" variant="outline" onClick={() => setFollowing((prev) => !prev)}>
          {following ? "Stop live tail" : "Live tail"}
        </Button>
      </div>
      {error && <p className="text-sm text-destructive">{error}</p>}
      <pre
        ref={logRef}
        className="max-h-64 overflow-auto rounded bg-muted p-2 font-mono text-xs whitespace-pre-wrap"
      >
        {lines || <span className="text-muted-foreground italic">{placeholder}</span>}
      </pre>
    </div>
  );
}
