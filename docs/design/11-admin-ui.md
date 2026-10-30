# 11 — Admin UI design language

Status: **as built (M2b-1 Tasks 7–10)** · Date: 2026-10-25

The admin SPA's visual and interaction language: what it borrows, the typography, the
colour tokens, spacing, density, motion, the keyboard map, the component inventory and
the checklist every UI change is reviewed against. 07 §3 covers the stack, embedding
and screens; this document covers how they look and behave. The product choices here
are M2b P3 (fonts) and P4 (accent), accepted by Ruling R2.

Sources of truth in code:

| What | Where |
|---|---|
| Token values (the only file with colour literals) | `admin/src/styles/tokens.css` |
| Tailwind theme mapping, base layer, reduced motion | `admin/src/styles/globals.css` |
| Font faces | `admin/src/styles/fonts.css` |
| Token names, contrast pairs | `admin/src/design/tokens.ts` |
| Colour maths (OKLCH → sRGB, WCAG ratio) | `admin/src/design/contrast.ts` |
| The gate | `admin/src/design/contrast.test.ts` (Vitest), `admin/e2e/design.spec.ts` (browser) |
| Theme and density | `admin/src/design/{theme.tsx,density.tsx,preferences.ts}` |

## 1. Study notes — patterns borrowed

Observed in the public products; no metrics are claimed.

| Product | Borrowed |
|---|---|
| **Linear** | ⌘K as the primary navigation; `g`-chord shortcuts and a `?` sheet; compact density; borders over shadows; optimistic updates; a property sidebar edited in place. |
| **Vercel dashboard** | A monochrome base with one accent; status dots; toasts with an undo action; skeletons shaped like the final layout; mono for identifiers. |
| **Stripe Dashboard** | Dense tables with filter chips and sticky headers; row hover actions; metric tiles with sparklines and deltas; considered empty states. |
| **Ghost Admin** | A distraction-free editor (large title field, settings in a slide-over, `/` insert menu); publishing as a deliberate confirm step with a summary; a word count. |
| **Notion** | The slash menu and the selection bubble. |
| **GitHub Primer** | Status colour semantics and the focus-ring standard. |
| **WordPress — avoid** | Admin-notice nags, meta-box clutter, full page reloads, inconsistent screen layouts. |

## 2. Typography

- **UI: Inter Variable** — `@fontsource-variable/inter` 5.3.0, `opsz.css` +
  `opsz-italic.css`: axes `wght` 100–900 and `opsz`. `font-optical-sizing: auto` (set on
  `html`) lets headings pick the display cut by size.
- **Mono: Geist Mono Variable** — `@fontsource-variable/geist-mono` 5.3.0, `wght.css`
  (`wght` 100–900).
- Both OFL-1.1, self-hosted: Vite hashes the woff2 files into `dist/assets/` and the
  binary embeds them (principle 1; no CDN; the CSP's `font-src 'self'` holds). Every
  face carries fontsource's `unicode-range` subsets (Latin, Latin Extended, Cyrillic,
  Cyrillic Extended, Greek, Greek Extended, Vietnamese; Geist Mono also Symbols 2), so
  the browser fetches only what a page uses, and `font-display: swap`. The licence
  texts ship at `/admin/licenses/inter-variable.txt` and
  `/admin/licenses/geist-mono-variable.txt` (copied from the packages into
  `admin/public/licenses/`).
- **Shipped weight** (measured on the Task 7 build): 20 woff2 files, **781 088 bytes
  (763 KiB)** in total — Inter normal 340 256, Inter italic 370 384, Geist Mono 70 448 bytes.
  The Latin subsets, all an English page needs, are Inter Latin normal 72 920 bytes,
  Inter Latin italic 79 716 and Geist Mono Latin 23 128.
- Stacks: `--font-sans: "Inter Variable", ui-sans-serif, system-ui, sans-serif`;
  `--font-mono: "Geist Mono Variable", ui-monospace, monospace`. shadcn's Nova preset
  added Geist Sans; it is removed (F13).

Scale (`--text-*` in `globals.css`; Tailwind's default sizes are reset, so only these
exist):

| Token | px / line-height | Use |
|---|---|---|
| `xs` | 12 / 16 | Captions, badges, kbd |
| `sm` | 13 / 18 | Secondary text, table meta; ids, slugs, versions and hashes in mono |
| `base` | 14 / 20 | **UI default** (body) |
| `md` | 16 / 24 | Prominent body, form intros |
| `editor` | 17 / 28 | Editor body (instead of `md` there) |
| `lg` | 18 / 26 | Section titles |
| `xl` | 22 / 28 | Page titles in dense screens |
| `2xl` | 28 / 34 | Page titles; auth screen titles (Task 9: `3xl` wrapped "Choose a new password" in the 400 px card) |
| `3xl` | 36 / 42 | Editor title field |

- Headings (`h1`–`h6`, base layer) use weight 600 and tracking −0.01em; `xl` carries
  −0.01em and `2xl`/`3xl` −0.02em in the scale itself.
- Numbers in tables and tiles use `font-variant-numeric: tabular-nums` (`td`/`th` by
  default; `tabular-nums` elsewhere).
- Ids, slugs, versions and hashes are mono at `sm` (`font-mono text-sm`); `code`,
  `kbd`, `samp`, `pre` are mono by default.

Not verified: how Chromium on Linux renders Inter's `opsz` axis; the CI contact sheet
(`admin-screens` artifact) is where to check. Task 7 only checked, in macOS Chromium,
that both families load from `/admin/assets/` and that `h1` computes to Inter Variable
(`e2e/design.spec.ts`).

## 3. Colour

