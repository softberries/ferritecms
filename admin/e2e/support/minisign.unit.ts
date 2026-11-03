// Unit tests (Vitest, the `scripts` project) of the E2E minisign signer: the format rules,
// the signature verifies with Node's Ed25519, and, when the `minisign` CLI is on the PATH,
// with minisign itself.

import { execFileSync } from "node:child_process";
import { createHash, createPublicKey, verify } from "node:crypto";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { generateKey, signingPayload } from "./minisign";

function hasMinisign(): boolean {
  try {
    execFileSync("minisign", ["-v"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

describe("minisign", () => {
  it("keys_have_the_minisign_shape", () => {
    const k = generateKey();
    const raw = Buffer.from(k.publicBase64, "base64");
    expect(raw).toHaveLength(42);
    expect(raw.subarray(0, 2).toString()).toBe("Ed");
    expect(k.keyId).toMatch(/^[0-9A-F]{16}$/);
    expect(Buffer.from(raw.subarray(2, 10)).reverse().toString("hex").toUpperCase()).toBe(k.keyId);
    expect(k.publicFile.split("\n")[1]).toBe(k.publicBase64);
  });

  it("signatures_are_prehashed_and_verify", () => {
    const k = generateKey();
    const data = Buffer.from("hello");
    const lines = k.sign(data).split("\n");
    expect(lines).toHaveLength(5);
    expect(lines[2]).toMatch(/^trusted comment: /);
    const sig = Buffer.from(lines[1] ?? "", "base64");
    expect(sig).toHaveLength(74);
    expect(sig.subarray(0, 2).toString()).toBe("ED");
    const raw = Buffer.from(k.publicBase64, "base64").subarray(10);
    const key = createPublicKey({
      key: { kty: "OKP", crv: "Ed25519", x: raw.toString("base64url") },
      format: "jwk",
    });
    const digest = createHash("blake2b512").update(data).digest();
    expect(verify(null, digest, key, sig.subarray(10))).toBe(true);
    const trusted = (lines[2] ?? "").slice("trusted comment: ".length);
    const global = Buffer.from(lines[3] ?? "", "base64");
    expect(verify(null, Buffer.concat([sig.subarray(10), Buffer.from(trusted)]), key, global)).toBe(
      true,
    );
  });

  it.runIf(hasMinisign())("the_minisign_cli_verifies_it", () => {
    const k = generateKey();
    const dir = mkdtempSync(join(tmpdir(), "ferrite-minisign-"));
    const file = join(dir, "data");
    writeFileSync(file, "payload bytes");
    writeFileSync(join(dir, "data.minisig"), k.sign(Buffer.from("payload bytes")));
    writeFileSync(join(dir, "key.pub"), k.publicFile);
    const out = execFileSync("minisign", ["-V", "-p", join(dir, "key.pub"), "-m", file], {
      encoding: "utf8",
    });
    expect(out).toContain("Signature and comment signature verified");
  });

  it("payload_is_three_lines", () => {
    const p = signingPayload("m", Buffer.from("c"));
    expect(p.split("\n")).toHaveLength(4);
    expect(p.startsWith("ferrite-plugin-signature/1\nmanifest ")).toBe(true);
  });
});
