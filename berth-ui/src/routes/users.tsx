import { createFileRoute } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ApiError } from "@/lib/auth";
import { useAuth } from "@/lib/auth";
import { useCreateUser, useDeleteUser, useUpdateUser, useUsers } from "@/lib/queries";

const ROLE_LABELS: Record<string, string> = { member: "Member", admin: "Admin" };

export const Route = createFileRoute("/users")({
  component: UsersPage,
});

function UsersPage() {
  const { user: currentUser } = useAuth();
  const { data: users, error: queryError } = useUsers();
  const error =
    queryError instanceof ApiError ? queryError.message : queryError && "Failed to load users";

  if (currentUser?.role !== "admin") {
    return <p className="text-sm text-muted-foreground">Only admins can manage users.</p>;
  }

  return (
    <div className="flex flex-col gap-6">
      <h1 className="font-heading text-2xl font-semibold">Users</h1>
      {error && <p className="text-sm text-destructive">{error}</p>}

      <CreateUserForm />

      <div className="flex flex-col gap-3">
        {users?.map((user) => (
          <UserRow key={user.username} username={user.username} role={user.role} />
        ))}
      </div>
    </div>
  );
}

function CreateUserForm() {
  const createUser = useCreateUser();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState("member");

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    createUser.mutate(
      { username, password, role },
      {
        onSuccess: () => {
          setUsername("");
          setPassword("");
          setRole("member");
        },
      },
    );
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>New user</CardTitle>
      </CardHeader>
      <CardContent>
        <form onSubmit={handleSubmit} className="flex flex-wrap items-end gap-3">
          <div className="flex flex-col gap-2">
            <Label htmlFor="new-username">Username</Label>
            <Input
              id="new-username"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              required
            />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="new-password">Password</Label>
            <Input
              id="new-password"
              type="password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              required
            />
          </div>
          <div className="flex flex-col gap-2">
            <Label>Role</Label>
            <Select value={role} onValueChange={(value) => value && setRole(value)}>
              <SelectTrigger>
                <SelectValue>{(value: string) => ROLE_LABELS[value] ?? value}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="member">Member</SelectItem>
                <SelectItem value="admin">Admin</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <Button type="submit" disabled={createUser.isPending}>
            {createUser.isPending ? "Creating…" : "Create user"}
          </Button>
          {createUser.error && (
            <p className="w-full text-sm text-destructive">
              {createUser.error instanceof ApiError
                ? createUser.error.message
                : "Failed to create user"}
            </p>
          )}
        </form>
      </CardContent>
    </Card>
  );
}

function UserRow({ username, role }: { username: string; role: string }) {
  const { user: currentUser } = useAuth();
  const updateUser = useUpdateUser();
  const deleteUser = useDeleteUser();
  const isSelf = currentUser?.username === username;
  const error = updateUser.error ?? deleteUser.error;
  const [showResetPassword, setShowResetPassword] = useState(false);

  return (
    <div className="flex flex-col gap-3 rounded-lg border p-3">
      <div className="flex items-center justify-between gap-4">
        <div className="flex items-center gap-2">
          <span className="font-medium">{username}</span>
          <Badge variant={role === "admin" ? "default" : "outline"}>{role}</Badge>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={updateUser.isPending}
            onClick={() =>
              updateUser.mutate({ username, role: role === "admin" ? "member" : "admin" })
            }
          >
            Make {role === "admin" ? "member" : "admin"}
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={isSelf}
            title={isSelf ? "Change your own password from Settings" : undefined}
            onClick={() => setShowResetPassword((v) => !v)}
          >
            Reset password
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={isSelf || deleteUser.isPending}
            title={isSelf ? "You can't delete your own account" : undefined}
            onClick={() => deleteUser.mutate(username)}
          >
            Delete
          </Button>
        </div>
      </div>
      {showResetPassword && !isSelf && (
        <ResetPasswordForm username={username} onDone={() => setShowResetPassword(false)} />
      )}
      {error && (
        <p className="text-sm text-destructive">
          {error instanceof ApiError ? error.message : "Action failed"}
        </p>
      )}
    </div>
  );
}

function ResetPasswordForm({ username, onDone }: { username: string; onDone: () => void }) {
  const updateUser = useUpdateUser();
  const [password, setPassword] = useState("");

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    updateUser.mutate({ username, password }, { onSuccess: onDone });
  }

  return (
    <form onSubmit={handleSubmit} className="flex flex-wrap items-end gap-3">
      <div className="flex flex-col gap-2">
        <Label htmlFor={`reset-password-${username}`}>New password</Label>
        <Input
          id={`reset-password-${username}`}
          type="password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          required
        />
      </div>
      <Button type="submit" size="sm" disabled={updateUser.isPending}>
        {updateUser.isPending ? "Setting…" : "Set password"}
      </Button>
    </form>
  );
}
