import { describe, expect, it, vi } from "vitest";
import { consumeResetToken, holdResetToken, takeResetToken } from "./reset-token";

describe("take_reset_token_strips_hash", () => {
  it("returns the token and drops the fragment", () => {
    const history = { replaceState: vi.fn() };
    const token = takeResetToken(
      { pathname: "/admin/reset-password", search: "", hash: "#token=abc.DEF_123-x" },
      history,
    );
    expect(token).toBe("abc.DEF_123-x");
    expect(history.replaceState).toHaveBeenCalledTimes(1);
    expect(history.replaceState).toHaveBeenCalledWith(null, "", "/admin/reset-password");
    const url = history.replaceState.mock.calls[0]?.[2] as string;
    expect(url).not.toContain("#");
    expect(url).not.toContain("abc");
  });

  it("keeps the query", () => {
    const history = { replaceState: vi.fn() };
    takeResetToken(
      { pathname: "/admin/reset-password", search: "?a=1", hash: "#token=t" },
      history,
    );
    expect(history.replaceState).toHaveBeenCalledWith(null, "", "/admin/reset-password?a=1");
  });

  it("strips the fragment without a token too", () => {
    for (const hash of ["", "#", "#other=1", "#token="]) {
      const history = { replaceState: vi.fn() };
      expect(
        takeResetToken({ pathname: "/admin/reset-password", search: "", hash }, history),
      ).toBeNull();
      expect(history.replaceState).toHaveBeenCalledTimes(1);
      expect(history.replaceState).toHaveBeenCalledWith(null, "", "/admin/reset-password");
    }
  });

  it("hands the token over once", () => {
    holdResetToken("secret");
    expect(consumeResetToken()).toBe("secret");
    expect(consumeResetToken()).toBeNull();
  });
});
