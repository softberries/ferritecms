import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect } from "react";
import { expect, test } from "vitest";
import type { StaleChoice } from "@/lib/stale-guard";
import { useStaleAsk } from "./use-stale-ask";

type Ask = () => Promise<StaleChoice>;

function Host({ expose }: { expose: (ask: Ask) => void }) {
  const { ask, dialog } = useStaleAsk("Menu main");
  useEffect(() => {
    expose(ask);
  }, [ask, expose]);
  return dialog;
}

function mount() {
  const box: { ask: Ask | null } = { ask: null };
  const view = render(
    <Host
      expose={(ask) => {
        box.ask = ask;
      }}
    />,
  );
  const ask = () => {
    if (!box.ask) throw new Error("not mounted");
    let p!: Promise<StaleChoice>;
    act(() => {
      p = box.ask!();
    });
    return p;
  };
  return { ask, unmount: view.unmount };
}

test("second_ask_settles_the_first", async () => {
  const { ask } = mount();
  const first = ask();
  const second = ask();
  // The first ask is cancelled at once; the dialog now answers the second.
  await expect(first).resolves.toBe("cancel");
  await userEvent.click(screen.getByRole("button", { name: "Overwrite" }));
  await expect(second).resolves.toBe("overwrite");
});

test("unmount_settles_a_pending_ask", async () => {
  const { ask, unmount } = mount();
  const pending = ask();
  expect(screen.getByText("Changed by someone else")).toBeInTheDocument();
  unmount();
  await expect(pending).resolves.toBe("cancel");
});
