import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { expect, test, vi } from "vitest";
import { CodeInput } from "./code-input";

function Host({ onComplete }: { onComplete: (code: string) => void }) {
  const [code, setCode] = useState("");
  return (
    <CodeInput
      id="enrol-code"
      label="Code from the app"
      value={code}
      onChange={setCode}
      onComplete={onComplete}
    />
  );
}

test("code_input_keeps_six_digits_and_completes", async () => {
  const onComplete = vi.fn();
  render(<Host onComplete={onComplete} />);
  const input = screen.getByLabelText("Code from the app");
  expect(input).toHaveAttribute("id", "enrol-code");
  expect(input).toHaveAttribute("inputmode", "numeric");
  expect(input).toHaveAttribute("autocomplete", "one-time-code");
  await userEvent.type(input, "12a3 45");
  expect(input).toHaveValue("12345");
  expect(onComplete).not.toHaveBeenCalled();
  await userEvent.type(input, "67");
  expect(input).toHaveValue("123456");
  expect(onComplete).toHaveBeenCalledExactlyOnceWith("123456");
});
