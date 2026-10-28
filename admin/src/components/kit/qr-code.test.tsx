import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { QrCode } from "./qr-code";
import { QR_QUIET_ZONE, qrMatrix } from "./qr-matrix";

// What `POST /auth/totp/setup` answers (`otpauth_url`), with a made-up secret.
const OTPAUTH =
  "otpauth://totp/FerriteCMS:admin?secret=JBSWY3DPEHPK3PXP&issuer=FerriteCMS&algorithm=SHA1&digits=6&period=30";

test("qr_code_matrix_and_svg", () => {
  const m = qrMatrix(OTPAUTH);
  // 107 bytes at error correction M need version 7 (version 6 holds 106): 17 + 4 × 7 = 45.
  expect(m.version).toBe(7);
  expect(m.size).toBe(45);
  expect(m.data).toHaveLength(45);
  expect(m.data.every((row) => row.length === 45)).toBe(true);

  const { container } = render(<QrCode text={OTPAUTH} label="Scan with your authenticator app" />);
  const svg = screen.getByRole("img", { name: "Scan with your authenticator app" });
  expect(svg.tagName.toLowerCase()).toBe("svg");
  // The quiet zone is 4 modules on every side; drawn at 200 px by default.
  expect(QR_QUIET_ZONE).toBe(4);
  expect(svg.getAttribute("viewBox")).toBe("0 0 53 53");
  expect(svg.getAttribute("width")).toBe("200");
  expect(svg.getAttribute("height")).toBe("200");
  // One background rect plus one rect per dark module.
  const dark = m.data.flat().filter(Boolean).length;
  expect(dark).toMatchInlineSnapshot(`1070`);
  expect(container.querySelectorAll("rect[data-module]")).toHaveLength(dark);
  expect(container.querySelectorAll("rect")).toHaveLength(dark + 1);
  // The secret is only in the drawing: no attribute or text carries the text.
  expect(container.innerHTML).not.toContain("JBSWY3DPEHPK3PXP");
  expect(container.textContent).toBe("");
});

test("qr_code_takes_a_size", () => {
  render(<QrCode text="hello" label="Code" size={120} />);
  const svg = screen.getByRole("img", { name: "Code" });
  expect(svg.getAttribute("width")).toBe("120");
  // Version 1 (21 modules) plus the quiet zone.
  expect(svg.getAttribute("viewBox")).toBe("0 0 29 29");
});
