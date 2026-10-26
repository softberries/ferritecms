import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, test, vi } from "vitest";
import { getContentRefs } from "@/api/content";
import { qk } from "@/api/keys";
import type { MenuNode } from "./menu-model";
import { useTargets } from "./targets";

vi.mock("@/api/content", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/content")>()),
  getContentRefs: vi.fn(),
}));

const EVENT = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
const GONE = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c";

const node = (id: string, targetId: string): MenuNode => ({
  id,
  parentId: null,
  data: { kind: "item", targetId, url: null, label: "" },
});

describe("menu_targets_test", () => {
  test("an_item_of_a_type_outside_the_nav_is_named_from_refs", async () => {
    vi.mocked(getContentRefs).mockResolvedValue([
      {
        id: EVENT,
        type: "event",
        title: "Spring fair",
        status: "published",
        permalink: "/event/spring-fair/",
      },
    ]);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, staleTime: Infinity } },
    });
    // A type the viewer's sidebar does not list (no nav entry is involved any more).
    client.setQueryData(qk.types, [{ name: "event", label: "Events" }]);
    client.setQueryData(qk.taxonomies, []);
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const nodes = [node("a", EVENT), node("b", GONE)];
    const { result } = renderHook(() => useTargets(nodes), { wrapper });
    await waitFor(() => expect(result.current.get("a")?.state).toBe("found"));
    expect(result.current.get("a")).toEqual({
      state: "found",
      title: "Spring fair",
      kind: "Events",
      status: "published",
    });
    // Absent from the answer (gone or unreadable): missing.
    expect(result.current.get("b")).toEqual({ state: "missing" });
    // Both looked up in one request.
    expect(getContentRefs).toHaveBeenCalledTimes(1);
    expect(vi.mocked(getContentRefs).mock.calls[0]?.[0]).toEqual([EVENT, GONE]);
  });
});
