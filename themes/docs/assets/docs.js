// FerriteCMS docs theme: progressive enhancement only (M4-3b P59). Every page works
// without it. Adds a "link to this section" anchor to each heading with an id and a
// Copy button to code blocks. No network requests.
(() => {
  "use strict";
  const pages = document.querySelector(".docs-sidebar__pages");
  if (pages && !CSS.supports("selector(::details-content)")) {
    // Older browsers cannot show a closed <details> as the wide-screen sidebar.
    const wide = matchMedia("(min-width: 60rem)");
    const sync = () => { if (wide.matches) pages.open = true; };
    sync();
    wide.addEventListener("change", sync);
  }
  const body = document.querySelector(".prose");
  if (!body) return;
  for (const h of body.querySelectorAll("h1[id],h2[id],h3[id],h4[id],h5[id],h6[id]")) {
    const a = document.createElement("a");
    a.className = "heading-anchor";
    a.href = "#" + h.id;
    a.setAttribute("aria-label", "Link to this section");
    a.textContent = "#";
    h.append(" ", a);
  }
  if (!navigator.clipboard || !window.isSecureContext) return;
  const status = document.createElement("p");
  status.className = "visually-hidden";
  status.setAttribute("role", "status");
  document.body.append(status);
  for (const pre of body.querySelectorAll("pre")) {
    let timer;
    const wrap = document.createElement("div");
    wrap.className = "code-block";
    pre.before(wrap);
    wrap.append(pre);
    const b = document.createElement("button");
    b.type = "button";
    b.className = "copy-button";
    b.textContent = "Copy";
    b.addEventListener("click", async () => {
      let ok = true;
      try {
        await navigator.clipboard.writeText(pre.textContent);
      } catch {
        ok = false;
      }
      b.textContent = ok ? "Copied" : "Not copied";
      status.textContent = ok ? "Code copied to the clipboard" : "The code could not be copied";
      clearTimeout(timer);
      timer = setTimeout(() => {
        b.textContent = "Copy";
        status.textContent = "";
      }, 2000);
    });
    wrap.append(b);
  }
})();
