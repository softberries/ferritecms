import { act, renderHook, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { useSecretCall } from "./use-secret-call";

test("on_success_throw_goes_to_on_error", async () => {
  const boom = new Error("follow-up failed");
  const onError = vi.fn();
  const unhandled = vi.fn();
  window.addEventListener("unhandledrejection", unhandled);
  try {
    const { result } = renderHook(() =>
      useSecretCall({
        mutationFn: (code: string) => Promise.resolve(code.length),
        onSuccess: () => {
          throw boom;
        },
        onError,
      }),
    );
    act(() => result.current.mutate("123456"));
    await waitFor(() => expect(onError).toHaveBeenCalledWith(boom));
    expect(onError).toHaveBeenCalledTimes(1);
    // Settled: not pending, and a second call runs again.
    await waitFor(() => expect(result.current.isPending).toBe(false));
    act(() => result.current.mutate("654321"));
    await waitFor(() => expect(onError).toHaveBeenCalledTimes(2));
    await new Promise((r) => setTimeout(r, 0));
    expect(unhandled).not.toHaveBeenCalled();
  } finally {
    window.removeEventListener("unhandledrejection", unhandled);
  }
});

test("call_failure_goes_to_on_error_and_skips_on_success", async () => {
  const onSuccess = vi.fn();
  const onError = vi.fn();
  const { result } = renderHook(() =>
    useSecretCall({
      mutationFn: () => Promise.reject(new Error("refused")),
      onSuccess,
      onError,
    }),
  );
  act(() => result.current.mutate(undefined));
  await waitFor(() => expect(onError).toHaveBeenCalledTimes(1));
  expect(onSuccess).not.toHaveBeenCalled();
});
