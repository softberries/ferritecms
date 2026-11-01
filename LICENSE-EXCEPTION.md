# Additional permission for plugins and themes

FerriteCMS is licensed under the GNU Affero General Public License, version 3 only
(`AGPL-3.0-only`; the text is in [`LICENSE`](LICENSE)). Its copyright holder, Krzysztof
Grajek, grants the following additional permission under section 7 of that license.

1. A plugin (a WebAssembly component that FerriteCMS loads) or a theme (the templates,
   styles and assets that FerriteCMS renders) is not a work based on FerriteCMS merely
   because it uses the plugin interface (the WIT files in `wit/`) or the theme interface
   (the data and helpers FerriteCMS gives templates), or because FerriteCMS loads, runs or
   renders it.
2. You may copy and modify the following into such a plugin or theme and license the result
   under terms of your choice: the plugin SDK (`crates/ferrite-sdk`), the WIT files (`wit/`)
   and code generated from them, the example plugins (`plugins/examples/`) and the starter
   themes (`themes/`).

This permission covers plugins and themes only. FerriteCMS itself, and any modified version
of it, stays under `AGPL-3.0-only`. Whoever conveys a modified version of FerriteCMS may
remove this permission from it, as section 7 allows.