All colours are `oklch(L C H [/ A])`, light / dark. Neutrals have a cool graphite cast
(hue 260); the single accent is **"ember"** (oxidised iron, the product's namesake).

| Token | Light | Dark |
|---|---|---|
| `background` | 0.99 0.002 250 | 0.165 0.008 260 |
| `foreground` | 0.21 0.012 260 | 0.965 0.004 260 |
| `card` | 1 0 0 | 0.195 0.009 260 |
| `muted` | 0.965 0.004 260 | 0.22 0.01 260 |
| `muted-foreground` | 0.50 0.015 260 | 0.71 0.015 260 |
| `border` | 0.91 0.005 260 | 0.29 0.01 260 |
| `input` | 0.64 0.01 260 | 0.52 0.012 260 |
| `primary` (ember) | 0.55 0.17 40 | 0.73 0.15 52 |
| `primary-foreground` | 0.99 0.005 40 | 0.20 0.03 45 |
| `primary-hover` (primary fill on hover) | 0.49 0.15 40 | 0.80 0.12 55 |
| `ring` | = `primary` | = `primary` |
| `destructive` | 0.55 0.21 27 | 0.70 0.18 25 |
| `success` | 0.52 0.13 150 | 0.74 0.14 150 |
| `warning` | **0.55 0.115 70** (see changes) | 0.80 0.14 80 |
| `info` | 0.52 0.14 255 | 0.72 0.13 250 |

Derived tokens (`var()` in `tokens.css`, so they follow the mode):

- `card-foreground`, `popover-foreground`, `secondary-foreground`,
  `accent-foreground` = `foreground`; `popover` = `card`; `secondary` = `muted`.
  shadcn's `accent` is the hover/selected surface, not the brand accent: `accent` =
  `muted`.
- Content status: `status-draft` = `muted-foreground`, `status-pending` = `warning`,
  `status-scheduled` = `info`, `status-published` = `success`, `status-private` =
  `foreground` at 70 % (`color-mix(in oklch, var(--foreground) 70%, transparent)`,
  so it follows `foreground` in both modes; `status_private_derives_from_foreground`
  checks it), `status-trash` = `destructive`. Status is
  never colour alone: the badge is a dot plus a label (Task 8 `StatusBadge`).
- Charts: `chart-1` ember, `chart-2` info, `chart-3` success, `chart-4` warning,
  `chart-5` a violet 0.55 0.16 300 / 0.72 0.14 300. Series are also told apart by shape
  or label, never by colour alone.
- `overlay` (the scrim behind dialogs, sheets and alert dialogs; decoration, no
  contrast pair): 0.21 0.012 260 / 0.24 light, 0.1 0.005 260 / 0.6 dark (Task 8,
  replacing the generated `bg-black/10`).
- Sidebar: `sidebar` = `card`, `sidebar-accent` = `muted`, `sidebar-foreground` and
  `sidebar-accent-foreground` = `foreground`, `sidebar-primary(-foreground)` =
  `primary(-foreground)`, `sidebar-border` = `border`, `sidebar-ring` = `ring`.
- `color-scheme` is `light` on `:root` and `dark` on `.dark` (scrollbars, form
  controls); `::selection` is ember at 24 %.
- `qr-fg` / `qr-bg` (M2b-3): `oklch(0 0 0)` / `oklch(1 0 0)`, defined on `:root` only,
  so they stay black on white in dark mode too (phone cameras read QR codes as dark on
  light). `QrCode` draws with them (`fill-qr-fg`, `fill-qr-bg`); their pair is gated at
  4.5:1 like text (21:1).

Tailwind names: `bg-primary`, `text-success`, `text-status-published`,
`bg-chart-3`, … (`--color-*` in `globals.css`).

**The gate** (`contrast.test.ts`): `parseTokens` reads `:root` (light) and `.dark`
(dark; `.dark` sits on `<html>`, so references resolve in the mode), converts OKLCH to
sRGB with Björn Ottosson's OKLab matrices, composites translucent colours over their
background in gamma sRGB as browsers blend, and computes the WCAG 2.x ratio. Text
pairs need ≥ 4.5:1 (1.4.3); `input`, `ring` and chart series need ≥ 3:1 (1.4.11). Every
colour token must also lie strictly inside the sRGB gamut (every linear channel in
0..1; the 1e-6 tolerance only absorbs float noise), so no browser gamut mapping can
move a ratio. The parser understands OKLCH values, `var()` references and
`color-mix(in oklch, <colour> N%, transparent)` fades. Values computed 2026-10-25 from the
shipped `tokens.css`:

| Foreground | Background | Min | Light | Dark |
|---|---|---|---|---|
| `foreground` | `background` | 4.5 | 17.22 | 17.41 |
| `foreground` | `card` | 4.5 | 17.72 | 16.52 |
| `foreground` | `muted` | 4.5 | 16.01 | 15.64 |
| `muted-foreground` | `background` | 4.5 | 5.83 | 7.49 |
| `muted-foreground` | `card` | 4.5 | 6.00 | 7.11 |
| `muted-foreground` | `muted` | 4.5 | 5.42 | 6.73 |
| `primary-foreground` | `primary` | 4.5 | 5.08 | 7.27 |
| `primary-foreground` | `primary-hover` | 4.5 | 6.51 | 9.45 |
| `primary` (link text) | `background` | 4.5 | 5.09 | 7.69 |
| `primary` (link text) | `card` | 4.5 | 5.24 | 7.30 |
| `destructive` | `background` | 4.5 | 5.25 | 6.65 |
| `destructive` | `card` | 4.5 | 5.40 | 6.32 |
| `success` | `background` | 4.5 | 5.04 | 8.83 |
| `success` | `card` | 4.5 | 5.19 | 8.38 |
| `warning` | `background` | 4.5 | 4.84 | 10.17 |
| `warning` | `card` | 4.5 | 4.98 | 9.66 |
| `info` | `background` | 4.5 | 5.39 | 7.81 |
| `info` | `card` | 4.5 | 5.55 | 7.41 |
| `status-draft` | `background` / `card` | 4.5 | 5.83 / 6.00 | 7.49 / 7.11 |
| `status-pending` | `background` / `card` | 4.5 | 4.84 / 4.98 | 10.17 / 9.66 |
| `status-scheduled` | `background` / `card` | 4.5 | 5.39 / 5.55 | 7.81 / 7.41 |
| `status-published` | `background` / `card` | 4.5 | 5.04 / 5.19 | 8.83 / 8.38 |
| `status-private` | `background` / `card` | 4.5 | 6.46 / 6.54 | 8.74 / 8.48 |
| `status-trash` | `background` / `card` | 4.5 | 5.25 / 5.40 | 6.65 / 6.32 |
| `sidebar-foreground` | `sidebar` / `sidebar-accent` | 4.5 | 17.72 / 16.01 | 16.52 / 15.64 |
| `input` | `background` | 3 | 3.27 | 3.50 |
| `ring` | `background` | 3 | 5.09 | 7.69 |
| `chart-1` … `chart-5` | `background` | 3 | 5.09, 5.39, 5.04, 4.84, 5.06 | 7.69, 7.81, 8.83, 10.17, 7.40 |
| `chart-1` … `chart-5` | `card` | 3 | 5.24, 5.55, 5.19, 4.98, 5.21 | 7.30, 7.41, 8.38, 9.66, 7.02 |

`border` (1.27 light, 1.37 dark on `background`) is decoration, not a boundary that
carries meaning; anything a user must find (inputs, focus) uses `input` or `ring`.

**Changes to the starting values:** one. Light `warning` 0.55 **0.13** 70 → 0.55
**0.115** 70: at L 0.55, hue 70, chroma 0.13 lies outside sRGB (linear blue channel
−0.010), so its rendered colour would depend on the browser's gamut mapping; 0.115 is
the largest chroma in steps of 0.005 that is strictly inside (0.12 still gives linear
blue −0.0008; Task 7 first shipped 0.12 under a 0.001 tolerance, tightened in its fix
round). Its ratios do not move (4.84 on `background`, 4.98 on `card`). Every starting value passed its
contrast pair unchanged.

## 4. Spacing, radius, elevation, density

- Spacing: Tailwind's 4 px scale (`--spacing: 0.25rem`), unchanged.
- Radius: `--radius` 0.5rem; `rounded-sm` 0.3, `rounded-md` 0.4, `rounded-lg` 0.5,
  `rounded-xl` 0.7 rem (`calc(var(--radius) * 0.6 | 0.8 | 1 | 1.4)`; 2xl–4xl kept for
  shadcn).
- Elevation:
  - `--shadow-1` `0 1px 2px oklch(0 0 0 / 0.06)` (resting surfaces),
  - `--shadow-2` `0 4px 12px oklch(0 0 0 / 0.08), 0 1px 3px oklch(0 0 0 / 0.06)`
    (raised: menus, cards on hover),
  - `--shadow-3` `0 12px 32px oklch(0 0 0 / 0.14)` (overlays: dialogs, sheets,
    the palette).
  - Utilities `shadow-1`…`shadow-3`; Tailwind's own names map onto them so generated
    components follow the scale: `shadow-2xs`/`xs`/`sm` → 1, `md` → 2, `lg`/`xl`/`2xl`
    → 3.
  - In dark mode all three are `none`: elevation comes from a 1 px `border` plus a
    lighter `card` (0.195 on a 0.165 `background`).
- Density through `html[data-density]`:

  | Variable | `comfortable` (default) | `compact` | Utility |
  |---|---|---|---|
  | `--control-h` (buttons, inputs, selects) | 36 px | 30 px | `h-control` |
  | `--row-h` (table rows) | 44 px | 34 px | `h-row` |
  | `--page-pad` (page padding) | 24 px | 16 px | `p-page`, `px-page`, … |

  The choice is stored per viewer in `localStorage` (`ferrite.admin.density`), read
  inside try/catch (private windows and blocked storage fall back to `comfortable`).
  `DensityProvider` / `useDensity()` (`@/design/density`) switch it. On touch-width
  screens targets stay ≥ 44 px whatever the density (§8): under `@media (pointer:
  coarse)` `tokens.css` sets `--control-h` 44 px and `--row-h` 48 px for both densities
  (`:root:root, :root[data-density]`, so it outranks the compact rule; Task 8, E2E
  `styleguide_touch_targets`).
- `--sticky-top` (default `0px`, `tokens.css`): the height of whatever sticks to the
  top of the viewport above page content. The shell sets it to its top bar's height
  (`3.5rem`, on the `SidebarProvider` wrapper in `routes/_app.tsx`); the
  styleguide sets it to its own header (6.5 rem below `sm`, 4 rem above). `DataTable`
  frames use it to cap their height (§7).
- Theme: `ThemeProvider` / `useTheme()` (`@/design/theme`; `{ theme, resolvedTheme,
  setTheme }`, the shape the shadcn sonner wrapper expects, P9) with `light | dark |
  system`; `system` follows `prefers-color-scheme` live. Once JS runs, `<html>` carries
  exactly one of `.light` / `.dark` (`applyTheme`), and the choice is stored in
  `localStorage` (`ferrite.admin.theme`), also inside try/catch. `main.tsx` calls
  `applyStoredPreferences()` as module code before the first render (no inline
  script; the CSP forbids one).
- **First paint.** The stylesheet is render-blocking but the module script is
  deferred, so the browser can paint before any JS runs. For that frame `<html>` has
  no class, and `tokens.css` covers it on its own: `@media (prefers-color-scheme:
  dark) { :root:not(.light):not(.dark) { color-scheme: dark; --background; --foreground } }`
  with the `.dark` values (`dark_first_frame_matches_dark_tokens` keeps them equal;
  E2E `dark_os_first_paint_is_dark_before_js` loads the page with JS disabled under
  an emulated dark scheme). Only these tokens are needed: before React mounts, `#root`
  is empty, so the canvas (`body` background) and `color-scheme` are all that paint.
  The `dark:` variant (`.dark *`) does not match in that frame, and nothing uses it yet.
  - Flash-free: `system` on any OS (the default), and an explicit choice whatever the
    OS (P57). An explicit choice is also a cookie `ferrite_admin_theme=dark|light`
    (`Path=/admin`, `SameSite=Lax`, one year, `Secure` on https, not `HttpOnly` since
    JS writes it; removed for `system`, and refreshed from `localStorage` on every load).
    Serving `index.html` (`crates/ferrite-http/src/admin.rs`) adds `class="dark"` or
    `class="light"` to `<html>` when the cookie is exactly one of those values; any
    other value is ignored, and the class is a fixed string, never the cookie's text.
    No inline script is added, so the CSP is unchanged. `index.html` carries
    `Vary: Accept-Encoding, Cookie` and a rewritten page the ETag of its own bytes, so
    no cache hands one theme's copy to another viewer (Rust
    `index_html_gets_theme_class_from_cookie`; E2E
    `explicit_dark_against_light_os_has_no_light_frame`).
  - Still one frame late: a choice whose cookie the browser does not send (cookies
    blocked), which falls back to the class set by JS.
  - Density has no first-paint effect: nothing density-sized renders before React.

