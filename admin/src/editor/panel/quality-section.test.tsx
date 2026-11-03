import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import type { CheckResponse, FindingDto } from "@/api/quality";
import { AltEditor } from "../image-node";
import { QualitySection } from "./quality-section";

const finding = (over: Partial<FindingDto>): FindingDto => ({
  rule: "image-alt-missing",
  severity: "error",
  category: "accessibility",
  wcag: ["1.1.1"],
  location: { kind: "body", path: [0] },
  key: "/a.png",
  context: null,
  detail: {},
  suppressed: null,
  ...over,
});

const report = (findings: FindingDto[]): CheckResponse => ({
  findings,
  counts: {
    error: findings.filter((f) => !f.suppressed && f.severity === "error").length,
    warning: findings.filter((f) => !f.suppressed && f.severity === "warning").length,
    notice: 0,
    suppressed: findings.filter((f) => f.suppressed).length,
  },
  gate: { mode: "off", blocked: false },
});

function renderSection(r: CheckResponse | null, over: Record<string, unknown> = {}) {
  const props = {
    report: r,
    failed: false,
    canSuppress: true,
    onShow: vi.fn(),
    onSuppress: vi.fn(() => Promise.resolve()),
    onUnsuppress: vi.fn(() => Promise.resolve()),
    ...over,
  };
  render(<QualitySection {...props} />);
  return props;
}

describe("quality_section", () => {
  test("groups by severity, counts in a live region, suppressed collapsed", async () => {
    renderSection(
      report([
        finding({ rule: "link-text-vague", severity: "warning", key: "w", context: "here" }),
        finding({ key: "e" }),
        finding({
          key: "s",
          suppressed: {
            rule: "image-alt-missing",
            key: "s",
            reason: "divider",
            user_id: null,
            created_at: "",
          },
        }),
      ]),
    );
    const counts = screen.getByText(/1 errors, 1 warnings/);
    expect(counts).toHaveAttribute("aria-live", "polite");
    const lists = screen.getAllByRole("list");
    expect(within(lists[0]!).getByText("Image without alt text")).toBeInTheDocument();
    expect(within(lists[1]!).getByText(/does not say where it leads/)).toBeInTheDocument();
    expect(screen.queryByText(/divider/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Suppressed (1)" }));
    expect(screen.getByText(/divider/)).toBeInTheDocument();
  });

  test("Show and the suppress dialog's required reason", async () => {
    const props = renderSection(report([finding({})]));
    await userEvent.click(screen.getByRole("button", { name: "Add alt text" }));
    expect(props.onShow).toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Suppress…" }));
    const dialog = await screen.findByRole("dialog");
    const submit = within(dialog).getByRole("button", { name: "Suppress" });
    expect(submit).toBeDisabled();
    await userEvent.type(within(dialog).getByLabelText("Why"), "  decorative divider ");
    expect(submit).toBeEnabled();
    await userEvent.click(submit);
    expect(props.onSuppress).toHaveBeenCalledWith(expect.objectContaining({ key: "/a.png" }), {
      key: "/a.png",
      reason: "decorative divider",
    });
  });

  test("states: waiting, none, failed", () => {
    renderSection(null);
    expect(screen.getByText("Checking…")).toBeInTheDocument();
  });

  test("the decorative switch writes decorative and clears alt", async () => {
    const onChange = vi.fn();
    render(<AltEditor alt="A lake" decorative={false} onChange={onChange} />);
    await userEvent.click(screen.getByRole("button", { name: /Edit the alt text/ }));
    await userEvent.click(await screen.findByRole("switch", { name: "Decorative image" }));
    await userEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(onChange).toHaveBeenCalledWith({ alt: "", decorative: true });
  });

  test("an empty alt without the flag says No alt text", () => {
    render(<AltEditor alt="" decorative={false} onChange={vi.fn()} />);
    expect(screen.getByRole("button", { name: /No alt text/ })).toBeInTheDocument();
  });
});
