#!/usr/bin/env python3
"""Writes the plugin registry index for a release (M6-2, design 14 §11).

Merges the previous index (the published one, already verified by the caller) with this
release's signed plugin folders, bumps the serial, applies registry/yanked.json, takes the
publishers (and their keys and revocations) from registry/publishers.json, checks the
result against the rules `ferrite_domain::plugin_registry::RegistryIndex::parse` applies,
and writes it. The caller signs the output with the registry key.

    registry-index.py --previous FILE|- --publishers registry/publishers.json \\
        --yanked registry/yanked.json --version V --base-url URL --plugins DIR --out FILE \\
        [--extra-key FILE]...

`--previous -` (or a missing file) starts at serial 1. DIR holds one folder per plugin
(`<name>/ferrite-plugin.toml`, `<name>/<name>.wasm`, `<name>/ferrite-plugin.minisig`); each
version's files are named `ferrite-plugin-<name>-<version>.{toml,wasm}` under URL.
`--extra-key` adds a public key file to the publisher of the plugins (the dry run's
throwaway key). Python 3 standard library only.
"""

import argparse
import base64
import datetime
import hashlib
import json
import os
import re
import sys

PUBLISHER_ID = re.compile(r"^[a-z0-9][a-z0-9.-]{1,63}$")
PLUGIN_NAME = re.compile(r"^[a-z0-9-]{2,48}$")
SEMVER = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
MAX_INDEX_BYTES = 2 * 1024 * 1024
MAX_MANIFEST = 64 * 1024
MAX_COMPONENT = 32 * 1024 * 1024
WORLD = "ferrite:plugin/plugin@0.1.0"


def fail(msg):
    print(f"::error::registry-index: {msg}", file=sys.stderr)
    sys.exit(1)


def key_line(text):
    """The base64 line of a minisign public key file (or a bare line)."""
    lines = [
        l.strip()
        for l in text.splitlines()
        if l.strip() and not l.strip().startswith("#")
    ]
    if len(lines) == 2 and lines[0].startswith("untrusted comment:"):
        return lines[1]
    if len(lines) == 1:
        return lines[0]
    fail("not a minisign public key file")


def key_id(b64):
    raw = base64.b64decode(b64, validate=True)
    if len(raw) != 42 or raw[:2] != b"Ed":
        fail(f"not an Ed25519 minisign key: {b64}")
    return raw[2:10]


def signature_key_id(sig_text):
    lines = sig_text.splitlines()
    if len(lines) < 4 or not lines[0].startswith("untrusted comment:"):
        fail("a minisig has four lines")
    raw = base64.b64decode(lines[1], validate=True)
    if len(raw) != 74 or raw[:2] != b"ED":
        fail("the signature is not a prehashed minisign signature")
    return raw[2:10]


def semver_key(v):
    m = SEMVER.match(v)
    if not m:
        fail(f"version {v!r} is not semver")
    core = tuple(int(x) for x in m.groups())
    pre = v.split("+", 1)[0].split("-", 1)[1] if "-" in v.split("+", 1)[0] else None
    # A prerelease sorts before its release.
    return core + ((0, pre) if pre else (1, ""),)