## 5. Motion

| Token | Value | Use |
|---|---|---|
| `--dur-instant` | 80 ms | Hover, press |
| `--dur-fast` | 120 ms | Popovers, tooltips; the default `transition` duration |
| `--dur-base` | 180 ms | Page transitions, sheets (≤ 200 ms, 07 §3) |
| `--dur-slow` | 240 ms | Drawers only |
| `--ease-out` | `cubic-bezier(0.22, 1, 0.36, 1)` | Enter, most transitions; the default `transition` easing |
| `--ease-in-out` | `cubic-bezier(0.65, 0, 0.35, 1)` | Moves between two resting states |

- Utilities: `ease-out`, `ease-in-out` (Tailwind's own values are overridden),
  `duration-(--dur-base)`; plain `transition` uses `--dur-fast` and `--ease-out`.
- Motion springs (the `motion` package): layout (list reorder) `{ type: "spring",
  stiffness: 520, damping: 42, mass: 1 }`; panels `{ type: "spring", stiffness: 380,
  damping: 34 }`. Layout animation needs `domMax` (F28): `main.tsx` wraps the app in
  `LazyMotion features={() => import("./lib/motion-features")…}` (its own chunk); use
  `m.*` from `motion/react-m` inside it. Numbers for Motion come from `MOTION` in
  `design/tokens.ts` (`durBase` 0.18 s, `easeOut`), kept equal to `tokens.css` by
  `motion_constants_match_tokens`.
- Route change (`shell/route-transition.tsx`): an `m.div` keyed by the leaf route id,
  enter only: `opacity 0→1, y 4→0` over `--dur-base` with `--ease-out`. **No
  `AnimatePresence`** (ruling at Task 10 fix round 1, overriding the plan's `mode="wait"`):
  an exiting copy keeps rendering the same `<Outlet>`, so the next route mounted inside
  it, unmounted with it and mounted again — twice per navigation, which would create a
  TipTap editor or a blocker twice. A new key remounts the div and plays the enter
  animation; a search-param change keeps it. Under reduced motion it animates opacity
  only, so no frame carries a transform. Tests: `one_navigation_mounts_the_new_route_once`
  (Vitest), E2E `reduced_motion_disables_transforms` (a `MutationObserver` records each
  inserted container's inline transform: `translateY(4px)` normally, none under reduced
  motion).
- Meaning only: enter/exit of overlays, list insert/remove/reorder, the route change
  (opacity plus 4 px y), the save-state indicator. Nothing loops, except loading
  skeletons' pulse, which stops under reduced motion. Nothing moves more than 8 px
  except sheets.
- `prefers-reduced-motion: reduce`:
  - `tokens.css` sets every `--dur-*` to `0ms`;
  - `globals.css` sets `animation-duration` and `transition-duration` to `0.01ms`,
    `animation-iteration-count` to 1 and `scroll-behavior` to `auto` on `*, ::before,
    ::after`, because tw-animate-css keyframes (Radix overlays) carry their own
    durations (F29);
  - `MotionConfig reducedMotion="user"` (in `main.tsx`) turns off transform and layout
    motion in Motion (opacity still fades).

## 6. Keyboard map

`mod` is ⌘ on macOS and Ctrl elsewhere. Single-key shortcuts never fire inside text
inputs or the editor.

Bindings go through `@/lib/shortcuts` (`registerShortcut`, `useShortcut`,
`listShortcuts`; one window `keydown` listener; sequences time out after 1 000 ms;
inside `input`, `textarea`, `select` and `[contenteditable]` only `allowInInput`
bindings fire; a key a component already handled — `defaultPrevented` — is left
alone). Local keys (the `DataTable` row keys, the editor's) stay in their components.
Bound by the shell as of Task 10: `mod+k` (`allowInInput`), `?`, `mod+b` and `g d`;
Task 13 adds `g p`/`g g` (nav items, `shell/nav.ts`) and, while a content list is open,
`/`, `#` and `esc` (clear the selection; not while a menu or dialog is open); the rest
arrive with their screens (`c p`/`c g` with the editor, Task 14). As
built in M2b-2: `g m` (Media, Task 7) and `g c` (Comments, Task 12) come from `navFor`'s
screen rules and bind once the nav entry is registered (both are); the comments queue
binds `a`, `u`, `s` and `#` (group "Comments queue", off while a dialog or menu is open)
while it is open; the media library and the redirects list focus their search with `/`
(the shared `SearchField`), and their tables, like the other lists, take the
`DataTable` keys; the type form, the menu editor and the media detail save on `mod+s`.
The Structure screens have no chord. M2b-3's Administration section (`SCREENS` in
`shell/nav.ts`, Task 5) fixes `g u` Users, `g a` Appearance, `g l` Plugins, `g s`
Settings and `g y` System; each binds once its screen registers its entry (all five do,
Tasks 7–11). The M2b-3 plan named `g p` for Plugins, which is Posts here; Plugins takes
`g l`. M3-2's Import screen (P32) takes `g i`, free until then; it registers with the
screen, between Plugins and Settings. The `?` sheet lists whatever is registered at the
moment it opens.

As built in M2b-3: a pending sequence keeps the keys pressed so far and looks its candidates
up in the registry at each step, so `g s` completes even when the nav registers its `g`
bindings again between the two keys (Task 14, `chord_completes_with_a_binding_registered_between_steps`).
On Apple platforms a letter or digit pressed with Option is matched by its physical key
(`e.code`; ⌥R types "®"), so `mod+alt+r` runs; with ⌘ held this holds whatever the browser
reports for AltGraph, and Option alone with AltGraph reported is left to type. Elsewhere Alt
and AltGr type real letters (Polish ą, ę, ś…), which no shortcut takes (R18 as updated;
`mac_option_reported_as_altgraph_still_runs_cmd_option_r`). A check on a real Mac is part
of the manual Safari pass.

| Where | Keys | Action |
|---|---|---|
| Anywhere | `mod+k` | Command palette |
| Anywhere (not in fields or the editor) | `mod+b` | Collapse / expand the sidebar |
| Anywhere | `?` | Shortcut sheet |
| Anywhere | `g d` | Dashboard |
| Anywhere | `g p` | Posts |
| Anywhere | `g g` | Pages |
| Anywhere | `g m` | Media (M2b-2) |
| Anywhere | `g c` | Comments (M2b-2) |
| Anywhere | `g u` | Users (M2b-3) |
| Anywhere | `g a` | Appearance: themes (M2b-3) |
| Anywhere | `g l` | Plugins (M2b-3; not `g p`, which is Posts) |
| Anywhere | `g i` | Import (M3-2) |
| Anywhere | `g s` | Settings (M2b-3) |
| Anywhere | `g y` | System (M2b-3) |
| Anywhere | `c p` / `c g` | New post / new page |
| Lists | `j` / `k` | Move down / up |
| Lists | `x` | Select the row |
| Lists | `enter` | Open |
| Lists | `/` | Focus search |
| Lists | `#` | Trash the selection (with undo) |
| Lists | `esc` | Clear the selection |
| Comments queue | `a` / `u` | Approve / unapprove the selection (M2b-2) |
| Comments queue | `s` | Mark the selection as spam (M2b-2) |
| Comments queue | `#` | Trash the selection (M2b-2) |
| Sortable rows (handle focused) | `space` / `enter`, arrows, `esc` | Pick up, move, drop; cancel |
| Editor | `mod+s` | Save |
| Editor | `mod+shift+p` | Publish dialog |
| Editor | `mod+.` | Toggle the side panel |
| Editor | `mod+alt+r` | Revisions |
| Editor | `esc` | Close the panel |

## 7. Component inventory

The living styleguide is the lazy route `/admin/_styleguide` (P12; file
`routes/_app/[_]styleguide.tsx`, F19; section modules in `routes/_app/-styleguide/`,
which the router ignores). It is linked from nowhere (no nav or palette entry).
`?section=<id>` shows one section (`colour type layout motion controls display
navigation overlays patterns ordering data charts`); without it, the whole page. It sits behind
the `_app` login guard (Task 9, `routes/_app.tsx`). Colour values and contrast ratios on the page are computed from `tokens.css`
itself (`?raw` + `parseTokens`), so the page cannot drift from the gate.

- **shadcn (radix-nova, `components.json` → `src/styles/globals.css`), added with
  `shadcn@4.21.4 add`:** button badge card checkbox command dialog dropdown-menu input
  input-group label popover scroll-area select separator sheet sidebar skeleton sonner
  switch table tabs textarea toggle toggle-group tooltip chart calendar breadcrumb
  avatar alert-dialog hover-card kbd, plus `src/hooks/use-mobile.ts` (sidebar).
- **Kit (`src/components/kit/`):**

  | Component | Props | Notes |
  |---|---|---|
  | `PageHeader` | `title, description?, actions?, tabs?` | h1 at `2xl`; actions wrap under the title below `sm` |
  | `StatusBadge` | `status: ContentStatus` | dot + label, `text-status-*`; drafts get a hollow dot |
  | `EmptyState` | `icon, title, body, action?` | dashed frame |
  | `ErrorState` | `error: unknown, retry` | `role="alert"`; an `ApiError`'s `detail` (else `title`), anything else a generic line; never `message` or a stack |
  | `ForbiddenState` | — | `EmptyState` with a lock |
  | `MetricTile` | `label, value: number, delta?, series?, caption?, href?` | value and change are text (the change also spoken: "Up 12 on the previous period"); `caption` names the change's period ("Last 30 days", Task 12); `href` is an admin path, a query allowed (router `Link`; the query is split off into `search`, Task 13, so preloading matches the path alone); the value carries `data-slot="metric-value"` |
  | `Sparkline` | `data: number[]` | Recharts `AreaChart` (`responsive`, `accessibilityLayer={false}`), `aria-hidden` |
  | `DataTable<T>` | `columns, data, getRowId, selection?, onRowOpen?, loading, empty, rowActions?, columnClassName?, label, animateRows?, footer?, className?` | TanStack Table v9 `useTable` + `rowSelectionFeature` (`data-table-features.ts`: `dataTableFeatures`, `dataTableColumns<T>()`, `DataTableColumn<T>`); roving tabindex on rows, `j`/`k` (and arrows) move, `x` toggles selection, `enter` opens; rows are `h-row`; `rowActions` show on row hover/focus, always on coarse pointers; skeleton rows while `loading`. **Scrolling:** the frame is its own scroll container on both axes at every width (`overflow-auto`), so a wide table never spills past it; `overflow-x` makes the frame the sticky header's scroll container, so the header sticks to the frame's top (`top: 0`), not the page's, and the frame is capped at `max-h: calc(100svh - var(--sticky-top) - var(--page-pad))` so a long list scrolls inside a frame that fits below the top bar. Hide less important columns per breakpoint with `columnClassName`. No header row over an empty state. `animateRows` (Task 13): rows are `m.tr` in `AnimatePresence`, fading in and out over `--dur-base` and moving with the layout spring (`layout="position"`, the frame has `layoutScroll`); nothing animates under reduced motion. `footer` renders inside the scrolling frame, below the table (the list's "Load more" and paging sentinel). Selected rows are tinted `bg-primary/4`, not `muted`: axe found the warning-coloured "Pending review" badge below 4.5:1 on `muted` |
  | `KbdCombo` | `keys` | `+` = together, space = sequence (`g d`); `mod` is ⌘ on Apple, Ctrl elsewhere; spoken text in `sr-only` |
  | `SaveState` | `state: saved \| saving \| dirty \| error \| offline, at?` | a polite `role="status"` region holds plain text; the visible label crossfades (Motion, 120 ms opacity) |
  | `RelativeTime` | `at` (RFC 3339) | `formatDistanceStrict` against a `now` that ticks every 30 s; "just now" within 45 s (Task 13 fix round 1); absolute time in `title` and `dateTime` |
  | `ScrollFade` | `activeSelector?, activeKey?` | a row that scrolls sideways without a scrollbar (`no-scrollbar`, `globals.css`), fading the edge that hides more; keeps the active element in view on change and on resize (status tabs, filter chips on phones; Task 13) |
  | `SortableList<T>` | `items, getId, render, onReorder, label, name?, className?` | `@dnd-kit` classic API (`DndContext`, `SortableContext`, `useSortable`, `verticalListSortingStrategy`); `PointerSensor` starts after 4 px, `KeyboardSensor` with `sortableKeyboardCoordinates` (Space/Enter pick up and drop, arrows move, Esc cancels). Each row: a drag handle (`aria-label` "Reorder <label>"), the content, and a menu (`DropdownMenu modal={false}`, "Actions for <label>") with Move up / Move down, the touch and E2E path. `label(item)` names the item in every announcement (`DndContext` `accessibility.announcements` and `screenReaderInstructions`, through `t()`; never the id; "over" only when the target changes); menu moves are spoken by the list's own polite `role="status"` (`data-slot="sortable-status"`) and give focus back to the row's menu button. `onReorder` gets the whole list in its new order, once per move. Rows from a menu move travel with the layout spring (§5); while dragging, dnd-kit's transform moves them and Motion's layout is off; under reduced motion nothing animates. Handle and menu are `size-control` (44 px on touch screens), rows `min-h-row` |
  | `SortableTree<T>` | `nodes: TreeNode<T>[], render, onChange(next, moved), maxDepth, label, canNest?, name?, className?` | the same row, sensors and announcements over `flatten(nodes)`, indented 24 px a level, `aria-level` on each row. While dragging, the row's descendants travel with it (hidden until the drop) and the row is drawn as a dashed guide at the projected depth (`projectDrop`: from the horizontal offset, between the next row's depth and the previous row's + 1, the subtree within `maxDepth`, skipping a parent `canNest` refuses). The menu adds Indent / Outdent (Indent off past `maxDepth` or under a parent that cannot nest); keyboard dragging only moves up and down (a drag started from the keyboard ignores `delta.x`, which `sortableKeyboardCoordinates` takes from the rows' indents). The dragged row's dnd-kit transform keeps only its y (`SortableRow lockX`): its x is the projected depth's inset, so the pointer never offsets it twice. A projection with no allowed level (`projectDrop` returns `null`) is no move. `onChange` gets every node and `{ id, parentId, index }` of the moved one |
  | `tree-model.ts` | pure | `TreeNode<T>` (`id, parentId, data`; siblings in array order, an orphan counts as a root), `flatten` (depth-first, with `depth` and sibling `index`), `moveNode` (refuses a move into its own subtree or under an unknown parent by returning the input), `indent` (last child of the previous sibling), `outdent` (next sibling of the parent), `projectDrop(flat, activeId, overId, offsetX, indentPx = 24, maxDepth, canNest?)` (`null` for an unknown id or when even the lowest level the neighbours allow sits under a parent `canNest` refuses), `subtreeIds`, `subtreeHeight` |
  | `DropZone` | `onFiles, accept?, onRejected?, children, chooser?, className?` | `dragenter`/`dragover`/`drop` on the whole area (a counter rides out child enter/leave), an overlay "Drop files to upload" (`aria-hidden`; the button is the accessible path) while files are over it; drags without `Files` are ignored (no `preventDefault`, so they are not drop targets). "Choose files" opens a hidden `<input type="file" multiple>` (reset after each choice). `accept` is the input's attribute and also filters drops (MIME, `type/*`, `.ext`); the rest go to `onRejected`. The server types uploads from their content anyway. `chooser={false}` hides the "Choose files" row for a screen with its own Upload button (the media library) |
  | `MediaThumb` | `media: MediaDto, size: sm \| md \| lg, caption?, fill?` | a 40 / 96 / 160 px square: the smallest variant whose short side fills the box at `devicePixelRatio`, else the original (raster images); SVG only as `<img src>` of the original; other kinds a lucide type icon with the extension (md/lg). `alt` is the item's alt text; without one the image is decorative (`alt=""`) and the title (else the file name) is the visible caption (`caption={false}` leaves it to the screen, as the media grid does). `fill` makes the box a square of the parent's width (a grid tile), choosing the picture for 208 px. A trashed item shows its type icon (its URLs answer 404 while trashed) |

  Added by the M2b-2 screens (Tasks 7–13), as built:

  - `SortableList`: `rowClassName` (e.g. `items-start`, so a tall open row keeps its
    handle at the top); `RowAction.overlay` runs a menu action from the menu's
    `onCloseAutoFocus`, so a dialog or sheet it opens keeps the focus.
  - `SortableTree`: `unordered` (terms: a move that keeps the parent is no move, and the
    announcements name only the parent), `actions` (replaces the default menu),
    `leadingActions`/`trailingActions` (items around Move up/down and Indent/Outdent:
    menus put Edit before and Remove after).
  - `DataTable`: `openOnClick` (default `true`; the comments queue opens rows only with
    `enter`), `align: "middle" | "top"` (body cells only), `data-row-id` on rows.
  - `DropZone`: `single` and `labels` (the redirects import).
  - `StaleDialog` and `useStaleAsk(resource)` (`kit/`), the `ask` of `staleGuard`
    (`lib/stale-guard.ts`, P31): "Changed by someone else" with Reload / Overwrite /
    Cancel, used by the type, taxonomy, term, menu and redirect forms.
  - `tab-styles.ts` (`TAB_LIST`, `tabLinkClass`, `tabCountClass`): the link-tab row of the
    content status tabs and the comment tabs. `SelectionBar` (`content/bulk-bar.tsx`) is
    the generic floating selection bar under `BulkBar` and the comments queue.
  - Outside the kit: `openMediaPicker({ kind?, title })` (`media/media-picker.tsx`,
    `MediaPickerHost` mounted once in `routes/_app.tsx`) resolves to the chosen
    `MediaDto` or `null`; the media grid takes `single` for it.

  Added for the M2b-3 administration screens (Task 5; styleguide section
  "Administration"):

  | Component | Props | Notes |
  |---|---|---|
  | `CodeInput` | `value, onChange, onComplete?, label?, id?, describedBy?, invalid?, disabled?, autoFocus? (true), ref?` | extracted unchanged from the TOTP sign-in: one real input (`inputmode="numeric"`, `autocomplete="one-time-code"`, digits only, at most 6) over six drawn cells in two groups of three; `onComplete` fires once the sixth digit is in. `label` is the `aria-label` when no `<label>` points at `id`. On narrow screens the cells shrink before the gaps, so the dash between the groups stays clear (review fix) |
  | `QrCode` | `text, label, size? (200)` | `uqr` 0.1.3 `encode(text, { ecc: "M", border: 0 })` (`qr-matrix.ts`) gives the boolean matrix; the component draws one `<svg role="img" aria-label={label}>` with a ground `<rect>` and one 1×1 `<rect>` per dark module, a 4-module quiet zone, `shape-rendering="crispEdges"`. Black on white in both themes (`--qr-fg` / `--qr-bg`, §3). No library SVG or HTML string is used; the text is only in the drawing |
  | `SecretField` | `name, set, onChange(v: string \| null \| undefined), id?, describedBy?, invalid?, disabled?` | write-only: "Set" / "Not set", never a value. Untouched reports nothing (`undefined`: the key is left out of the `PATCH`, the server keeps it); Replace ("Set a value" when unset) opens an empty `type="password"` input (`autocomplete="new-password"`) whose text is the new value (empty = `undefined`); Remove reports `null` ("Removed when you save", Undo). The text is held only in its state |
  | `SettingsForm` | `schema: SettingDefDto[], values, secrets?, invalid?, reload, onSubmit(patch), staleKey, onDirtyChange?, disabled?` | renders `text`, `textarea`, `bool` (switch), `number`, `select` (the options; a stored value they lack is still shown), `color` (a native colour input plus the hex, `#rrggbb` checked), `media` (`MediaChoice`, `kit/media-choice.tsx`, the media picker of every kind; the editor's media field and featured image use it too) and `secret` (`SecretField`); another kind is named as not editable here. Each field: label, a "Default: …" hint, "Reset to default" when the value differs from what a reset reads as (the default, else core's `empty_setting_value`: `""`, `false`, `0`, the first option; colours compared case-insensitively with `#rgb` expanded) or is `invalid` (sends `null`, unchecked); a cleared number is "Enter a number.", not a reset; Save with a marked field focuses it and says so; `invalid` keys say "Stored value is not valid; the default is used". Save sends only the changed keys through `staleGuard` (`reload` re-reads the values; the copy compared is the values the form started from; `staleKey` names the resource in the question). `onSubmit` may resolve to the saved values, which the form then starts from; a `422` shows at its fields. New `values` from the caller replace the start copy while nothing is edited |
  | `CopyOnce` | `value, label` | read-only mono field with Copy and "Shown only now. Store it somewhere safe." (warning colour); for a new token's value, held only in the dialog's state |

  The M3-2 Import screen (`/admin/import`, Task 9, P32; `src/import/`) adds no kit
  component: it composes `DropZone`s for the WXR file and the uploads zip, `Switch`es for
  the four options with the bases under an Advanced disclosure, a stepper of the seven run
  steps with the heartbeat and counts in the active-run panel (`GET /import/jobs/{id}`
  every 2 s while queued or running, `IMPORT_POLL_MS`), and a `DataTable` of runs with
  report links. The upload is `uploadWordpressImport` in `src/api/upload.ts` (the XHR
  `send` with progress; one `Idempotency-Key` per start, kept across a `5xx` Retry, R72).
  Strings are in the `import` namespace.

  The M4-2 dashboard's Site traffic (Task 9, P36; `src/dashboard/site-metrics.tsx`,
  `traffic-card.tsx`, `performance-card.tsx`, `metrics-series.ts`) adds no kit component
  but two patterns. **One range control for a group of charts:** a single-select
  `ToggleGroup` (`spacing={0}`, `role="radiogroup"`, "Time range"; each item selects on
  focus, so an arrow key moves and selects as the ARIA radio pattern expects, where Radix
  alone only moves focus) sits once in the
  group's header row above the cards it scopes, never inside one card, so every number
  in the group covers the same hours; a switch keeps the previous figures at 60 % opacity
  with `aria-busy` until the new ones arrive (`keepPreviousData`), no skeleton.
  **A polling card that fails keeps its figures:** an error state only without data;
  after a failed refresh, `StaleNote` (`dashboard/panel.tsx`, `role="status"`) says from
  when the shown figures are, and the query has `INLINE_ERRORS` (no toast each minute).
  Two measures of different size get separate charts (5xx errors as a bar strip under
  the traffic areas, sharing the x axis and y-axis width), never two y-scales; hours
  without a measure are gaps (`connectNulls={false}`) with a marker for an isolated
  point; every chart has its numbers as text (totals, headlines, a hidden table).

  `useStaleAsk` now settles a pending question with `cancel` when a second one is asked
  or the component unmounts (an M2b-2 minor). Strings of these components are in the
  `admin` namespace (the administration screens add theirs there too).

  `ContentStatus` and `CONTENT_STATUSES` live in `src/api/content.ts`. Kit and
  styleguide strings are in the `common` (`ui.*`, `status.*`, `kit.*`) and `styleguide`
  namespaces; the M2b-2 kit (sortable list and tree, drop zone) has its own `kit`
  namespace. `@dnd-kit` is imported only by the kit's sortable components, which only
  lazy routes use, so it stays out of the entry chunk.

