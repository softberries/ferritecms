import { expect, test, vi } from "vitest";
import { staleGuard } from "./stale-guard";

test("equal_proceeds_without_asking", async () => {
  const ask = vi.fn();
  const out = await staleGuard({
    reread: () => Promise.resolve({ a: 1, b: [1, 2] }),
    base: { a: 1, b: [1, 2] },
    ask,
  });
  expect(out).toEqual({ proceed: true });
  expect(ask).not.toHaveBeenCalled();
});

test("key_order_does_not_matter", async () => {
  const ask = vi.fn();
  const out = await staleGuard({
    reread: () => Promise.resolve({ a: 1, n: { x: 1, y: 2 } }),
    base: { n: { y: 2, x: 1 }, a: 1 },
    ask,
  });
  expect(out).toEqual({ proceed: true });
  expect(ask).not.toHaveBeenCalled();
});

test("differs_asks_with_their_copy", async () => {
  const ask = vi.fn(() => Promise.resolve("overwrite" as const));
  const out = await staleGuard({
    reread: () => Promise.resolve({ a: 2 }),
    base: { a: 1 },
    ask,
  });
  expect(ask).toHaveBeenCalledWith({ a: 2 });
  expect(out).toEqual({ proceed: true });
});

test("reload_returns_the_fresh_copy", async () => {
  const out = await staleGuard({
    reread: () => Promise.resolve({ a: 2 }),
    base: { a: 1 },
    ask: () => Promise.resolve("reload"),
  });
  expect(out).toEqual({ proceed: false, reloaded: { a: 2 } });
});

test("cancel_stops_without_a_copy", async () => {
  const out = await staleGuard({
    reread: () => Promise.resolve({ a: 2 }),
    base: { a: 1 },
    ask: () => Promise.resolve("cancel"),
  });
  expect(out).toEqual({ proceed: false });
});

test("custom_equal_is_used", async () => {
  const ask = vi.fn();
  const out = await staleGuard({
    reread: () => Promise.resolve({ a: 2, seen: 9 }),
    base: { a: 2, seen: 1 },
    equal: (x, y) => x.a === y.a,
    ask,
  });
  expect(out).toEqual({ proceed: true });
  expect(ask).not.toHaveBeenCalled();
});
