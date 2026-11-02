import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { installedTrust, plugin, trust, verifiedTrust } from "./fixtures";
import { PublisherLine, TrustSummary } from "./trust-summary";

function block(): HTMLElement {
  return screen.getByRole("group", { name: "Publisher" });
}

test("trust_summary_verified", () => {
  render(<TrustSummary trust={verifiedTrust()} />);
  const b = block();
  expect(b.dataset.state).toBe("verified");
  expect(b).toHaveTextContent("Verified publisher");
  expect(b).toHaveTextContent("FerriteCMS");
  expect(b).toHaveTextContent("432051207B9BA065");
  expect(b).toHaveTextContent("Its data will belong to FerriteCMS (registry).");
  // A registry publisher's claim is the verified text: no "not verified" line.
  expect(b).not.toHaveTextContent("not verified");
});

test("trust_summary_new_key", () => {
  render(
    <TrustSummary
      trust={trust({
        signature: "verified",
        signer: {
          key_id: "5A1C3D0000000000",
          source: "new",
          publisher: null,
          claimed: "example.dev",
        },
        owner: {
          change: "bind",
          current: null,
          next: "key:5A1C3D0000000000",
          next_label: "key 5A1C3D0000000000",
        },
        warnings: ["new_key"],
      })}
    />,
  );
  const b = block();
  expect(b.dataset.state).toBe("new_key");
  expect(b).toHaveTextContent("New publisher key");
  expect(b).toHaveTextContent("Approving trusts this key for this plugin's updates.");
  expect(b).toHaveTextContent("The manifest names the publisher example.dev (not verified).");
  expect(b.className).toContain("warning");
});

test("trust_summary_unsigned", () => {
  render(<TrustSummary trust={trust()} />);
  const b = block();
  expect(b.dataset.state).toBe("unsigned");
  // Said in words, not by colour alone.
  expect(b).toHaveTextContent("Unsigned plugin: nothing confirms who made it");
});

test("trust_summary_claim", () => {
  render(
    <TrustSummary
      trust={verifiedTrust({
        owner: {
          change: "claim",
          current: null,
          next: "reg:9F1E000000000000:ferritecms.dev",
          next_label: "FerriteCMS (registry)",
        },
        warnings: ["unsigned_data_claimed"],
      })}
    />,
  );
  expect(screen.getByText(/existing data will be bound to FerriteCMS \(registry\)/)).toBeVisible();
});

test("trust_summary_refusal_and_policy", () => {
  render(
    <TrustSummary
      trust={trust({
        policy: "signed",
        owner: null,
        refusal: { field: "signature", message: "this site installs signed plugins only" },
      })}
    />,
  );
  const b = block();
  expect(b.dataset.state).toBe("refused");
  expect(b).toHaveTextContent("this site installs signed plugins only");
  expect(b).toHaveTextContent("Signature policy: signed");
});

test("list_shows_publisher_state_as_text", () => {
  const { rerender } = render(
    <PublisherLine
      plugin={plugin({
        trust: installedTrust({
          state: "verified",
          signer: {
            key_id: "1",
            source: "registry",
            publisher: { id: "ferritecms.dev", name: "FerriteCMS" },
            claimed: "ferritecms.dev",
          },
        }),
      })}
    />,
  );
  expect(screen.getByText("FerriteCMS · Verified")).toBeVisible();
  rerender(
    <PublisherLine
      plugin={plugin({ publisher: "example.dev", trust: installedTrust({ state: "signed" }) })}
    />,
  );
  expect(screen.getByText("example.dev · Signed")).toBeVisible();
  rerender(<PublisherLine plugin={plugin({ publisher: "example.dev" })} />);
  expect(screen.getByText("example.dev · Unsigned")).toBeVisible();
  rerender(
    <PublisherLine
      plugin={plugin({ publisher: "example.dev", trust: installedTrust({ state: "revoked" }) })}
    />,
  );
  expect(screen.getByText("example.dev · Key revoked")).toBeVisible();
});

test("hostile_publisher_text_rendered_as_text", () => {
  render(
    <TrustSummary
      trust={trust({
        signature: "verified",
        signer: {
          key_id: "5A1C3D0000000000",
          source: "new",
          publisher: null,
          claimed: "<img src=x onerror=alert(1)>",
        },
      })}
    />,
  );
  expect(screen.getByText(/<img src=x onerror=alert\(1\)>/)).toBeVisible();
  expect(document.querySelector("img")).toBeNull();
});
