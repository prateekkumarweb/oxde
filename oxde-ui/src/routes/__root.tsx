import { createRootRoute, Link, Outlet } from "@tanstack/react-router";
import { TanStackRouterDevtools } from "@tanstack/react-router-devtools";
import { LoginScreen } from "@/components/login-screen";
import { useAuth } from "@/lib/auth";

const RootLayout = () => {
  const { isAuthenticated, logout } = useAuth();

  return (
    <>
      {isAuthenticated ? (
        <div className="min-h-svh">
          <header className="flex items-center justify-between border-b px-4 py-3">
            <Link to="/" className="font-semibold">
              OxDe
            </Link>
            <button
              type="button"
              onClick={logout}
              className="text-sm text-muted-foreground hover:underline"
            >
              Sign out
            </button>
          </header>
          <main className="mx-auto max-w-4xl p-4">
            <Outlet />
          </main>
        </div>
      ) : (
        <LoginScreen />
      )}
      <TanStackRouterDevtools />
    </>
  );
};

export const Route = createRootRoute({ component: RootLayout });
