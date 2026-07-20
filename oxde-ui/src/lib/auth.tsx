import {
  createContext,
  use,
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import type { UserView } from "@/lib/generated/UserView";

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, error: string) {
    super(`Error: ${error}`);
    this.name = "ApiError";
    this.status = status;
  }
}

async function errorFromResponse(response: Response): Promise<string> {
  const body = await response.text();
  try {
    const parsed: unknown = body ? JSON.parse(body) : null;
    if (
      parsed &&
      typeof parsed === "object" &&
      "error" in parsed &&
      typeof parsed.error === "string"
    ) {
      return parsed.error;
    }
  } catch {
    // Not a JSON error body - fall through to the raw text/status below.
  }
  return body || response.statusText;
}

async function doRequest<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/api${path}`, init);

  if (!response.ok) {
    throw new ApiError(response.status, await errorFromResponse(response));
  }
  const body = await response.text();
  const parsed: unknown = body ? JSON.parse(body) : undefined;
  // T is ts-rs-generated from the Rust response struct; trusted, not verifiable at runtime here.
  // eslint-disable-next-line no-unsafe-type-assertion
  return parsed as T;
}

interface AuthContextValue {
  login: (username: string, password: string) => Promise<void>;
  logout: () => void;
  request: <T>(path: string, init?: RequestInit) => Promise<T>;
  requestStream: (path: string, init?: RequestInit) => Promise<Response>;
  isAuthenticated: boolean;
  /** `null` until the initial session check (`GET /api/me`) resolves. */
  user: UserView | null;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<UserView | null>(null);
  const [checkedSession, setCheckedSession] = useState(false);

  useEffect(() => {
    doRequest<UserView>("/me")
      .then(setUser)
      .catch(() => setUser(null))
      .finally(() => setCheckedSession(true));
  }, []);

  const logout = useCallback(() => {
    setUser(null);
    void fetch("/api/logout", { method: "POST" });
  }, []);

  const login = useCallback(async (username: string, password: string) => {
    const loggedInUser = await doRequest<UserView>("/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    });
    setUser(loggedInUser);
  }, []);

  const request = useCallback(async <T,>(path: string, init?: RequestInit): Promise<T> => {
    try {
      return await doRequest<T>(path, init);
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        setUser(null);
      }
      throw error;
    }
  }, []);

  const requestStream = useCallback(async (path: string, init?: RequestInit): Promise<Response> => {
    const response = await fetch(`/api${path}`, init);
    if (!response.ok) {
      const error = await errorFromResponse(response);
      if (response.status === 401) {
        setUser(null);
      }
      throw new ApiError(response.status, error);
    }
    return response;
  }, []);

  const value = useMemo(
    () => ({ login, logout, request, requestStream, isAuthenticated: user != null, user }),
    [login, logout, request, requestStream, user],
  );

  // Avoid flashing the login screen while the initial `/api/me` check is
  // still in flight - render nothing until it resolves once.
  if (!checkedSession) {
    return null;
  }

  return <AuthContext value={value}>{children}</AuthContext>;
}

export function useAuth(): AuthContextValue {
  const ctx = use(AuthContext);
  if (!ctx) {
    throw new Error("useAuth must be used within AuthProvider");
  }
  return ctx;
}