Rules for generated files:

- Restyled only through tokens. Token values never go into `globals.css` or a
  component: after `shadcn add`, revert anything the CLI wrote into `globals.css`
  (`:root`/`.dark` blocks, `@theme` lines) — every variable it needs already exists in
  `tokens.css` — and restore the pins (F13). Task 8's `add` wrote nothing into
  `globals.css`; it set `recharts` to 3.8.0 and added `next-themes ^0.4.6`; both were
  restored/removed (`pnpm add -E recharts@3.10.1`, `pnpm remove next-themes`), and the
  lockfile ended byte-identical to before.
- Generated files are prettier-formatted once after `add`; ESLint turns off
  `react-refresh/only-export-components` and `react-hooks/set-state-in-effect` for
  `src/components/ui/**` and `src/hooks/use-mobile.ts` (F16). `chart.tsx` also gets
  `no-unsafe-assignment`, `no-unsafe-member-access`, `no-unsafe-argument`,
  `restrict-template-expressions` and `no-unnecessary-type-assertion` off: it types
  Recharts' payloads as `any`.
- English sr-only/aria strings go through `t("common:…")` (F31).
- Focus: the base layer draws `outline: 2px solid var(--ring); outline-offset: 2px` on
  `:focus-visible`. Generated `outline-none`/`outline-hidden` and `focus-visible:ring-*`
  classes hide it; Task 8 removed them from every interactive control (list below), so
  each shows the standard ring. Items inside clipping popovers and table rows draw the
  same ring inset (`-outline-offset-2`).
