// Writes `test-results/screens/index.html`: one row per screen with its four `snap`
// shots, for the design review gate (11 §8). Called from the global teardown.

import { existsSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { SCHEMES, SCREENS_DIR, VIEWPORTS, type Scheme, type Viewport } from "./snap";

const FILE = /^([a-z0-9][a-z0-9_-]*)\.(light|dark)\.(desktop|mobile)\.png$/;

export function writeContactSheet(dir: string = SCREENS_DIR): string | null {
  if (!existsSync(dir)) return null;
  const screens = new Map<string, Set<string>>();
  for (const f of readdirSync(dir)) {
    const m = FILE.exec(f);
    if (!m?.[1]) continue;
    const set = screens.get(m[1]) ?? new Set<string>();
    set.add(f);
    screens.set(m[1], set);
  }
  const names = [...screens.keys()].sort();
  const cell = (name: string, s: Scheme, v: Viewport) => {
    const f = `${name}.${s}.${v}.png`;
    const label = `${s} · ${v} ${VIEWPORTS[v].width}×${VIEWPORTS[v].height}`;
    return screens.get(name)?.has(f)
      ? `<figure class="${v}"><a href="${f}"><img src="${f}" alt="${name}, ${label}" loading="lazy"></a><figcaption>${label}</figcaption></figure>`
      : `<figure class="${v} missing"><figcaption>${label}: missing</figcaption></figure>`;
  };
  const rows = names
    .map((n) => {
      const figs = (Object.keys(VIEWPORTS) as Viewport[])
        .flatMap((v) => SCHEMES.map((s) => cell(n, s, v)))
        .join("");
      return `<section id="${n}"><h2><a href="#${n}">${n}</a></h2><div class="row">${figs}</div></section>`;
    })
    .join("\n");
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Admin screens</title>
<style>
  :root { color-scheme: light dark; --bg: #f6f7f9; --fg: #16181d; --muted: #5b6270; --card: #fff; --line: #dde1e7; }
  @media (prefers-color-scheme: dark) { :root { --bg: #111317; --fg: #e8eaee; --muted: #9aa1ad; --card: #1a1d23; --line: #2a2f38; } }
  body { margin: 0; padding: 24px 16px 64px; background: var(--bg); color: var(--fg); font: 14px/1.5 system-ui, sans-serif; }
  h1 { font-size: 20px; margin: 0 0 4px; } p { color: var(--muted); margin: 0 0 24px; }
  section { margin: 0 0 40px; } h2 { font-size: 15px; margin: 0 0 12px; } h2 a { color: inherit; text-decoration: none; }
  .row { display: flex; gap: 16px; align-items: flex-start; overflow-x: auto; padding-bottom: 8px; }
  figure { margin: 0; flex: none; background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 8px; }
  figure.desktop img { width: 480px; } figure.mobile img { width: 160px; }
  img { display: block; height: auto; border-radius: 4px; }
  figcaption { color: var(--muted); font-size: 12px; margin-top: 6px; }
  .missing { width: 160px; } .missing figcaption { color: #c2410c; }
</style>
</head>
<body>
<h1>Admin screens</h1>
<p>${names.length} screen(s), light and dark at desktop and mobile width. Review against docs/design/11-admin-ui.md §8; these are not pixel baselines.</p>
${rows}
</body>
</html>
`;
  const out = join(dir, "index.html");
  writeFileSync(out, html);
  return out;
}
