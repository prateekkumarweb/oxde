import { createContext, use, useCallback, useMemo, useState, type ReactNode } from "react";

const STORAGE_KEY = "oxde-auth";

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

function basicAuthHeader(username: string, password: string): string {
  return `Basic ${btoa(`${username}:${password}`)}`;
}

async function doRequest<T>(authHeader: string, path: string, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers);
  headers.set("Authorization", authHeader);
  const response = await fetch(`/api${path}`, { ...init, headers });

  const body = await response.text();
  if (!response.ok) {
    throw new ApiError(response.status, body || response.statusText);
  }
  return (body ? JSON.parse(body) : undefined) as T;
}

interface AuthContextValue {
  login: (username: string, password: string) => Promise<void>;
  logout: () => void;
  request: <T>(path: string, init?: RequestInit) => Promise<T>;
  isAuthenticated: boolean;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [authHeader, setAuthHeader] = useState<string | null>(() =>
    sessionStorage.getItem(STORAGE_KEY),
  );

  const logout = useCallback(() => {
    sessionStorage.removeItem(STORAGE_KEY);
    setAuthHeader(null);
  }, []);

  const login = useCallback(async (username: string, password: string) => {
    const header = basicAuthHeader(username, password);
    await doRequest(header, "/apps");
    sessionStorage.setItem(STORAGE_KEY, header);
    setAuthHeader(header);
  }, []);

  const request = useCallback(
    async <T,>(path: string, init?: RequestInit): Promise<T> => {
      if (!authHeader) {
        throw new ApiError(401, "not authenticated");
      }
      try {
        return await doRequest<T>(authHeader, path, init);
      } catch (error) {
        if (error instanceof ApiError && error.status === 401) {
          logout();
        }
        throw error;
      }
    },
    [authHeader, logout],
  );

  const value = useMemo(
    () => ({ login, logout, request, isAuthenticated: authHeader != null }),
    [login, logout, request, authHeader],
  );

  return <AuthContext value={value}>{children}</AuthContext>;
}

export function useAuth(): AuthContextValue {
  const ctx = use(AuthContext);
  if (!ctx) {
    throw new Error("useAuth must be used within AuthProvider");
  }
  return ctx;
}
