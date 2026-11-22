# OpenLayers browser distribution

Version 10.10.0; BSD-2-Clause. See OpenLayers-LICENSE.md.
Unmodified distribution files from the npm ol package via jsDelivr:
- https://cdn.jsdelivr.net/npm/ol@10.10.0/dist/ol.js
- https://cdn.jsdelivr.net/npm/ol@10.10.0/ol.css
- https://cdn.jsdelivr.net/npm/ol@10.10.0/LICENSE.md

The map loads local JavaScript only when opened. OSM raster tiles remain external
requests, with OpenLayers attribution controls and normal browser caching.

# SQLite / sql.js

Version 1.17.0, MIT; see sql-js-LICENSE. SQLite is public domain. Unmodified npm
browser artifacts:
- https://cdn.jsdelivr.net/npm/sql.js@1.17.0/dist/sql-wasm.js
- https://cdn.jsdelivr.net/npm/sql.js@1.17.0/dist/sql-wasm.wasm
- https://cdn.jsdelivr.net/npm/sql.js@1.17.0/LICENSE

Used by the contact log worker, with database images committed in IndexedDB.
No decoder uses SQLite and no native build step is added.
