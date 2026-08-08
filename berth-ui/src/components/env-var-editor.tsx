import type { EnvVarInput } from "@/lib/types";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

export function EnvVarEditor({
  envVars,
  onChange,
}: {
  envVars: EnvVarInput[];
  onChange: (envVars: EnvVarInput[]) => void;
}) {
  function update(index: number, patch: Partial<EnvVarInput>) {
    onChange(envVars.map((envVar, i) => (i === index ? { ...envVar, ...patch } : envVar)));
  }

  function toggleSecret(index: number, isSecret: boolean) {
    const envVar = envVars[index];
    const text = envVar.value.type === "secret" ? (envVar.value.value ?? "") : envVar.value.value;
    update(index, {
      value: isSecret ? { type: "secret", value: text } : { type: "plain", value: text },
    });
  }

  function change(index: number) {
    update(index, { value: { type: "secret", value: "" } });
  }

  function removeRow(index: number) {
    onChange(envVars.filter((_, i) => i !== index));
  }

  function addRow() {
    onChange([...envVars, { key: "", value: { type: "plain", value: "" } }]);
  }

  return (
    <div className="flex flex-col gap-2">
      {envVars.map((envVar, index) => {
        const masked = envVar.value.type === "secret" && envVar.value.value === null;
        return (
          <div key={index} className="flex items-center gap-2">
            <Input
              value={envVar.key}
              onChange={(event) => update(index, { key: event.target.value })}
              placeholder="KEY"
              className="font-mono"
            />
            {masked ? (
              <Input value="••••••••" disabled className="font-mono" />
            ) : (
              <Input
                value={envVar.value.value ?? ""}
                onChange={(event) =>
                  update(index, { value: { ...envVar.value, value: event.target.value } })
                }
                placeholder="value"
                className="font-mono"
              />
            )}
            <Label className="flex shrink-0 items-center gap-1.5 text-sm">
              <Checkbox
                checked={envVar.value.type === "secret"}
                disabled={masked}
                onCheckedChange={(checked) => toggleSecret(index, checked)}
              />
              Secret
            </Label>
            {masked && (
              <Button type="button" variant="outline" onClick={() => change(index)}>
                Change
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              onClick={() => removeRow(index)}
              aria-label="Remove"
            >
              &times;
            </Button>
          </div>
        );
      })}
      <Button type="button" variant="outline" onClick={addRow} className="self-start">
        Add variable
      </Button>
    </div>
  );
}
