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
  - Flash-free: `system` on any OS (the default), and an explicit choice that matches
    the OS.
  - Still flashes one frame: an explicit choice against the OS (stored `dark` on a light
    OS, or `light` on a dark one), because `localStorage` is unreadable without JS.
    Fixing it needs a server hint (for example a cookie that `index.html` serving turns
    into a class); deferred.
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
  (opacity plus 4 px y), the save-state indicator. Nothing loops, and nothing moves more
  than 8 px except sheets.
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
the rest arrive with their screens (`g p`/`g g` with the lists, Task 13; `c p`/`c g`
with the editor, Task 14; `g m`, `g s` in M2b-2/3). The `?` sheet lists whatever is
registered at the moment it opens.

| Where | Keys | Action |
|---|---|---|
| Anywhere | `mod+k` | Command palette |
| Anywhere (not in fields or the editor) | `mod+b` | Collapse / expand the sidebar |
| Anywhere | `?` | Shortcut sheet |
| Anywhere | `g d` | Dashboard |
| Anywhere | `g p` | Posts |
| Anywhere | `g g` | Pages |
| Anywhere | `g m` | Media (M2b-2) |
| Anywhere | `g s` | Settings (M2b-3) |
| Anywhere | `c p` / `c g` | New post / new page |
| Lists | `j` / `k` | Move down / up |
| Lists | `x` | Select the row |
| Lists | `enter` | Open |
| Lists | `/` | Focus search |
| Lists | `#` | Trash the selection (with undo) |
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
navigation overlays patterns data charts`); without it, the whole page. It sits behind
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
  | `MetricTile` | `label, value: number, delta?, series?, href?` | value and change are text (the change also spoken: "Up 12 on the previous period"); `href` is an admin path (router `Link`) |
  | `Sparkline` | `data: number[]` | Recharts `AreaChart` (`responsive`, `accessibilityLayer={false}`), `aria-hidden` |
  | `DataTable<T>` | `columns, data, getRowId, selection?, onRowOpen?, loading, empty, rowActions?, columnClassName?, label, className?` | TanStack Table v9 `useTable` + `rowSelectionFeature` (`data-table-features.ts`: `dataTableFeatures`, `dataTableColumns<T>()`, `DataTableColumn<T>`); roving tabindex on rows, `j`/`k` (and arrows) move, `x` toggles selection, `enter` opens; rows are `h-row`; `rowActions` show on row hover/focus, always on coarse pointers; skeleton rows while `loading`. **Scrolling:** the frame is its own scroll container on both axes at every width (`overflow-auto`), so a wide table never spills past it; `overflow-x` makes the frame the sticky header's scroll container, so the header sticks to the frame's top (`top: 0`), not the page's, and the frame is capped at `max-h: calc(100svh - var(--sticky-top) - var(--page-pad))` so a long list scrolls inside a frame that fits below the top bar. Hide less important columns per breakpoint with `columnClassName` |
  | `KbdCombo` | `keys` | `+` = together, space = sequence (`g d`); `mod` is ⌘ on Apple, Ctrl elsewhere; spoken text in `sr-only` |
  | `SaveState` | `state: saved \| saving \| dirty \| error \| offline, at?` | a polite `role="status"` region holds plain text; the visible label crossfades (Motion, 120 ms opacity) |
  | `RelativeTime` | `at` (RFC 3339) | `formatDistanceToNowStrict`, absolute time in `title` and `dateTime`; re-renders each minute |

  `ContentStatus` and `CONTENT_STATUSES` live in `src/api/content.ts`. Kit and
  styleguide strings are in the `common` (`ui.*`, `status.*`, `kit.*`) and `styleguide`
  namespaces.

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
palette (`shell/go-to-palette.tsx`, a minimal "Go to" list that Task 11 replaces), the
`?` sheet (`shell/shortcut-sheet.tsx`) and the app's `<Toaster>` (bottom right; the bare
styleguide mounts its own for its toast specimens).

- Nav (`shell/nav.ts` `navFor(me, types)`): Overview (Dashboard, `g d`), Content (Posts,
  Pages, then custom types by label, never `media`) when `me.capabilities` holds
  `read_admin`. Rows are `h-control` (36 px comfortable, 30 px compact, 44 px under
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

## 8. Design review checklist

Every UI task's contact sheet (`admin/test-results/screens/index.html`; light/dark ×
1440×900/390×844) is checked against this list; a screen that fails it is rejected like
a failing test.

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
