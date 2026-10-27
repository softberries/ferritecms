import { act, renderHook } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { MediaDto } from "@/api/media";
import { useUploadQueue } from "./use-upload-queue";

const finish: ((m: MediaDto) => void)[] = [];
vi.mock("@/api/upload", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/upload")>()),
  uploadMedia: () => new Promise<MediaDto>((resolve) => finish.push(resolve)),
}));

test("on_done_is_the_latest_callback_called_once", async () => {
  const first = vi.fn();
  const second = vi.fn();
  const { result, rerender } = renderHook(({ onDone }) => useUploadQueue(onDone), {
    initialProps: { onDone: first },
  });
  const queue = result.current.queue;
  act(() => {
    queue.add([new File(["x"], "a.png", { type: "image/png" })]);
  });
  // A new callback each render (as an inline arrow is): the queue stays the same one.
  rerender({ onDone: second });
  rerender({ onDone: second });
  expect(result.current.queue).toBe(queue);
  const media = { id: "m1" } as MediaDto;
  await act(async () => {
    finish[0]?.(media);
    await Promise.resolve();
  });
  expect(first).not.toHaveBeenCalled();
  expect(second).toHaveBeenCalledTimes(1);
  expect(second).toHaveBeenCalledWith(media);
});
