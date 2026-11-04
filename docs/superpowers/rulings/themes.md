# Rulings: the starter themes' redesign (2026-11-04)

The redesign of `themes/blog`, `themes/docs` and `themes/business` (design 06 §8). Each
ruling names what it changes, why, and the cost if it is wrong.

- **T1 (amends M4-3b P65 and P60):** each embedded starter theme may hold its fonts as
  latin-subset variable `woff2` files under `assets/fonts/`, each with its SIL OFL 1.1
  licence text next to it, and the size cap per theme rises from 150 000 bytes to
  512 KiB (`crates/ferrite-theme/tests/theme/load.rs`). Images and scripts stay out of
  `business`. The fonts: `blog` Newsreader (roman and italic, optical sizes) with Inter
  and JetBrains Mono; `docs` Inter and JetBrains Mono (the files the docs site's brand
  layer already ships); `business` Bricolage Grotesque. Each `@font-face` has
  `font-display: swap`, the latin `unicode-range` and a system fallback stack, so other
  scripts render in system fonts. Why: type carries most of a theme's look, and the
  system stacks differ per platform. Cost if wrong: about 0.9 MB more in every binary;
  the files can go again with the cap back at 150 000.
- **T2 (`blog`):** the default `accent_color` is `#be185d` (was `#c2410c`): the theme's
  new identity is a cool white page with a raspberry accent, not a warm orange one. A
  site that set the colour keeps its value. Cost if wrong: one default to change back.
- **T3 (`docs`):** `docs.js` may be up to 4 KiB (was 3): it also sets `data-lang` on a
  code block from its `language-*` class (the stylesheet prints it as a label; the brand
  layer's `brand.js` sets the same attribute with its own names), marks the "On this
  page" entry of the section being read (`is-active`, the class `brand.js` uses too) and
  focuses the header search (`id="search"`) on `/`. Still no network requests, still
  progressive. Cost if wrong: a test limit.
- **T4 (all three):** a written excerpt only: `blog`'s standfirst, `docs`' lead and
  `business`' page and hero ledes show `post.excerpt` only when its first six words
  (`excerpt(6)`) differ from the body's. Core stores an excerpt made from the body when
  none was written, which would repeat the first paragraph right under the title. An
  excerpt written to start like the body is hidden too, which is the same repetition.
  Listings (the lead story, cards) still show every excerpt. Cost if wrong: an excerpt
  that should show does not; a condition to drop.
- **T5 (`business`):** a closing band (`partials/cta-band.html`) above the footer shows
  the site's tagline (or title) and the CTA button, under the same rules as the header's
  button (none on the 200 page the CTA links to, none when `cta.html` shows nothing). It
  does not depend on the request path otherwise, so the shared 404 body stays
  path-independent. No testimonials section: no content type or field holds them.
