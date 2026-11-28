// Small helpers the shell modules share: escaping, IRI prefixes, units, the saved UI state.
import { app } from './app.js';

export const esc = s => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;');
export const $ = id => document.getElementById(id);

export const PREFIXES = {
  'https://w3id.org/rete/cad/fzk-haus/sys/': 's', 'https://w3id.org/rete/cad/fzk-haus/': 'fzk', 'https://w3id.org/rete/cad/sys#': 'sys', 'https://w3id.org/rete/cad#': 'cad',
  'https://w3id.org/bot#': 'bot', 'https://brickschema.org/schema/Brick#': 'brick', 'https://w3id.org/rete/geo3#': 'geo3', 'https://w3id.org/rete/geo3/function/': 'geof3',
  'http://www.opengis.net/ont/geosparql#': 'geo', 'http://www.w3.org/1999/02/22-rdf-syntax-ns#': 'rdf', 'http://www.w3.org/2000/01/rdf-schema#': 'rdfs', 'http://www.w3.org/2001/XMLSchema#': 'xsd',
  'http://www.w3.org/2002/07/owl#': 'owl', 'http://www.w3.org/ns/sosa/': 'sosa', 'http://qudt.org/schema/qudt/': 'qudt', 'http://qudt.org/vocab/unit/': 'unit', 'http://qudt.org/vocab/quantitykind/': 'qk',
  'http://www.w3.org/ns/shacl#': 'sh',
};
const PFX = Object.entries(PREFIXES).sort((a, b) => b[0].length - a[0].length);
export function shortIri(iri) {
  const s = String(iri); for (const [ns, p] of PFX) if (s.startsWith(ns) && s.length > ns.length) return p + ':' + s.slice(ns.length);
  return s;
}
// an N-Triples term -> short display text
export function shortTerm(t) {
  t = String(t ?? '');
  if (t[0] === '<') return shortIri(t.slice(1, -1));
  const m = t.match(/^"([\s\S]*)"(?:@([\w-]+)|\^\^<(.*)>)?$/);
  if (m) return m[1] + (m[2] ? ' @' + m[2] : m[3] && !/XMLSchema#(string|integer|decimal|double)$/.test(m[3]) ? ' ^' + shortIri(m[3]) : '');
  return t;
}
export const prefixLines = text => { const used = []; for (const [ns, p] of Object.entries(PREFIXES)) if (new RegExp('(^|[^\\w])' + p + ':').test(text) && !new RegExp('PREFIX\\s+' + p + ':', 'i').test(text)) used.push(`PREFIX ${p}: <${ns}>`); return used; };

// ---- the saved UI state ----
const KEY = 'reteBuilding.ui.v1';
export const DEFAULTS = { side: 'explore', sideW: 296, insp: false, inspW: 330, botOpen: false, botH: 340, botTab: 'console', botMax: false, theme: 'dark', density: 'compact', units: 'metric', inspClosed: false };
export const ui = { ...DEFAULTS };
try { Object.assign(ui, JSON.parse(localStorage.getItem(KEY) || '{}')); } catch { /* no storage */ }
let st = 0;
export function saveUi() { clearTimeout(st); st = setTimeout(() => { try { localStorage.setItem(KEY, JSON.stringify(ui)); } catch { /* private window */ } }, 150); }

// ---- units: values stay metric inside, these format for display ----
export const fmt = {
  len: m => (ui.units === 'imperial' ? (m * 3.28084).toFixed(1) + ' ft' : (+m).toFixed(2) + ' m'),
  area: m2 => (ui.units === 'imperial' ? (m2 * 10.7639).toFixed(0) + ' ft²' : (+m2).toFixed(1) + ' m²'),
  temp: c => (ui.units === 'imperial' ? (c * 9 / 5 + 32).toFixed(1) + ' °F' : (+c).toFixed(1) + ' °C'),
  num: (v, d = 2) => (typeof v === 'number' ? (Number.isInteger(v) ? String(v) : v.toFixed(d)) : String(v)),
};
export const isNarrow = () => matchMedia('(max-width:640px)').matches;
export const isMedium = () => matchMedia('(max-width:900px)').matches;
export const sleep = ms => new Promise(r => setTimeout(r, ms));
// The rectangle (client px) of the 3D view, for walk and fly: the touch zones (stick, look) are measured in it and a pointer outside it is not theirs.
// viewport.js makes canvas.getBoundingClientRect() answer with the 3D pane's rectangle in the four-view layout (the whole canvas in the single one),
// so this is that call; it is one place to change if that ever stops being true.
export const rect3d = canvas => canvas.getBoundingClientRect();
// a feature of the other agent's rendering API, or null: has('viewport.setLayout')
export function api(path) {
  let o = app; for (const k of path.split('.')) { if (o == null) return null; o = o[k]; }
  return typeof o === 'function' ? o : (o === undefined ? null : o);
}
export const has = path => { let o = app; const ks = path.split('.'); for (const k of ks) { if (o == null) return false; o = o[k]; } return typeof o === 'function'; };
