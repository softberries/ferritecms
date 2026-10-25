// What a route shows when its loader or render fails (the router's
// `defaultErrorComponent`): inside the shell for routes under `_app`. A `403` is the
// permission-denied state, a `404` the not-found screen, anything else `ErrorState`
// with a retry (the problem's `detail`, never a stack trace).

import { useQueryErrorResetBoundary } from "@tanstack/react-query";
import { type ErrorComponentProps, useRouter } from "@tanstack/react-router";
import { ApiError } from "@/api/problem";
import { ErrorState } from "@/components/kit/error-state";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { NotFound } from "./not-found";

export function RouteError({ error, reset }: ErrorComponentProps) {
  const router = useRouter();
  const queryReset = useQueryErrorResetBoundary();
  if (error instanceof ApiError && error.status === 404) return <NotFound />;
  return (
    <div
      data-slot="route-error"
      className="mx-auto flex w-full max-w-xl flex-1 flex-col justify-center py-8"
    >
      {error instanceof ApiError && error.status === 403 ? (
        <ForbiddenState />
      ) : (
        <ErrorState
          error={error}
          retry={() => {
            queryReset.reset();
            reset();
            void router.invalidate();
          }}
        />
      )}
    </div>
  );
}
