import { Link } from "@tanstack/react-router";

import { Button } from "@/components/ui/button";

export function NotFound() {
  return (
    <div className="flex flex-col items-center justify-center gap-4 py-24 text-center">
      <p className="font-heading text-sm text-muted-foreground">404</p>
      <h1 className="font-heading text-2xl font-semibold">Page not found</h1>
      <p className="max-w-sm text-sm text-muted-foreground">
        The page you're looking for doesn't exist or may have been moved.
      </p>
      <Button render={<Link to="/" />} className="mt-2">
        Back to apps
      </Button>
    </div>
  );
}
