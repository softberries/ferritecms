import { act, renderHook } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { MediaDto } from "@/api/media";
import { useUploadQueue } from "./use-upload-queue";

const finish: ((m: MediaDto) => void)[] = [];
vi.mock("@/api/upload", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/upload")>()),
  uploadMedia: () => new Promise<MediaDto>((resolve) => finish.push(resolve)),
}));

test("on_done_is_registered_once_and_the_latest_is_called", async () => {
  const first = vi.fn();
  const second = vi.fn();
  const { result, rerender } = renderHook(({ onDone }) => useUploadQueue(onDone), {
    initialProps: { onDone: first },
  });
  const queue = result.current.queue;
  // Registrations from here on: a new callback each render must not re-register.
  const adds = vi.spyOn(Set.prototype, "add");
  act(() => {
    queue.add([new File(["x"], "a.png", { type: "image/png" })]);
  });
  // A new callback each render (as an inline arrow is): the queue stays the same one.
  rerender({ onDone: second });
  rerender({ onDone: second });
  expect(result.current.queue).toBe(queue);
  expect(adds.mock.calls.filter(([v]) => typeof v === "function")).toEqual([]);
  adds.mockRestore();
  const media = { id: "m1" } as MediaDto;
  await act(async () => {
    finish[0]?.(media);
    await Promise.resolve();
  });
  expect(first).not.toHaveBeenCalled();
  expect(second).toHaveBeenCalledTimes(1);
  expect(second).toHaveBeenCalledWith(media);
});
