// The map libraries, bundled by build.mjs into lib/mapkit/mapkit.js (one IIFE, ASCII) and loaded only when a map is
// first opened: Leaflet draws the map, protomaps-leaflet draws vector tiles on canvas, pmtiles reads a PMTiles
// archive by HTTP Range (or, for tiles inside a .rete, through the engine). No WebGL, no fonts, no sprites.
import L from "leaflet";
import * as P from "protomaps-leaflet";
import * as pmtiles from "pmtiles";
import css from "leaflet/dist/leaflet.css";

// protomaps-leaflet and pmtiles' raster layer extend the global L
globalThis.L = L;
globalThis.RetelensMapKit = { L, P, pmtiles, css, version: { leaflet: L.version } };
