// Resolving, opening, and caching the `.rete` graphs the server can reach.
//
// Every file is opened LAZILY — a published URL over HTTP Range, a local
// file over the client's `file://` reader — so nothing is ever loaded whole.
// A 20 GB graph on the user's disk costs the same handful of range reads as a
// 20 MB one, which is what makes a desktop extension over this format viable
// at all. The exception is RDF shared in the chat (load_rdf): it arrives as
// text in a tool call, so its graph is built and held in memory.
import { readFile, readdir, realpath, stat } from "node:fs/promises";
import { extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

import { build, open } from "rete-graph";

const RETE_EXT = ".rete";
const SCAN_DEPTH = 4;
const SCAN_TTL_MS = 5_000;
const SKIP_DIRS = new Set(["node_modules", ".git", ".cache", "__pycache__"]);

const CHAT = "chat:";
const CHAT_MAX_BYTES = 32 * 1024 * 1024; // RDF text one chat graph may hold
// RDF syntax by file extension, for a chat graph named after its file.
const CHAT_FORMATS = {
  ".ttl": "ttl",
  ".turtle": "ttl",
  ".nt": "nt",
  ".ntriples": "nt",
  ".nq": "nq",
  ".nquads": "nq",
  ".trig": "trig",
};

const WHERE = {
  published: "remote (HTTP Range)",
  local: "local file (lazy byte-range reads)",
  chat: "in memory (RDF shared in the chat)",
};

/** A resolved dataset: where its bytes live and what to call it. */
export class Source {
  constructor({ kind, key, url, path, entry, bytes, generation }) {
    this.kind = kind; // "published" | "local" | "chat"
    this.key = key;
    this.url = url;
    this.path = path;
    this.entry = entry ?? null; // catalog record, for published datasets
    this.bytes = bytes ?? null; // the built file image, for a chat graph
    this.generation = generation ?? 0; // times a chat graph was (re)loaded
  }

  /**
   * What `open()` takes — an http(s) URL, a file:// URL for local files, or
   * a chat graph's bytes (a view on shared memory, so handing it to a query
   * worker does not copy it).
   */
  get openTarget() {
    if (this.kind === "chat") return this.bytes;
    return this.kind === "local" ? pathToFileURL(this.path).href : this.url;
  }

  /** Names the open graph in a cache: changes whenever a chat graph is reloaded. */
  get cacheKey() {
    return this.kind === "chat" ? `${this.key}#${this.generation}` : this.openTarget;
  }

  /** How the bytes are read, as the tools' `stats.where` says it. */
  get where() {
    return WHERE[this.kind];
  }
}

export class GraphStore {
  #cache = new Map(); // cacheKey -> Graph (insertion-ordered = LRU eviction)
  #scan = { at: 0, files: [] };
  #generation = new Map(); // local path -> times build_rete rewrote it
  #chat = new Map(); // "chat:<name>" -> { source, text, size, format, parts, loadedAt }

  /**
   * @param {object} opts
   * @param {string[]} opts.allowedDirs directories the user granted access to
   * @param {{datasets: object[]}} opts.catalog the published-dataset snapshot
   * @param {number} [opts.maxOpen] resident graphs (each keeps a block cache)
   */
  constructor({ allowedDirs, catalog, maxOpen = 6 }) {
    this.allowedDirs = allowedDirs;
    this.catalog = catalog?.datasets ?? [];
    this.maxOpen = maxOpen;
  }

  // --- local files -------------------------------------------------------

  /**
   * Every `.rete` under the granted directories. Re-scanned at most every few
   * seconds so a file the user just built shows up without restarting the
   * extension.
   */
  async localFiles() {
    if (Date.now() - this.#scan.at < SCAN_TTL_MS) return this.#scan.files;
    const files = [];
    const seen = new Set();
    const walk = async (dir, depth) => {
      if (depth > SCAN_DEPTH) return;
      let entries;
      try {
        entries = await readdir(dir, { withFileTypes: true });
      } catch {
        return; // unreadable directory: skip, never fail a listing over it
      }
      for (const e of entries) {
        if (e.name.startsWith(".") || SKIP_DIRS.has(e.name)) continue;
        const full = join(dir, e.name);
        if (e.isDirectory()) await walk(full, depth + 1);
        else if (e.isFile() && e.name.toLowerCase().endsWith(RETE_EXT)) {
          if (seen.has(full)) continue;
          seen.add(full);
          const info = await stat(full).catch(() => null);
          files.push({
            key: e.name.slice(0, -RETE_EXT.length),
            path: full,
            size: info?.size ?? 0,
            modified: info?.mtime?.toISOString() ?? null,
          });
        }
      }
    };
    for (const dir of this.allowedDirs) await walk(dir, 0);
    files.sort((a, b) => a.key.localeCompare(b.key));
    this.#scan = { at: Date.now(), files };
    return files;
  }

  /**
   * Is `path` inside a directory the user granted? Compares REAL paths, so a
   * symlink pointing out of the sandbox does not escape it.
   */
  async #isAllowed(path) {
    const real = await realpath(path).catch(() => resolve(path));
    for (const dir of this.allowedDirs) {
      const realDir = await realpath(dir).catch(() => resolve(dir));
      const rel = relative(realDir, real);
      if (rel === "" || (!rel.startsWith("..") && !isAbsolute(rel))) return true;
    }
    return false;
  }

  // --- RDF shared in the chat ----------------------------------------------

  /**
   * Build RDF text into an in-memory graph named `chat:<name>`. Claude Desktop
   * does not hand chat attachments to extensions, so the text arrives in the
   * tool call — in parts when it is too long for one: `append` adds a part to
   * the text already held and rebuilds the whole graph, so prefixes declared
   * in the first part carry over. A part that does not parse is not added.
   */
  async loadChat(name, text, { format, append = false, quotedTripleSyntax = "rdf-star" } = {}) {
    const key = chatKey(name);
    const previous = this.#chat.get(key);
    const base = append ? previous : undefined;
    format ??= base?.format ?? formatOf(name) ?? "ttl";
    if (base && format !== base.format) {
      throw new UsageError(`${key} holds ${base.format}; a part appended to it must be ${base.format} too, not ${format}`);
    }
    const all = base ? `${base.text}\n${text}` : text;
    const size = Buffer.byteLength(all);
    if (size > CHAT_MAX_BYTES) {
      throw new UsageError(
        `${key} would hold ${size} bytes of RDF text — more than the ${CHAT_MAX_BYTES} a graph shared ` +
          "in the chat may. Build it with the `rete build` CLI into a granted folder instead.",
      );
    }
    let built;
    try {
      built = await build(all, format, { quotedTripleSyntax });
    } catch (error) {
      if (!base) throw error;
      const [first, ...rest] = String(error?.message ?? error).split("\n");
      throw new Error(
        `${first} — this part was NOT added: ${key} still holds its ${base.parts} earlier ` +
          `part${base.parts === 1 ? "" : "s"}. Fix the part and send it again; split parts where a ` +
          `statement ends.${rest.length ? `\n${rest.join("\n")}` : ""}`,
      );
    }
    // Shared memory: every query worker that opens it reads the same bytes.
    const bytes = new Uint8Array(new SharedArrayBuffer(built.length));
    bytes.set(built);
    if (previous) this.#cache.delete(previous.source.cacheKey);
    const source = new Source({ kind: "chat", key, bytes, generation: (previous?.source.generation ?? 0) + 1 });
    const entry = { source, text: all, size, format, parts: (base?.parts ?? 0) + 1, loadedAt: new Date().toISOString() };
    this.#chat.set(key, entry);
    return entry;
  }

  /** The chat graphs held, for list_datasets. */
  chatGraphs() {
    return [...this.#chat.values()];
  }

  /**
   * The text of an RDF file under a granted folder — for one too large to
   * send through the chat, which the user saved there instead. A bare name is
   * looked for in each granted folder.
   */
  async readGrantedText(path) {
    if (this.allowedDirs.length === 0) {
      throw new UsageError("no folder granted — add one in the extension's settings (Graph folders), or pass the RDF as `rdf`");
    }
    const candidates = isAbsolute(path) ? [resolve(path)] : this.allowedDirs.map((dir) => resolve(dir, path));
    for (const full of candidates) {
      if (!(await this.#isAllowed(full))) {
        throw new UsageError(`${full} is outside the directories this extension may read`);
      }
      const info = await stat(full).catch(() => null);
      if (!info?.isFile()) continue;
      if (info.size > CHAT_MAX_BYTES) {
        throw new UsageError(
          `${full} is ${info.size} bytes — more than the ${CHAT_MAX_BYTES} of RDF text a graph built ` +
            "here may hold. Build it with the `rete build` CLI into a granted folder instead.",
        );
      }
      return readFile(full, "utf8");
    }
    throw new UsageError(`no such file in the granted folders: ${path}`);
  }

  // --- resolution --------------------------------------------------------

  /**
   * Turn a `dataset` argument into a Source. Accepts, in order: an http(s)
   * URL to any published `.rete`; `chat:<name>`, RDF loaded from the chat;
   * the name of a local file under a granted directory (with or without the
   * extension, or a full path); a chat graph's bare name; a key from the
   * published catalog. Local files win a name clash — the user's own data is
   * the more specific answer.
   */
  async resolve(dataset) {
    const name = String(dataset ?? "").trim();
    if (!name) throw new UsageError("no dataset given — call list_datasets to see what is available");

    if (/^https?:\/\//i.test(name)) {
      return new Source({ kind: "published", key: name, url: name });
    }

    if (name.toLowerCase().startsWith(CHAT)) {
      const held = this.#chat.get(chatKey(name));
      if (held) return held.source;
      throw new UsageError(
        `no ${chatKey(name)} here. RDF shared in the chat is held only until the extension ` +
          "restarts: load it with load_rdf (again).",
      );
    }

    if (name.startsWith("file://")) {
      throw new UsageError("pass a plain path or a file name, not a file:// URL");
    }

    const files = await this.localFiles();
    const bare = name.toLowerCase().endsWith(RETE_EXT) ? name.slice(0, -RETE_EXT.length) : name;
    const byKey = files.find((f) => f.key === bare || f.key === name);
    if (byKey) return new Source({ kind: "local", key: byKey.key, path: byKey.path });

    const plain = !name.includes("/") && !name.includes(sep);
    const chat = plain && this.#chat.size ? this.#chat.get(chatKey(name, false)) : undefined;
    if (chat) return chat.source;

    if (name.includes(sep) || name.includes("/") || isAbsolute(name)) {
      const path = resolve(name);
      if (!(await this.#isAllowed(path))) {
        throw new UsageError(
          `${path} is outside the directories this extension may read. Grant it in ` +
            "the extension's settings (Graph folders), or pass a published dataset key or URL.",
        );
      }
      const info = await stat(path).catch(() => null);
      if (!info?.isFile()) throw new UsageError(`no such file: ${path}`);
      return new Source({ kind: "local", key: name, path });
    }

    const entry = this.catalog.find((d) => d.key === name);
    if (entry) {
      if (!entry.url && entry.shards?.length) {
        throw new UsageError(
          `${entry.key} is published as ${entry.shards.length} shards, so it has no single ` +
            `file. Query one shard by URL, or federate across them:\n${entry.shards.join("\n")}`,
        );
      }
      return new Source({ kind: "published", key: entry.key, url: entry.url, entry });
    }

    const near = [...files.map((f) => f.key), ...this.#chat.keys(), ...this.catalog.map((d) => d.key)]
      .filter((k) => k.includes(bare) || bare.includes(k))
      .slice(0, 5);
    throw new UsageError(
      `unknown dataset "${name}". ` +
        (near.length ? `Did you mean: ${near.join(", ")}? ` : "") +
        "Call list_datasets for the published catalog and the local files.",
    );
  }

  /**
   * A `.rete` under a granted folder was just (re)written — by `build_rete`.
   * Rescan on the next lookup, so the name that tool hands back resolves at
   * once rather than after the scan TTL, and drop any graph opened from the
   * file's previous bytes.
   */
  changed(path) {
    this.#scan.at = 0;
    this.#cache.delete(new Source({ kind: "local", key: "", path: resolve(path) }).openTarget);
    this.#generation.set(resolve(path), (this.#generation.get(resolve(path)) ?? 0) + 1);
  }

  /**
   * A token that changes whenever a local file's bytes may have: bumped by
   * changed() (build_rete), and derived from size and mtime for any other
   * writer. The query workers key their open graphs by it, so they never
   * answer from a file's previous bytes. A published URL names immutable
   * bytes, and a chat graph's cacheKey already changes with its bytes: "".
   */
  async stamp(source) {
    if (source.kind !== "local") return "";
    const path = resolve(source.path);
    const info = await stat(path).catch(() => null);
    return `${this.#generation.get(path) ?? 0}:${info?.size}:${info?.mtimeMs}`;
  }

  /** The open (and cached) graph for a dataset argument, plus its Source. */
  async graph(dataset) {
    const source = await this.resolve(dataset);
    const key = source.cacheKey;
    let graph = this.#cache.get(key);
    if (graph) {
      // refresh LRU position
      this.#cache.delete(key);
      this.#cache.set(key, graph);
      return { graph, source };
    }
    graph = await open(source.openTarget);
    this.#cache.set(key, graph);
    while (this.#cache.size > this.maxOpen) {
      const oldest = this.#cache.keys().next().value;
      this.#cache.delete(oldest);
    }
    return { graph, source };
  }
}

/** A user-facing problem (bad dataset name, path outside the sandbox, …). */
export class UsageError extends Error {}

/** The RDF syntax a file name's extension says ("people.nt" → "nt"), or undefined. */
export const formatOf = (name) => CHAT_FORMATS[extname(String(name ?? "")).toLowerCase()];

/**
 * "people.ttl", "chat:people" → "chat:people": a file's name, minus its
 * extension. A name with nothing usable in it throws, or, unless `strict`,
 * gives null.
 */
function chatKey(name, strict = true) {
  let stem = String(name ?? "").trim();
  if (stem.toLowerCase().startsWith(CHAT)) stem = stem.slice(CHAT.length);
  stem = stem.split(/[\\/]/).pop();
  const ext = extname(stem).toLowerCase();
  if (CHAT_FORMATS[ext] || ext === RETE_EXT || ext === ".txt") stem = stem.slice(0, -ext.length);
  stem = stem.replace(/[^\w.-]+/g, "-").replace(/^[-.]+|[-.]+$/g, "");
  if (stem) return `${CHAT}${stem}`;
  if (!strict) return null;
  throw new UsageError(`"${name}" is not a usable name — call it after the file, e.g. "people.ttl"`);
}