def manifest_field(text, key):
    in_plugin = False
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("["):
            in_plugin = s.split("#", 1)[0].strip() == "[plugin]"
            continue
        if in_plugin:
            m = re.match(r'^%s\s*=\s*"([^"]*)"' % re.escape(key), s)
            if m:
                return m.group(1)
    return None


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--previous", required=True)
    ap.add_argument("--publishers", required=True)
    ap.add_argument("--yanked", required=True)
    ap.add_argument("--version", required=True)
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--plugins", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--extra-key", action="append", default=[])
    args = ap.parse_args()

    previous = None
    if args.previous != "-" and os.path.exists(args.previous):
        with open(args.previous, "rb") as f:
            previous = json.load(f)
        if previous.get("schema") != 1:
            fail("the previous index is not schema 1")

    with open(args.publishers) as f:
        publishers = json.load(f)["publishers"]
    with open(args.yanked) as f:
        yanked = json.load(f)
    if not publishers:
        fail("registry/publishers.json lists no publisher")
    extra = []
    for path in args.extra_key:
        with open(path) as f:
            extra.append(key_line(f.read()))
    # The plugins' publisher is the first one (the official plugins').
    publishers[0]["keys"] = list(dict.fromkeys(publishers[0].get("keys", []) + extra))

    known = {}
    for p in publishers:
        if not PUBLISHER_ID.match(p["id"]):
            fail(f"publisher id {p['id']!r} is not valid")
        for k in p.get("keys", []) + [r["key"] for r in p.get("revoked_keys", [])]:
            kid = key_id(k)
            if kid in known:
                fail(f"key {k} is listed twice")
            known[kid] = p["id"]
    current_keys = {key_id(k): p["id"] for p in publishers for k in p.get("keys", [])}

    plugins = {}
    for p in (previous or {}).get("plugins", []):
        plugins[p["name"]] = p

    base = args.base_url.rstrip("/")
    added = 0
    for name in sorted(os.listdir(args.plugins)):
        folder = os.path.join(args.plugins, name)
        if not os.path.isdir(folder):
            continue
        if not PLUGIN_NAME.match(name):
            fail(f"plugin folder {name!r} is not a plugin name")
        manifest = open(os.path.join(folder, "ferrite-plugin.toml"), "rb").read()
        wasm_files = [f for f in os.listdir(folder) if f.endswith(".wasm")]
        if len(wasm_files) != 1:
            fail(f"{folder} must hold exactly one .wasm")
        component = open(os.path.join(folder, wasm_files[0]), "rb").read()
        sig_path = os.path.join(folder, "ferrite-plugin.minisig")
        if not os.path.exists(sig_path):
            fail(f"{folder} has no ferrite-plugin.minisig")
        signature = open(sig_path).read()
        text = manifest.decode("utf-8")
        if manifest_field(text, "name") != name:
            fail(f"{folder}: the manifest's name is not {name}")
        version = manifest_field(text, "version")
        if version != args.version:
            fail(f"{folder}: version {version} is not the release version {args.version}")
        publisher = manifest_field(text, "publisher")
        signer = signature_key_id(signature)
        if current_keys.get(signer) != publisher:
            fail(
                f"{name} is signed by a key that is not a current key of publisher {publisher!r} "
                "in registry/publishers.json"
            )
        if len(manifest) > MAX_MANIFEST or len(component) > MAX_COMPONENT:
            fail(f"{name}: a file is too large")
        entry = plugins.setdefault(
            name,
            {
                "name": name,
                "publisher": publisher,
                "description": manifest_field(text, "description") or "",
                "license": manifest_field(text, "license"),
                "homepage": manifest_field(text, "homepage"),
                "versions": [],
            },
        )
        if entry["publisher"] != publisher:
            fail(f"{name}: the publisher changed from {entry['publisher']} to {publisher}")
        entry["description"] = manifest_field(text, "description") or entry.get("description", "")
        entry["versions"] = [v for v in entry["versions"] if v["version"] != version]
        entry["versions"].append(
            {
                "version": version,
                "world": WORLD,
                "published_at": datetime.datetime.now(datetime.timezone.utc).strftime(
                    "%Y-%m-%dT%H:%M:%SZ"
                ),
                "manifest": {
                    "url": f"{base}/ferrite-plugin-{name}-{version}.toml",
                    "sha256": sha256(manifest),
                    "bytes": len(manifest),
                },
                "component": {
                    "url": f"{base}/ferrite-plugin-{name}-{version}.wasm",
                    "sha256": sha256(component),
                    "bytes": len(component),
                },
                "signature": signature,
                "yanked": None,
            }
        )
        added += 1
    if added == 0:
        fail(f"no plugin folders in {args.plugins}")

    for y in yanked:
        entry = plugins.get(y["name"])
        for v in (entry or {}).get("versions", []):
            if v["version"] == y["version"]:
                v["yanked"] = y.get("reason") or "yanked"

    if previous:
        dropped = {p["name"] for p in previous.get("plugins", [])} - set(plugins)
        if dropped:
            fail(f"the new index would drop {sorted(dropped)}")
    for p in plugins.values():
        if p["publisher"] not in {x["id"] for x in publishers}:
            fail(f"{p['name']} names an unknown publisher")
        p["versions"].sort(key=lambda v: semver_key(v["version"]), reverse=True)
        if len(p["versions"]) > 100:
            fail(f"{p['name']} lists more than 100 versions")
        for k in ("license", "homepage"):
            if p.get(k) is None:
                p.pop(k, None)

    index = {
        "schema": 1,
        "name": "FerriteCMS plugins",
        "serial": (previous["serial"] + 1) if previous else 1,
        "generated_at": datetime.datetime.now(datetime.timezone.utc).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        ),
        "publishers": publishers,
        "plugins": [plugins[n] for n in sorted(plugins)],
    }
    if len(index["plugins"]) > 1000:
        fail("more than 1000 plugins")
    data = (json.dumps(index, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    if len(data) > MAX_INDEX_BYTES:
        fail("the index is larger than 2 MiB")
    with open(args.out, "wb") as f:
        f.write(data)
    print(f"wrote {args.out}: serial {index['serial']}, {len(index['plugins'])} plugins")


if __name__ == "__main__":
    main()
