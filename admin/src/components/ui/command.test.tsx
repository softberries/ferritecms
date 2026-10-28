import { act, render, screen } from "@testing-library/react";
import { beforeAll, expect, test } from "vitest";
import { Command, CommandGroup, CommandInput, CommandItem, CommandList } from "./command";

beforeAll(() => {
  // cmdk scrolls the selected option into view; jsdom has no layout.
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.scrollIntoView ??= () => {};
});

function Picker({ items, epoch }: { items: string[]; epoch: number }) {
  return (
    <Command shouldFilter={false}>
      <CommandInput placeholder="Search" />
      <CommandList>
        <CommandGroup heading="Results">
          {items.map((v) => (
            // `epoch` in the key remounts every option with the same value, as a refetch
            // that rebuilds the list does.
            <CommandItem key={`${epoch}-${v}`} value={v}>
              {v}
            </CommandItem>
          ))}
        </CommandGroup>
      </CommandList>
    </Command>
  );
}

const flush = () => act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));

test("active_descendant_follows_options_remounted_with_the_same_values", async () => {
  // cmdk 1.1.1 keeps the id of the option it selected first; when options remount, that
  // id names nothing, which axe reports (aria-valid-attr-value).
  const { rerender } = render(<Picker items={["alpha", "beta"]} epoch={0} />);
  await flush();
  rerender(<Picker items={["alpha", "beta"]} epoch={1} />);
  await flush();

  const selected = document.querySelector('[cmdk-item][aria-selected="true"]');
  expect(selected).not.toBeNull();
  for (const el of [screen.getByRole("combobox"), screen.getByRole("listbox")]) {
    const id = el.getAttribute("aria-activedescendant");
    expect(id).toBe(selected?.id);
    expect(document.getElementById(id ?? "")).not.toBeNull();
  }
});
