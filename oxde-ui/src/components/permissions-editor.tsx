import type { AppPermission, PermissionLevel } from "@/lib/types";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

const PERMISSION_LEVEL_LABELS: Record<PermissionLevel, string> = { read: "Read", write: "Write" };

export function PermissionsEditor({
  permissions,
  onChange,
}: {
  permissions: AppPermission[];
  onChange: (permissions: AppPermission[]) => void;
}) {
  function updateUsername(index: number, username: string) {
    onChange(permissions.map((grant, i) => (i === index ? { ...grant, username } : grant)));
  }

  function updateLevel(index: number, level: PermissionLevel) {
    onChange(permissions.map((grant, i) => (i === index ? { ...grant, level } : grant)));
  }

  function removeRow(index: number) {
    onChange(permissions.filter((_, i) => i !== index));
  }

  function addRow() {
    onChange([...permissions, { username: "", level: "read" }]);
  }

  return (
    <div className="flex flex-col gap-2">
      <Label>Collaborators</Label>
      {permissions.map((grant, index) => (
        <div key={index} className="flex gap-2">
          <Input
            value={grant.username}
            onChange={(event) => updateUsername(index, event.target.value)}
            placeholder="username"
          />
          <Select
            value={grant.level}
            onValueChange={(value) => updateLevel(index, value === "write" ? "write" : "read")}
          >
            <SelectTrigger>
              <SelectValue>
                {(value: string) =>
                  (value === "read" || value === "write" ? PERMISSION_LEVEL_LABELS[value] : null) ??
                  value
                }
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="read">Read</SelectItem>
              <SelectItem value="write">Write</SelectItem>
            </SelectContent>
          </Select>
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
        Add collaborator
      </Button>
    </div>
  );
}
