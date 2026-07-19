import { createRootRoute, Link, Outlet } from "@tanstack/react-router";
import { TanStackRouterDevtools } from "@tanstack/react-router-devtools";
import { LoginScreen } from "@/components/login-screen";
import { Button } from "@/components/ui/button";
import { useAuth } from "@/lib/auth";

const RootLayout = () => {
  const { isAuthenticated, logout } = useAuth();

  return (
    <>
      {isAuthenticated ? (
        <div className="min-h-svh">
          <header className="flex items-center justify-between border-b px-6 py-3">
            <Link to="/" className="flex items-center gap-2 font-heading text-lg font-semibold">
              <img src={`${import.meta.env.BASE_URL}icon.svg`} alt="" className="size-5" />
              OxDe
            </Link>
            <Button variant="ghost" size="sm" onClick={logout}>
              Sign out
            </Button>
          </header>
          <main className="p-6">
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
