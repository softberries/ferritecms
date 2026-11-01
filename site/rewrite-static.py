"""Rewrites a mirrored FerriteCMS site for static hosting (site/export-static.sh).

    python3 -I site/rewrite-static.py DIR ORIGIN BASE_URL

- Files saved with a query string in their name (`style.css?v=…`) lose it: a static host
  ignores the query and serves the plain path.
- ORIGIN (the local server) becomes BASE_URL in HTML, CSS, XML and text files.
- When BASE_URL has a path (`/ferritecms`), root-relative links in HTML and CSS
  (`href="/…"`, `src`, `action`, `srcset`, `url(/…)`) get it in front.
- `&#x2f;` in HTML becomes `/` (the renderer escapes it in attributes, which hides links
  from the rewrite).
- The search form is removed: searching needs the server.
"""

import os
import re
import sys
from urllib.parse import urlsplit

TEXT = (".html", ".css", ".xml", ".txt", ".js", ".json", ".svg")
SEARCH_FORM = re.compile(r'<form class="search-form[^"]*".*?</form>\s*', re.S)
ATTR = re.compile(r'\b(href|src|action|poster)="/(?!/)')
SRCSET = re.compile(r'\bsrcset="([^"]*)"')
CSS_URL = re.compile(r"url\((['\"]?)/(?!/)")


def prefix_srcset(match: re.Match, path: str) -> str:
    parts = []
    for item in match.group(1).split(","):
        item = item.strip()
        if item.startswith("/") and not item.startswith("//"):
            item = path + item
        parts.append(item)
    return 'srcset="' + ", ".join(parts) + '"'


def rewrite(text: str, name: str, origin: str, base: str, path: str) -> str:
    if name.endswith(".html"):
        # The renderer writes `/` in attributes as `&#x2f;`; HTML never needs it escaped.
        text = text.replace("&#x2f;", "/").replace("&#X2F;", "/")
        text = SEARCH_FORM.sub("", text)
    text = text.replace(origin + "/", base + "/").replace(origin, base)
    if path and name.endswith((".html", ".css")):
        text = ATTR.sub(lambda m: f'{m.group(1)}="{path}/', text)
        text = SRCSET.sub(lambda m: prefix_srcset(m, path), text)
        text = CSS_URL.sub(lambda m: f"url({m.group(1)}{path}/", text)
    return text


def main() -> None:
    out, origin, base = sys.argv[1], sys.argv[2].rstrip("/"), sys.argv[3].rstrip("/")
    path = urlsplit(base).path.rstrip("/")
    for dirpath, _, files in os.walk(out):
        for name in files:
            if "?" in name:
                plain = name.split("?", 1)[0]
                if os.path.exists(os.path.join(dirpath, plain)):
                    os.remove(os.path.join(dirpath, name))
                    continue
                os.rename(os.path.join(dirpath, name), os.path.join(dirpath, plain))
                name = plain
            if not name.endswith(TEXT):
                continue
            file = os.path.join(dirpath, name)
            with open(file, encoding="utf-8") as f:
                text = f.read()
            new = rewrite(text, name, origin, base, path)
            if new != text:
                with open(file, "w", encoding="utf-8") as f:
                    f.write(new)


if __name__ == "__main__":
    main()
