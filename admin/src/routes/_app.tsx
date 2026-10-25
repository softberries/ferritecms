// The signed-in area: every route under `_app` passes the login guard first (pre-flight
// F19; Task 10 turns this into the shell layout). No session → `/login?next=…`; a
// session that must change its password → `/change-password` (Review Focus 4).

import { Outlet, createFileRoute, useRouter } from "@tanstack/react-router";
import { ErrorState } from "@/components/kit/error-state";
import { ensureSession } from "@/auth/session";

export const Route = createFileRoute("/_app")({
  beforeLoad: async (ctx) => ({ me: await ensureSession(ctx) }),
  component: Outlet,
  errorComponent: AppError,
});

/** The guard could not ask the server (offline, 5xx): say so and offer a retry. */
function AppError({ error }: { error: unknown }) {
  const router = useRouter();
  return (
    <main id="main" className="mx-auto flex min-h-svh max-w-xl flex-col justify-center p-page">
      <ErrorState
        error={error}
        retry={() => {
          void router.invalidate();
        }}
      />
    </main>
  );
}
