// Minisign signatures made with Node's own crypto (M6-2, design 14 D19): the release
// binary under test has no signing code, and a second, independent implementation of the
// format also catches a verifier that only agrees with itself. Throwaway keys only.
//
// Format (minisign 0.8+, prehashed): the public key is base64("Ed" + key id (8 bytes) +
// Ed25519 public key (32)); a signature file is four lines: an untrusted comment,
// base64("ED" + key id + Ed25519(BLAKE2b-512(data))), a trusted comment, and
// base64(Ed25519(signature || trusted comment text)).

import { createHash, generateKeyPairSync, randomBytes, sign } from "node:crypto";

export interface TestKey {
  /** The minisign public key file (`ferrite-plugin.pub`, `registry_key`). */
  publicFile: string;
  /** The bare base64 line. */
  publicBase64: string;
  /** 16 uppercase hex digits, as minisign prints it. */
  keyId: string;
  /** A prehashed minisign signature file over `data`. */
  sign(data: Buffer | string): string;
}

/** A new throwaway Ed25519 minisign key. */
export function generateKey(): TestKey {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const jwk = publicKey.export({ format: "jwk" });
  const raw = Buffer.from(jwk.x ?? "", "base64url");
  const id = randomBytes(8);
  const publicBase64 = Buffer.concat([Buffer.from("Ed"), id, raw]).toString("base64");
  // Key ids print as the little-endian number in uppercase hex.
  const keyId = Buffer.from(id).reverse().toString("hex").toUpperCase();
  return {
    publicBase64,
    keyId,
    publicFile: `untrusted comment: minisign public key ${keyId}\n${publicBase64}\n`,
    sign(data) {
      const bytes = typeof data === "string" ? Buffer.from(data) : data;
      const digest = createHash("blake2b512").update(bytes).digest();
      const signature = sign(null, digest, privateKey);
      const trusted = `timestamp:${Math.floor(Date.now() / 1000)}\tfile:e2e`;
      const global = sign(null, Buffer.concat([signature, Buffer.from(trusted)]), privateKey);
      const line = Buffer.concat([Buffer.from("ED"), id, signature]).toString("base64");
      return [
        "untrusted comment: signature from an e2e throwaway key",
        line,
        `trusted comment: ${trusted}`,
        global.toString("base64"),
        "",
      ].join("\n");
    },
  };
}

/** The signing payload of a plugin (design 14 §2.1). */
export function signingPayload(manifest: Buffer | string, component: Buffer): string {
  const sha = (b: Buffer | string) => createHash("sha256").update(b).digest("hex");
  return `ferrite-plugin-signature/1\nmanifest ${sha(manifest)}\ncomponent ${sha(component)}\n`;
}
