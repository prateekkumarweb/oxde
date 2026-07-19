import { createRootRoute, Link, Outlet } from "@tanstack/react-router";
import { TanStackRouterDevtools } from "@tanstack/react-router-devtools";
import { Moon, Sun } from "lucide-react";
import { LoginScreen } from "@/components/login-screen";
import { Button } from "@/components/ui/button";
import { useAuth } from "@/lib/auth";
import { useTheme } from "@/lib/theme";

const RootLayout = () => {
  const { isAuthenticated, logout } = useAuth();
  const { theme, toggleTheme } = useTheme();

  return (
    <>
      {isAuthenticated ? (
        <div className="min-h-svh">
          <header className="flex items-center justify-between border-b px-6 py-3">
            <Link to="/" className="flex items-center gap-2 font-heading text-lg font-semibold">
              <img src={`${import.meta.env.BASE_URL}icon.svg`} alt="" className="size-5" />
              OxDe
            </Link>
            <div className="flex items-center gap-2">
              <Button
                variant="ghost"
                size="icon-sm"
                onClick={toggleTheme}
                aria-label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
              >
                {theme === "dark" ? <Sun /> : <Moon />}
              </Button>
              <Button variant="ghost" size="sm" onClick={logout}>
                Sign out
              </Button>
            </div>
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
