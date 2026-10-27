// A write that carries a password, a code or a reset token, run as a plain awaited call
// rather than a TanStack mutation: the mutation cache keeps a mutation's `variables`
// (the secret) for `gcTime` after it settles, and nothing here may outlive the form
// (Review Focus 3). Same shape as the `useMutation` it replaces: `mutate`, `isPending`,
// `onSuccess`, `onError`. A failure (of the call or of `onSuccess`) goes to `onError` and
// to the app's handler (`reportApiError`: a `401` still leads to sign in), as a
// mutation's did.

import { useRef, useState } from "react";
import { reportApiError } from "@/api/client";

export interface SecretCallOptions<A, R> {
  mutationFn: (args: A) => Promise<R>;
  onSuccess?: (result: R) => void | Promise<void>;
  onError?: (error: unknown) => void;
}

export interface SecretCall<A> {
  mutate: (args: A) => void;
  isPending: boolean;
}

export function useSecretCall<A, R>({
  mutationFn,
  onSuccess,
  onError,
}: SecretCallOptions<A, R>): SecretCall<A> {
  const [isPending, setPending] = useState(false);
  // Guards a second submit before the pending state has rendered.
  const running = useRef(false);
  const mutate = (args: A) => {
    if (running.current) return;
    running.current = true;
    setPending(true);
    void (async () => {
      try {
        // A throw in `onSuccess` (a follow-up read that fails, say) is a failure of the
        // call too: never an unhandled rejection.
        try {
          const result = await mutationFn(args);
          await onSuccess?.(result);
        } catch (error) {
          reportApiError(error);
          onError?.(error);
        }
      } finally {
        running.current = false;
        setPending(false);
      }
    })();
  };
  return { mutate, isPending };
}