- Overlays that are not controls keep `outline-none` (dialog, alert-dialog, popover and
  hover-card content, command list): they only take focus when they hold nothing
  focusable. Command items never take focus (cmdk keeps it in the input).
- Menus: use `DropdownMenu modal={false}`. Radix's modal mode marks the rest of the page
  `aria-hidden` while it stays focusable, which axe reports as `aria-hidden-focus`
  (checked in Task 8; `Dialog`, `AlertDialog` and `Sheet` pass).
- Do not put `CommandSeparator` inside `CommandList`: a `separator` child of a `listbox`
  fails axe `aria-required-children`.
- Do not put `Kbd`/`KbdCombo` inside a filled (primary) button: their muted colours fail
  contrast there; show the shortcut in the button's tooltip.
- `no_raw_colours_outside_tokens` allowlists exactly these strings, only in
  `src/components/ui/chart.tsx` (F30): `[stroke='#ccc']` and `[stroke='#fff']` — the
  shadcn chart's selectors for Recharts' default grid/axis and dot/sector strokes. The
  generated file holds exactly three `#ccc` and two `#fff` (checked 2026-10-25), nothing
  else, and no other file may hold a colour literal.

**Every edit to a generated file (Task 8):**

| File | Edit | Why |
|---|---|---|
| `button.tsx` | removed `outline-none`, `focus-visible:border-ring`, `focus-visible:ring-3`, `focus-visible:ring-ring/50`; in `destructive` removed `focus-visible:border-destructive/40 focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40` | standard focus ring |
| `button.tsx` | `default` hover `bg-primary/80` → `bg-primary-hover` (token, §3) | `primary/80` gave 3.64:1 for the label (axe on the forced hover state); a mix toward `foreground` was nearly invisible in dark mode (fix round 1) |
| `button.tsx` | `destructive` hover `bg-destructive/20` (dark `/30`) → `hover:border-destructive/50` | the tint gave 3.88:1 (light) / 3.91:1 (dark) |
| `button.tsx` | size `default` `h-8 px-2.5` → `h-control px-3` (icon padding 2 → 2.5); `icon` `size-8` → `size-control`; `lg` `h-9` → `h-[calc(var(--control-h)+4px)] px-3.5`; `icon-lg` likewise | buttons follow density (§4) and touch height; `lg` stays above default |
| `badge.tsx` | removed the `focus-visible:*` ring classes (both places); link hover `bg-primary/80` → `bg-primary-hover` | standard focus ring; contrast |
| `checkbox.tsx` | removed `outline-none` and the `focus-visible:*` ring classes; `group/checkbox`; indeterminate shows `MinusIcon` and the filled primary style, with `dark:data-[state=indeterminate]:bg-primary` because the generated `dark:bg-input/30` outranks the plain variant (fix round 1) | standard focus ring; "select all" with some rows selected |
| `input.tsx` | removed `outline-none` and the `focus-visible:*` ring classes; `h-8` → `h-control` | focus ring; density |
| `textarea.tsx` | removed `outline-none` and the `focus-visible:*` ring classes | focus ring |
| `input-group.tsx` | group: removed `outline-none` and the `has-[…:focus-visible]` border/ring classes, added `has-[[data-slot=input-group-control]:focus-visible]:outline-2 …:outline-offset-2 …:outline-ring`; `h-8` → `h-control`; controls: `focus-visible:ring-0` → `focus-visible:outline-none` | the ring goes around the group, not the inner input |
| `select.tsx` | trigger: removed `outline-none` and ring classes, `data-[size=default]:h-8` → `h-control`; item: `outline-hidden` → `-outline-offset-2` | focus ring; density |
| `switch.tsx` | removed `outline-none` and the `focus-visible:*` ring classes | focus ring |
| `toggle.tsx` | removed `outline-none` and ring classes; `default` size `h-8 min-w-8` → `h-control min-w-control`; pressed `bg-muted` → `border-primary/50 bg-primary/10 text-foreground` with an ember icon | focus ring; density; `bg-muted` was nearly invisible on `card` (0.965 on 1), and ember text on the tint gave 4.42:1 |
| `tabs.tsx` | trigger: removed the `focus-visible:*` ring/outline classes, `text-foreground/60` → `text-muted-foreground`; content: removed `outline-none` | focus ring (panels are focusable); inactive tabs were 4.48:1 on `muted` |
| `dropdown-menu.tsx` | item, checkbox item, radio item, sub-trigger: `outline-hidden` → `-outline-offset-2` | focus ring, inset inside the menu |
| `scroll-area.tsx` | viewport: removed `outline-none` and ring classes, added `-outline-offset-2` and `tabIndex={0}` | a scrollable region must be keyboard-reachable (axe `scrollable-region-focusable`) |
| `sidebar.tsx` | group label, group action, menu button, menu action, sub button: removed `outline-hidden focus-visible:ring-2`; strings through `t()` | focus ring; F31 |
| `sidebar.tsx` (Task 10) | removed the `sidebar_state` cookie write and the global `mod+b` `keydown` listener | the shell stores the state per viewer (`ferrite.admin.sidebar` in `localStorage`, `preferences.ts`) and binds `mod+b` through `shortcuts.ts`, which leaves text fields and the editor alone (TipTap's bold is `mod+b`) |
| `sidebar.tsx` (Task 10) | `SidebarInset` renders a `div`, not `main` | the shell puts its top bar (a banner) and its own `<main id="main">` inside |
| `sidebar.tsx` (Task 10) | `data-active={isActive \|\| undefined}` (menu and sub buttons) | `data-active="false"` would match Tailwind's `data-active:` variant (it tests presence), styling every row as active |
| `sidebar.tsx` (Task 10) | mobile `SheetContent` `onCloseAutoFocus` focuses `[data-sidebar=trigger]` | the sheet has no `SheetTrigger`, so Radix left focus on `<body>` after Escape |
| `calendar.tsx` | day: removed `group-data-[focused=true]/day:border-ring …:ring-[3px] …:ring-ring/50` | the day button (a `Button`) shows the standard ring |
| `chart.tsx` | removed `[&_.recharts-surface]:outline-hidden` | Recharts 3's accessibility layer makes the surface focusable |
| `command.tsx` | input: `outline-hidden` → `focus-visible:outline-none`; its `InputGroup` gets `has-[[data-slot=command-input]:focus-visible]:outline-2 …:outline-offset-2 …:outline-ring`; `CommandDialog` title/description default to `t("ui.commandPalette")` / `t("ui.commandPaletteDescription")` | focus ring on the field; F31 |
| `dialog.tsx`, `sheet.tsx`, `alert-dialog.tsx` | overlay `bg-black/10` → `bg-overlay`; "Close" → `t("ui.close")` (dialog, sheet) | mode-aware scrim token; F31 |
| `breadcrumb.tsx` | `aria-label="breadcrumb"` → `t("ui.breadcrumb")`; "More" → `t("ui.more")` | F31 |
| `sonner.tsx` | `useTheme` from `@/design/theme` instead of `next-themes`; dropped the `as ToasterProps["theme"]` cast | P9 (the cast was unnecessary with our typed theme) |

Tailwind, CSS and lint config edits made in Task 8:

- `globals.css`: `@custom-variant hover` = Tailwind's own `@media (hover: hover) {
  &:hover }` plus `&:is(.force-hover)`, so `.force-hover` shows every `hover:` and
  `group-hover:` style; the base `:focus-visible` rule also matches `.force-focus`. Both
  classes exist for the styleguide's state matrices.
- `globals.css`: `--color-overlay`, `--color-primary-hover`; `tokens.css`: `--overlay`,
  `--primary-hover`, `--sticky-top` and the coarse-pointer block (§3, §4); `tokens.ts`:
  `overlay` in the surface group, `primary-hover` in brand (and its contrast pair).
- Numbers and dates in the kit format with `uiLocale()` (`src/lib/locale.ts`: the
  document's `lang`, else the browser's), not a hard-coded `"en"`. `RelativeTime` still
  uses date-fns' default (English) locale; M2b ships `en` only.
- `eslint.config.js`: the `chart.tsx` override above.
- `e2e/support/snap.ts`: `settle()` lets two frames pass before collecting animations,
  because a colour-scheme change reaches `<html>` through a React effect and the colour
  transitions start only after it (axe otherwise sampled mid-transition).

Tailwind and lint config edits made in Task 7:

- `eslint.config.js` turns off `react-refresh/only-export-components` for
  `src/design/theme.tsx` and `src/design/density.tsx` (a provider and its hook share a
  module; the import path `@/design/theme` is fixed by the plan).
- `vitest.config.ts` (`app` project) sets `css.include` to `src/styles/*.css`, so the
  gate can read them as `?raw` text (Vitest blanks CSS otherwise).

**The shell (Task 10).** `routes/_app.tsx` is the signed-in layout: the login guard,
then `SidebarProvider` (controlled; state per viewer) → skip link, `AppSidebar`
(`shell/app-sidebar.tsx`), `SidebarInset` with `TopBar` (`shell/top-bar.tsx`) and
`<main id="main">` holding `RouteTransition`, plus the shell's shortcuts, the ⌘K
palette (`palette/palette.tsx`, Task 11, below), the
`?` sheet (`shell/shortcut-sheet.tsx`) and the app's `<Toaster>` (bottom right; the bare
styleguide mounts its own for its toast specimens).

- Nav (`shell/nav.ts` `navFor(me, types)`): Overview (Dashboard, `g d`), Content (Posts,
  Pages, then custom types by label, never `media`) when `me.capabilities` holds
  `read_admin`. M2b-2 screens add a `ScreenEntry` (`screen`, `to`, `label` key,
  `icon`) to the readonly `SCREEN_ENTRIES` literal in `nav.ts` once their route exists
  (never at runtime) (ruling F-A); `SCREENS` fixes where
  each goes and who sees it, all behind `read_admin` (R26): Content gets Media
  (`read_admin`; `g m`; read-only for contributors) and Comments (`moderate_comments`;
  `g c`; a pending-count badge, added by `withBadges` for the sidebar only, `aria-hidden`
  with an sr-only "(n waiting)" on the link), and a Structure section holds Content
  types (`manage_settings`), Taxonomies (`manage_settings` or `manage_terms`), Menus
  (`manage_menus`) and Redirects (`manage_redirects`), in that order; all six entries are
  registered as of M2b-2. The palette's Create group adds "Upload media", "New content
  type", "New taxonomy", "New menu" and "New redirect" where the viewer may. Rows are `h-control` (36 px comfortable, 30 px compact, 44 px under
  `pointer: coarse`); the top bar stays 56 px in both densities (it is `--sticky-top`).
  The active row is
  tinted (`sidebar-accent`) with its icon in `primary`. Tooltips name the icons only on
  the collapsed rail.
- Top bar (56 px, sticky, translucent with a blur): sidebar toggle (tooltip shows
  `mod+b`), breadcrumbs from the matched routes' `staticData.crumb` (`(t) => string`,
  `shell` namespace; typed in `router.tsx`), the "Search engines discouraged" pill
  (`reading.discourage_indexing`; a popover button, so a tap or Enter shows what it
  means — its text is hidden below `lg`, and a `title` never shows on touch), the ⌘K
  trigger (an input-like button at `md`+, an icon below), the "New" split button
  (`edit_own`; new post, menu: new page) and the account menu (`shell/user-menu.tsx`:
  name, role, e-mail, theme and density radio groups, shortcuts, sign out).
- `staticData.bare: true` renders a route under `_app` without the shell (the
  styleguide, which has its own frame).
- The document title is one `<title>` in `__root.tsx`: the deepest crumb, then
  "FerriteCMS".
- States: the router's `defaultErrorComponent` (`shell/error-boundary.tsx`) shows an
  `ApiError` 403 as `ForbiddenState`, 404 as the not-found screen, anything else as
  `ErrorState` with a retry (`router.invalidate()`); `defaultNotFoundComponent` and the
  catch-all `routes/_app/$.tsx` show `shell/not-found.tsx` (pre-flight F20: no
  `/not-found` route; the address stays as typed, and the guard runs first). `main.tsx`'s
  `onCaughtError` (`lib/caught-error.ts`, unit-tested) does not log an `ApiError` an
  error component rendered (a handled answer); everything else is still logged.
- Crumb links: `match.pathname` excludes the `/admin` basepath, so a non-last crumb's
  `Link` resolves once (`/admin/parent`; `non_last_crumb_links_inside_the_admin_once`).
- Toasts: the app's `<Toaster>` is mounted only in the shell, and the global error
  handler does not toast `rate-limited`/`busy` on the sign-in, reset and forced-change
  screens, which answer inline: a `429` with a countdown from `Retry-After`, a `503
  busy` with their generic error (sonner replays still-active toasts when a `<Toaster>`
  mounts, so the handler, not only the mount point, has to hold back).

**The content list (Task 13).** `routes/_app/content.$type.index.tsx`, modules in
`src/content/`:

- URL state (`list-search.ts`, Zod): `status` (absent = all but trash), `q`, `mine`,
  `term`, `order` (absent = `updated`); defaults never reach the URL and an invalid
  member is dropped. `mine` is sent as `author=<me.id>` (F34). A search drops `author`,
  `term` and `order` from the request (07 §1); the Mine, term and sort chips stay in
  the URL but show `aria-disabled` with a tooltip saying why.
- The loader reads the types and the type's counts (`GET /content/{type}/counts`): a
  missing type (or `media`) is the not-found screen, a `403`/`500` the shell's forbidden
  and error states (shell.spec's route-state screenshots intercept that read). The rows
  load in the component (`useInfiniteQuery` over `next_cursor`, 50 a page, the previous
  rows dimmed to 60 % while a new filter loads). The crumb and document title are the
  type's label (`staticData.crumb(t, loaderData)`).
- Header: label, count (all but trash), "New post/page/entry" (`edit_own`). Status tabs
  are links with counts (`nav` "Status", `aria-current`), not ARIA tabs (there is no
  panel). Toolbar: search (`/`, 200 ms debounce, Esc clears), Mine, one dashed "+
  <taxonomy>" chip per taxonomy of the type (Popover + Command, ≤ 200 terms; a chosen
  term shows "<taxonomy> <term>" with a separate remove button), the sort menu, "Clear
  filters".
- Columns: select; title (a link, `tabIndex=-1`; two lines on phones) with the slug in
  mono under it (F6; a hierarchical type prefixes the parent's slug); parent
  (hierarchical only: from the loaded rows, else one `GET` per missing parent); status
  with the scheduled time; author (initials + display name); date (published, else last
  updated, labelled). Below `md` status and date fold into a line under the title.
  Row actions: Edit and Trash/Restore icons at `md`+, and a menu (Edit, Preview
  `/_preview/<id>` in a new tab for anything outside the trash, Restore, Move to trash,
  Delete permanently), each from `allowed` (Review Focus 4).
- Bulk (`bulk.ts` `runBulk`, `list-actions.ts`): a floating selection bar (count,
  "Publish 2" when fewer than the selection qualify, Esc clears) fans out 4 requests at
  a time with each item's `If-Match`; failures stay on screen in a report (a `412`
  reads "Someone changed it since this list loaded"); a `401` among them goes to the
  global handler. Trash (row, bulk or `#`) shows "N moved to trash" with Undo for 10 s;
  Undo restores and re-applies the old status (F36: published → publish, pending →
  submit, scheduled → schedule if still ahead; private or a lapsed schedule come back
  as drafts with a notice). Undo calls the API on each item's own type (the toast
  outlives a navigation) and is offered only for trashed items with `allowed.restore`;
  when none of the items to trash has `allowed.trash_undoable` (a contributor's own
  drafts) the list asks first ("You won't be able to restore …") and the toast has no
  Undo; a mixed trash says how many Undo restores. Besides that, Delete permanently is
  the only confirm (`AlertDialog`, a filled destructive button: the tinted one fails
  contrast on the dialog footer). The selection bar centres on `<main>` (measured), not
  the viewport. Failure reports belong to the list they happened on.
- Empty states per tab, for a search ("No results for “…”" + Clear search) and for
  other filters; skeleton rows while the first page loads; `ErrorState` when it fails.

**The command palette (Task 11).** `palette/palette.tsx`: the shadcn `CommandDialog`,
opened by `mod+k` (`allowInInput`, so it also closes from its own field) and the top
bar's trigger (`aria-haspopup="dialog"`, `aria-keyshortcuts`).

- Groups, in this order without a query: **Recent** (`palette/recent.ts`: the last 8
  screens and items, newest first, per viewer in `localStorage`
  `ferrite.admin.recent.<user id>`, read in try/catch; the shell pushes every nav screen
  the viewer lands on, the palette pushes each item it opens; the screen already open is
  left out; only shown before anything is typed, so no row appears twice; the `_app`
  guard (`enterShell`) sets the viewer before any child renders and `logout()` (and a
  `401` in the guard) clears it, so nothing is read or written while signed out), **Go to**
  (every `navFor` item with its chord, and the `?` sheet), **Create** (new post, new
  page, "New entry in <label>" per custom type the nav shows; only with `edit_own`; each
  goes to `/content/<type>/new`), **Content** (live search) and **Preferences** (theme
  light/dark/system, density; the current one is checked and announced "Current").
- Filtering: cmdk's own `defaultFilter` ranks the static rows (each row's value is its
  label, with keywords such as "go to", "create", "theme"). Content hits are already
  matched by the server, so the palette's `filter` gives every one a fixed 0.5: they
  keep the server's order. Groups stay in render order (cmdk 1.1.1 sorts rows inside
  a group; its group sort looks groups up by id in `data-value`, which holds the
  heading, so it never moves one), and Content renders last: a few exact static
  matches ("theme", "new post") come first, then the open-ended hits. The query's words are
  marked in hit titles (semibold, ember underline); the type chip shows only when the
  hits span more than one type. The search query carries `meta: INLINE_ERRORS`
  (`api/client.ts`): the global handler still redirects on `401`, but does not toast,
  since the palette says the search failed itself. `vimBindings` is off
  (cmdk's Ctrl+K would swallow `mod+k` off Apple platforms); arrows loop.
- Search (`palette/sources.ts` `searchContent(q, types, signal)`): from 2 characters
  (trimmed), it waits 150 ms (abortable), then sends one `GET /content/{type}?q=…&limit=8`
  per type the nav shows, in parallel; hits come by type (nav order), then the server's
  relevance order. A failing type is left out; all failing shows "Content search
  failed". TanStack Query (`qk.paletteSearch`) aborts the previous round when the query
  changes and keeps the last answer up while the next loads (`keepPreviousData`).
  `PaletteHit` has no permalink (F6): a hit shows title, slug (mono), status dot with
  its label, and the type. Opening a hit goes to `/content/<type>/<id>`.
- Loading without flicker: nothing changes for the first 250 ms of a search; after
  that, a first search shows three skeleton rows under "Content" and a later one dims
  the previous hits to 60 %. The footer's `role="status"` says "Searching content…" or
  "N content results" (it is visually hidden on phones, still read).
- Empty: a centred icon, "No results for “…”" and a hint, drawn over the list, not
  inside it (`useCommandState`): a listbox holding text but no option fails axe
  `aria-required-children`, so `CommandEmpty` is not used.
- Frame: pinned at 12 vh from the top, 40 rem wide, a fixed list height
  (`min(26rem, 76svh − 7rem)`), so it never jumps while results change; on phones a
  near full-screen sheet (`100dvh − 1rem`) with a "Cancel" button instead of the `Esc`
  hint and the footer. Enter: fade, scale 0.98 and 4 px down over `--dur-fast`; exit
  over `--dur-instant`.
- The input has no focus ring or fill (Linear, Raycast): the open dialog is the focused
  context, separated by the header's bottom border; the keyboard position is the active
  row: `bg-muted`, a 2 px ember bar at its left edge and its icon tile in ember. Rows
  are 40 px (48 px under `pointer: coarse`). Every opening starts empty: the body is
  keyed per opening, so a reopening during the closing animation does not keep the
  old query.

## 8. Design review checklist

Every UI task's contact sheet (`admin/test-results/screens/index.html`; light/dark ×
1440×900/390×844) is checked against this list; a screen that fails it is rejected like
a failing test. Since M2b-3 the snapshots are taken only in the `chromium-sqlite` E2E project
(P55); the other engines run the same specs without them.

- [ ] Alignment to the 4 px grid.
- [ ] One primary action per view.
- [ ] Consistent page header (title, description, actions, tabs).
- [ ] Loading skeletons, empty, error and permission-denied states present.
- [ ] Dark mode at parity: no hard-coded colours; `grep -E "#[0-9a-f]{3,6}|rgb\(" src`
  finds none outside `tokens.css` (and the F30 chart selectors). The unit test
  `no_raw_colours_outside_tokens` is stricter: it also catches `rgba(`, `hsl(`,
  `hsla(`, `oklch(`, `oklab(`, `lab(`, `lch(`, `hwb(` and the mode-blind
  `bg|text|border|ring|…-white|black` utilities (`bg-black/50` included; shadcn
  overlays need `bg-foreground/…` or a token instead).
- [ ] Mobile width without horizontal scroll and with ≥ 44 px touch targets.
- [ ] Text never truncated without a tooltip or `title`.
- [ ] Numbers tabular.
- [ ] Focus visible (§7 focus rule).
- [ ] Motion within §5.
- [ ] Every screen visit also passes `expectAccessible` (axe, WCAG 2.2 AA tags) in both
  schemes, and the guard fixture (no CSP violation, page error or `console.error`).
