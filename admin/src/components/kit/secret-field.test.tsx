import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { SecretField } from "./secret-field";

test("untouched_is_undefined", async () => {
  const onChange = vi.fn();
  render(<SecretField name="API key" set onChange={onChange} />);
  expect(screen.getByText("Set")).toBeInTheDocument();
  // Nothing is reported until the user acts; the form then leaves the key out.
  expect(onChange).not.toHaveBeenCalled();
  // Replace, then Cancel: back to untouched.
  await userEvent.click(screen.getByRole("button", { name: "Replace" }));
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onChange).toHaveBeenLastCalledWith(undefined);
  expect(screen.queryByLabelText("API key")).toBeNull();
});

test("replace_sends_the_string", async () => {
  const onChange = vi.fn();
  render(<SecretField name="API key" set onChange={onChange} />);
  await userEvent.click(screen.getByRole("button", { name: "Replace" }));
  const input = screen.getByLabelText("API key");
  // An empty password input, never pre-filled.
  expect(input).toHaveAttribute("type", "password");
  expect(input).toHaveValue("");
  expect(input).toHaveFocus();
  await userEvent.type(input, "s3cret");
  expect(onChange).toHaveBeenLastCalledWith("s3cret");
  // Cleared again: nothing to send.
  await userEvent.clear(input);
  expect(onChange).toHaveBeenLastCalledWith(undefined);
});

test("remove_sends_null", async () => {
  const onChange = vi.fn();
  render(<SecretField name="API key" set onChange={onChange} />);
  await userEvent.click(screen.getByRole("button", { name: "Remove" }));
  expect(onChange).toHaveBeenLastCalledWith(null);
  expect(screen.getByText("Removed when you save")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Undo" }));
  expect(onChange).toHaveBeenLastCalledWith(undefined);
  expect(screen.getByText("Set")).toBeInTheDocument();
});

test("not_set_offers_a_value_and_no_remove", async () => {
  const onChange = vi.fn();
  render(<SecretField name="API key" set={false} onChange={onChange} />);
  expect(screen.getByText("Not set")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Remove" })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Set a value" }));
  await userEvent.type(screen.getByLabelText("API key"), "k");
  expect(onChange).toHaveBeenLastCalledWith("k");
});

test("never_renders_a_value", async () => {
  const { container } = render(<SecretField name="API key" set onChange={() => undefined} />);
  // No input at all until Replace; then only an empty one.
  expect(container.querySelector("input")).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Replace" }));
  const inputs = container.querySelectorAll("input");
  expect(inputs).toHaveLength(1);
  expect(inputs[0]).toHaveAttribute("autocomplete", "new-password");
  expect(inputs[0]?.getAttribute("value") ?? "").toBe("");
});
