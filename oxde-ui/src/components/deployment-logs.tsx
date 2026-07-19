import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { useApi } from "@/lib/api";
import { ApiError } from "@/lib/auth";

interface DeploymentLogsProps {
  appName: string;
  deploymentId: string;
  onClose: () => void;
}

type StreamState = "connecting" | "streaming" | "closed";

export function DeploymentLogs({ appName, deploymentId, onClose }: DeploymentLogsProps) {
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
  }, [api, appName, deploymentId, following]);

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
    <div className="flex flex-col gap-2 rounded-lg border p-3">
      <div className="flex items-center justify-between">
        <span className="text-sm font-medium">Logs - {deploymentId}</span>
        <div className="flex gap-2">
          <Button size="sm" variant="outline" onClick={() => setFollowing((prev) => !prev)}>
            {following ? "Stop live tail" : "Live tail"}
          </Button>
          <Button size="sm" variant="outline" onClick={onClose}>
            Close
          </Button>
        </div>
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
