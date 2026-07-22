// Resolving, opening, and caching the `.rete` graphs the server can reach.
//
// Every source is opened LAZILY — a published URL over HTTP Range, a local
// file over the client's `file://` reader — so nothing is ever loaded whole.
// A 20 GB graph on the user's disk costs the same handful of range reads as a
// 20 MB one, which is what makes a desktop extension over this format viable
// at all.
import { readdir, realpath, stat } from "node:fs/promises";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

import { open } from "rete-graph";

const RETE_EXT = ".rete";
const SCAN_DEPTH = 4;
const SCAN_TTL_MS = 5_000;
const SKIP_DIRS = new Set(["node_modules", ".git", ".cache", "__pycache__"]);

/** A resolved dataset: where its bytes live and what to call it. */
export class Source {
  constructor({ kind, key, url, path, entry }) {
    this.kind = kind; // "published" | "local"
    this.key = key;
    this.url = url;
    this.path = path;
    this.entry = entry ?? null; // catalog record, for published datasets
  }

  /** What `open()` takes — an http(s) URL, or a file:// URL for local files. */
  get openTarget() {
    return this.kind === "local" ? pathToFileURL(this.path).href : this.url;
  }
}

export class GraphStore {
  #cache = new Map(); // openTarget -> Graph (insertion-ordered = LRU eviction)
  #scan = { at: 0, files: [] };

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

  // --- resolution --------------------------------------------------------

  /**
   * Turn a `dataset` argument into a Source. Accepts, in order: an http(s)
   * URL to any published `.rete`; the name of a local file under a granted
   * directory (with or without the extension, or a full path); a key from the
   * published catalog. Local files win a name clash — the user's own data is
   * the more specific answer.
   */
  async resolve(dataset) {
    const name = String(dataset ?? "").trim();
    if (!name) throw new UsageError("no dataset given — call list_datasets to see what is available");

    if (/^https?:\/\//i.test(name)) {
      return new Source({ kind: "published", key: name, url: name });
    }

    if (name.startsWith("file://")) {
      throw new UsageError("pass a plain path or a file name, not a file:// URL");
    }

    const files = await this.localFiles();
    const bare = name.toLowerCase().endsWith(RETE_EXT) ? name.slice(0, -RETE_EXT.length) : name;
    const byKey = files.find((f) => f.key === bare || f.key === name);
    if (byKey) return new Source({ kind: "local", key: byKey.key, path: byKey.path });

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

    const near = [...files.map((f) => f.key), ...this.catalog.map((d) => d.key)]
      .filter((k) => k.includes(bare) || bare.includes(k))
      .slice(0, 5);
    throw new UsageError(
      `unknown dataset "${name}". ` +
        (near.length ? `Did you mean: ${near.join(", ")}? ` : "") +
        "Call list_datasets for the published catalog and the local files.",
    );
  }

  /** The open (and cached) graph for a dataset argument, plus its Source. */
  async graph(dataset) {
    const source = await this.resolve(dataset);
    const target = source.openTarget;
    let graph = this.#cache.get(target);
    if (graph) {
      // refresh LRU position
      this.#cache.delete(target);
      this.#cache.set(target, graph);
      return { graph, source };
    }
    graph = await open(target);
    this.#cache.set(target, graph);
    while (this.#cache.size > this.maxOpen) {
      const oldest = this.#cache.keys().next().value;
      this.#cache.delete(oldest);
    }
    return { graph, source };
  }
}

/** A user-facing problem (bad dataset name, path outside the sandbox, …). */
export class UsageError extends Error {}
