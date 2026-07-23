import type { EnvVar } from "@/lib/types";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

export function EnvVarEditor({
  envVars,
  onChange,
}: {
  envVars: EnvVar[];
  onChange: (envVars: EnvVar[]) => void;
}) {
  function updateKey(index: number, key: string) {
    onChange(envVars.map((envVar, i) => (i === index ? { ...envVar, key } : envVar)));
  }

  function updateValue(index: number, value: string) {
    onChange(envVars.map((envVar, i) => (i === index ? { ...envVar, value } : envVar)));
  }

  function removeRow(index: number) {
    onChange(envVars.filter((_, i) => i !== index));
  }

  function addRow() {
    onChange([...envVars, { key: "", value: "" }]);
  }

  return (
    <div className="flex flex-col gap-2">
      <Label>Environment variables</Label>
      {envVars.map((envVar, index) => (
        <div key={index} className="flex gap-2">
          <Input
            value={envVar.key}
            onChange={(event) => updateKey(index, event.target.value)}
            placeholder="KEY"
            className="font-mono"
          />
          <Input
            value={envVar.value}
            onChange={(event) => updateValue(index, event.target.value)}
            placeholder="value"
            className="font-mono"
          />
          <Button
            type="button"
            variant="outline"
            onClick={() => removeRow(index)}
            aria-label="Remove"
          >
            &times;
          </Button>
        </div>
      ))}
      <Button type="button" variant="outline" onClick={addRow} className="self-start">
        Add variable
      </Button>
    </div>
  );
}
