// The FerriteCMS documentation skin's script (site/brand/). Progressive enhancement:
// every page reads and navigates without it. Adds the light/dark switch, the search
// dialog over the Pagefind index (`<base>/pagefind/`, made by export-static.sh), code
// labels and highlighting, the active "On this page" entry and the landing page tabs.
(() => {
  "use strict";
  const root = document.documentElement;
  const script = document.currentScript;
  const base = script ? new URL(script.src).pathname.replace(/\/brand\/brand\.js$/, "") : "";
  const apple = /mac|iphone|ipad/i.test(navigator.platform || "");

  // Theme ---------------------------------------------------------------
  const KEY = "ferrite-docs-theme";
  const dark = () =>
    root.dataset.theme ? root.dataset.theme === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
  const syncTheme = () => {
    const isDark = dark();
    for (const b of document.querySelectorAll("[data-theme-toggle]")) {
      b.setAttribute("aria-label", isDark ? "Switch to light mode" : "Switch to dark mode");
    }
    // Pictures follow the page's scheme, not only the system's.
    for (const s of document.querySelectorAll("source[data-dark]")) {
      s.media = isDark ? "all" : "not all";
    }
  };
  for (const b of document.querySelectorAll("[data-theme-toggle]")) {
    b.addEventListener("click", () => {
      const next = dark() ? "light" : "dark";
      root.dataset.theme = next;
      try {
        localStorage.setItem(KEY, next);
      } catch {
        /* storage blocked: the choice lasts for this page */
      }
      syncTheme();
    });
  }
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", syncTheme);
  syncTheme();

  // Code ---------------------------------------------------------------
  const LABEL = { sh: "Shell", toml: "TOML", rust: "Rust", jinja: "Jinja", text: "" };
  for (const code of document.querySelectorAll(".prose pre > code[class*='language-']")) {
    const lang = (code.className.match(/language-([\w-]+)/) || [])[1];
    const block = code.closest(".code-block");
    if (block && LABEL[lang] !== "") block.dataset.lang = LABEL[lang] || lang;
    if (lang === "jinja") code.classList.replace("language-jinja", "language-django");
  }
  const highlight = () => {
    if (window.Prism && Prism.highlightAllUnder) Prism.highlightAllUnder(document.querySelector(".prose") || document.body);
  };
  if (window.Prism && Prism.highlightAllUnder) highlight();
  else window.addEventListener("load", highlight, { once: true });

  for (const b of document.querySelectorAll("[data-copy-target]")) {
    b.addEventListener("click", async () => {
      const el = document.getElementById(b.dataset.copyTarget);
      const text = [...el.textContent.split("\n")]
        .filter((l) => l.startsWith("$ "))
        .map((l) => l.slice(2))
        .join("\n");
      try {
        await navigator.clipboard.writeText(text);
        b.textContent = "Copied";
      } catch {
        b.textContent = "Not copied";
      }
      setTimeout(() => (b.textContent = "Copy"), 1800);
    });
  }

  // On this page ---------------------------------------------------------
  const tocLinks = [...document.querySelectorAll(".docs-toc a[href^='#']")];
  if (tocLinks.length && "IntersectionObserver" in window) {
    const byId = new Map(tocLinks.map((a) => [decodeURIComponent(a.hash.slice(1)), a]));
    const heads = [...byId.keys()].map((id) => document.getElementById(id)).filter(Boolean);
    const visible = new Set();
    const mark = () => {
      let current = heads.find((h) => visible.has(h));
      if (!current) {
        current = heads.filter((h) => h.getBoundingClientRect().top < 120).pop() || heads[0];
      }
      for (const a of tocLinks) a.classList.toggle("is-active", byId.get(current.id) === a);
    };
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) (e.isIntersecting ? visible.add(e.target) : visible.delete(e.target));
        mark();
      },
      { rootMargin: "-64px 0px -60% 0px" },
    );
    heads.forEach((h) => io.observe(h));
  }

  // Current section in the header ----------------------------------------
  for (const a of document.querySelectorAll(".bx-nav a")) {
    if (location.pathname.startsWith(a.pathname)) a.setAttribute("aria-current", "true");
  }

  // Landing page tabs ----------------------------------------------------
  const tabs = [...document.querySelectorAll(".bx-tabs [role='tab']")];
  const select = (tab, focus) => {
    for (const t of tabs) {
      const on = t === tab;
      t.setAttribute("aria-selected", String(on));
      t.tabIndex = on ? 0 : -1;
      document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
    }
    if (focus) tab.focus();
  };
  tabs.forEach((t, i) => {
    t.addEventListener("click", () => select(t, false));
    t.addEventListener("keydown", (e) => {
      const d = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
      if (d) {
        e.preventDefault();
        select(tabs[(i + d + tabs.length) % tabs.length], true);
      }
    });
  });

  // Search ----------------------------------------------------------------
  const dialog = document.querySelector(".bx-search");
  if (!dialog || typeof dialog.showModal !== "function") return;
  for (const k of document.querySelectorAll("[data-search-key]")) k.textContent = apple ? "⌘K" : "Ctrl K";
  const input = dialog.querySelector("input");
  const list = dialog.querySelector(".bx-search__results");
  let pagefind;
  let active = -1;
  let seq = 0;
  const empty = (text) => {
    list.replaceChildren();
    list.dataset.empty = text;
  };
  const load = async () => {
    if (!pagefind) {
      pagefind = await import(`${base}/pagefind/pagefind.js`);
      await pagefind.options({ baseUrl: `${base}/` });
      pagefind.init();
    }
    return pagefind;
  };
  const open = () => {
    if (dialog.open) return;
    dialog.showModal();
    input.select();
    empty("Type to search the documentation");
    load().catch(() => empty("Search is not available on this copy of the site"));
  };
  const pick = (i) => {
    const items = [...list.querySelectorAll(".bx-result")];
    if (!items.length) return;
    active = (i + items.length) % items.length;
    items.forEach((el, j) => el.setAttribute("aria-selected", String(j === active)));
    items[active].scrollIntoView({ block: "nearest" });
  };
  const render = (results) => {
    list.replaceChildren();
    active = -1;
    if (!results.length) return empty("No pages match");
    for (const r of results) {
      const a = document.createElement("a");
      a.className = "bx-result";
      a.setAttribute("role", "option");
      a.href = r.url;
      const path = document.createElement("span");
      path.className = "bx-result__path";
      path.textContent = r.url
        .replace(base, "")
        .split("/")
        .filter(Boolean)
        .slice(0, -1)
        .map((s) => s.charAt(0).toUpperCase() + s.slice(1).replace(/-/g, " "))
        .join(" › ") || "Home";
      const title = document.createElement("span");
      title.className = "bx-result__title";
      title.textContent = r.meta.title;
      const ex = document.createElement("span");
      ex.className = "bx-result__excerpt";
      // Pagefind's excerpt is the page's own text, HTML-escaped, with <mark> around hits.
      ex.innerHTML = r.excerpt;
      a.append(path, title, ex);
      list.append(a);
    }
    pick(0);
  };
  input.addEventListener("input", async () => {
    const q = input.value.trim();
    const mine = ++seq;
    if (!q) return empty("Type to search the documentation");
    try {
      const pf = await load();
      const search = await pf.debouncedSearch(q, {}, 120);
      if (!search || mine !== seq) return;
      const data = await Promise.all(search.results.slice(0, 8).map((r) => r.data()));
      if (mine === seq) render(data);
    } catch {
      empty("Search is not available on this copy of the site");
    }
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Escape") (e.preventDefault(), dialog.close());
    else if (e.key === "ArrowDown") (e.preventDefault(), pick(active + 1));
    else if (e.key === "ArrowUp") (e.preventDefault(), pick(active - 1));
    else if (e.key === "Enter") {
      const el = list.querySelectorAll(".bx-result")[active];
      if (el) (e.preventDefault(), (location.href = el.href));
    }
  });
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) dialog.close();
  });
  for (const b of document.querySelectorAll("[data-search-open]")) b.addEventListener("click", open);
  document.addEventListener("keydown", (e) => {
    const typing = e.target.closest && e.target.closest("input, textarea, select, [contenteditable]");
    if ((e.key === "k" || e.key === "K") && (apple ? e.metaKey : e.ctrlKey)) {
      e.preventDefault();
      dialog.open ? dialog.close() : open();
    } else if (e.key === "/" && !typing && !dialog.open) {
      e.preventDefault();
      open();
    }
  });
})();
