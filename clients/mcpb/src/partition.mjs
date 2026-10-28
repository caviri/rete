// Partitioned queries: one query run as many short ones, split by the values
// of one variable and checkpointed to disk chunk by chunk, so an extraction
// that takes many minutes survives a cancel, a crash or a restart of Claude
// Desktop — sending the same call again carries on from the last finished
// chunk.
//
// The engine plans the split (Graph.partitionPlan): it refuses one that would
// change the answer, and writes the keys query — the variable's values, read
// off the one cheapest pattern that binds it. Each chunk is the query with its
// keys seeded as VALUES, which the engine turns into an index lookup per key.
// Chunks are sized from how long the previous ones took, two run at once
// (leaving the third worker to other queries), and their rows are merged in
// key order once all are in.
import { createHash, randomBytes } from "node:crypto";
import { mkdir, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";

const TARGET_MS = 15_000; // how long a chunk should take
const FIRST_CHUNK = 25; // keys in the first chunk, before any timing exists
const MAX_CHUNK = 5_000;
const PARALLEL = 2;
const MAX_KEYS = 1_000_000;
const RETRIES = 2; // a chunk whose reads failed (a network blip) is sent again this often
const KEEP_MS = 7 * 24 * 3600_000; // checkpoints untouched this long are deleted
const FORMAT = 1; // the checkpoint layout
// A fixed chunk size instead of the timed one, for tests.
const FIXED_CHUNK = Number(process.env.RETE_MCPB_CHUNK) || 0;

class Cancelled extends Error {
  constructor() {
    super("a step of this query was cancelled; send the same call again to resume");
  }
}

export class PartitionRuns {
  #jobs;
  #dir;
  #held = new Map(); // checkpoint id -> the job running it, or finished from it

  /**
   * @param {import("./jobs.mjs").JobManager} jobs where the steps run
   * @param {string} dir where checkpoints live, one folder per query
   */
  constructor(jobs, dir) {
    this.#jobs = jobs;
    this.#dir = dir;
    this.#sweep().catch(() => {});
  }

  /**
   * The job running `spec` split by `spec.var`: the one already running — or
   * finished and still held — for the same query on the same bytes, or else a
   * new one, which resumes from the query's checkpoint when there is one.
   */
  start(spec) {
    const id = createHash("sha256")
      .update(JSON.stringify([FORMAT, spec.key, spec.query, spec.var.replace(/^[?$]/, ""), Boolean(spec.reason)]))
      .digest("hex")
      .slice(0, 32);
    const held = this.#held.get(id);
    if (held && this.#jobs.get(held.id) === held && (held.state === "running" || held.state === "done")) {
      return held;
    }
    const job = this.#jobs.track({
      dataset: spec.dataset,
      query: spec.query,
      reason: spec.reason,
      where: spec.where,
      partition: {
        by: `?${spec.var.replace(/^[?$]/, "")}`,
        phase: "planning",
        keys: null,
        keys_done: 0,
        chunks_done: 0,
        rows: 0,
        chunk_size: null,
        resumed_keys: 0,
        keys_from_where: null,
      },
    });
    job.run = new Run(this.#jobs, job, spec, join(this.#dir, id));
    this.#held.set(id, job);
    job.run.go();
    return job;
  }

  /** Delete checkpoints nobody has touched for a week. */
  async #sweep() {
    const now = Date.now();
    for (const name of await readdir(this.#dir).catch(() => [])) {
      const dir = join(this.#dir, name);
      const info = await stat(dir).catch(() => null);
      if (info?.isDirectory() && now - info.mtimeMs > KEEP_MS) await rm(dir, { recursive: true, force: true });
    }
  }
}

class Run {
  constructor(jobs, job, spec, dir) {
    this.jobs = jobs;
    this.job = job;
    this.spec = spec;
    this.dir = dir;
    this.info = job.partition;
    this.steps = new Set(); // running
    this.spent = { reads: 0, bytes: 0 }; // by the steps that have ended
    this.stats = { fileBytes: null, bytesRead: 0, requests: 0, sessionBytesRead: 0 };
    this.cancelled = false;
    this.saving = Promise.resolve();
  }

  /** Reads and bytes so far, of every step. */
  progress() {
    let { reads, bytes } = this.spent;
    for (const s of this.steps) {
      const p = this.jobs.progress(s);
      reads += p.reads;
      bytes += p.bytes;
    }
    return { reads, bytes };
  }

  /** How many chunks are running now. */
  get running() {
    return [...this.steps].filter((s) => s.keys).length;
  }

  cancel() {
    if (this.cancelled) return;
    this.cancelled = true;
    for (const s of [...this.steps]) this.jobs.cancel(s);
    this.jobs.finish(this.job, "cancelled");
  }

  async go() {
    const { job, info, spec } = this;
    try {
      // Planned every time: it is a few reads, and its content hash says
      // whether a checkpoint was made on these very bytes.
      const planned = await this.step({ op: "plan", query: spec.query, var: spec.var });
      const plan = planned.plan;
      info.keys_from_where = plan.keys_from_where;
      let state = await this.load();
      if (!state || state.contentHash !== planned.contentHash || JSON.stringify(state.plan) !== JSON.stringify(plan)) {
        await rm(this.dir, { recursive: true, force: true });
        state = { format: FORMAT, contentHash: planned.contentHash, plan, keys: null, done: [], size: FIXED_CHUNK || FIRST_CHUNK };
      }
      await mkdir(this.dir, { recursive: true });

      if (!state.keys) {
        info.phase = "listing keys";
        const listed = await this.step({ query: plan.keys_query });
        const keys = listed.env.rows.map((row) => row[plan.var]).filter((k) => k !== undefined);
        if (keys.some((k) => k.startsWith("_:"))) {
          throw new Error(
            `?${plan.var} takes blank-node values, which cannot be named in a chunk — split by a ` +
              "variable whose values are IRIs or literals",
          );
        }
        if (keys.length > MAX_KEYS) {
          throw new Error(
            `?${plan.var} takes ${keys.length} values — more than the ${MAX_KEYS} a split holds. ` +
              "Narrow the query first (a more specific pattern on that variable), or split it by hand.",
          );
        }
        state.keys = keys;
        await writeAtomic(join(this.dir, "keys.json"), JSON.stringify(keys));
        await this.save(state);
      }
      info.keys = state.keys.length;
      info.keys_done = info.resumed_keys = covered(state.done);
      info.chunks_done = state.done.length;
      info.rows = state.done.reduce((n, d) => n + d[2], 0);
      info.chunk_size = state.size;

      info.phase = "chunks";
      await this.chunks(state);
      info.phase = "merging";
      job.env = await this.merge(state);
      job.ms = Date.now() - job.createdAt;
      job.stats = this.stats;
      info.phase = "done";
      this.jobs.finish(job, "done");
    } catch (error) {
      if (this.cancelled) return; // cancel() has ended the job
      // Anything else, a step cancelled on its own included, fails the job.
      for (const s of [...this.steps]) this.jobs.cancel(s);
      job.error = error?.message ?? String(error);
      this.jobs.finish(job, "failed");
    }
  }

  /** Run one step on a worker, and wait for it. */
  async step(fields) {
    if (this.cancelled) throw new Cancelled();
    const { dataset, key, target, reason } = this.spec;
    const s = this.jobs.submit({ dataset, key, target, reason, internal: true, ...fields });
    this.steps.add(s);
    await s.settled;
    this.steps.delete(s);
    this.spent.reads += s.progress?.reads ?? 0;
    this.spent.bytes += s.progress?.bytes ?? 0;
    if (s.stats) {
      this.stats.fileBytes = s.stats.fileBytes;
      this.stats.bytesRead += s.stats.bytesRead;
      this.stats.requests += s.stats.requests;
      this.stats.sessionBytesRead = Math.max(this.stats.sessionBytesRead, s.stats.sessionBytesRead);
    }
    if (this.cancelled || s.state === "cancelled") throw new Cancelled();
    if (s.state === "failed") throw new Error(s.error);
    return s;
  }

  /** Run every chunk not done yet, PARALLEL at a time. */
  async chunks(state) {
    const n = state.keys.length;
    if (n === 0) {
      // Nothing to split, but the answer still has a shape (its variables).
      if (!state.done.length) await this.chunk(state, 0, 0);
      return;
    }
    const { plan } = state;
    // A LIMIT without ORDER BY (a split never has one) asks for any rows, not
    // the first ones: once the finished chunks hold enough, no more start.
    // Not under dedupe, whose rows may still shrink.
    const enough = plan.limit !== null && !plan.dedupe ? plan.offset + plan.limit : Infinity;
    const rows = () => state.done.reduce((sum, d) => sum + d[2], 0);
    const todo = gaps(state.done, n);
    const tries = new Map();
    const running = new Map(); // settle promise -> [start, end]
    while (todo.length || running.size) {
      while (running.size < PARALLEL && todo.length && rows() < enough) {
        const [s, e] = todo.shift();
        const end = Math.min(e, s + state.size);
        if (end < e) todo.unshift([end, e]);
        const done = this.chunk(state, s, end).then(
          () => null,
          (error) => error,
        );
        running.set(done, [s, end]);
      }
      if (!running.size) break;
      const { p, error } = await Promise.race([...running.keys()].map((p) => p.then((error) => ({ p, error }))));
      const [s, e] = running.get(p);
      running.delete(p);
      if (this.cancelled || error instanceof Cancelled) throw new Cancelled();
      if (!error) continue;
      const tried = (tries.get(`${s}-${e}`) ?? 0) + 1;
      tries.set(`${s}-${e}`, tried);
      if (/range fetch failed|fetch failed|network|ECONN|ETIMEDOUT|socket/i.test(error.message) && tried <= RETRIES) {
        todo.unshift([s, e]); // a network blip: the same chunk again
      } else if (/worker stopped|memory|allocation/i.test(error.message) && e - s > 1) {
        const mid = s + Math.floor((e - s) / 2); // out of memory: halve it
        todo.unshift([s, mid], [mid, e]);
        state.size = Math.max(1, mid - s);
      } else {
        throw new Error(`the chunk of keys ${s + 1}–${e} (from ${state.keys[s]}) failed: ${error.message}`);
      }
    }
  }

  /** One chunk: run it, keep its rows on disk, and size the next ones by its pace. */
  async chunk(state, s, e) {
    const step = await this.step({ query: this.spec.query, var: state.plan.var, keys: state.keys.slice(s, e) });
    const env = step.env;
    const items = env.kind === "construct" ? env.triples : env.rows;
    await writeAtomic(
      join(this.dir, `c${s}-${e}.json`),
      JSON.stringify({ vars: env.vars ?? null, items, warnings: step.warnings }),
    );
    state.done.push([s, e, items.length]);
    if (e > s && !FIXED_CHUNK) {
      // The engine's own time, not the wait for a worker or the graph's open.
      const fit = Math.round(((e - s) * TARGET_MS) / Math.max(1, step.ms ?? TARGET_MS));
      state.size = Math.min(MAX_CHUNK, Math.max(1, Math.round((state.size + fit) / 2)));
    }
    await this.save(state);
    this.info.keys_done += e - s;
    this.info.chunks_done += 1;
    this.info.rows += items.length;
    this.info.chunk_size = state.size;
  }

  /** Every chunk's rows in key order: deduplicated when the plan says, then OFFSET/LIMIT. */
  async merge(state) {
    const { plan } = state;
    const seen = new Set();
    const out = [];
    const warnings = new Map();
    let vars = null;
    for (const [s, e] of [...state.done].sort((a, b) => a[0] - b[0])) {
      const part = JSON.parse(await readFile(join(this.dir, `c${s}-${e}.json`), "utf8"));
      vars ??= part.vars;
      for (const item of part.items) {
        if (plan.dedupe) {
          const k = JSON.stringify(item);
          if (seen.has(k)) continue;
          seen.add(k);
        }
        out.push(item);
      }
      for (const w of part.warnings ?? []) {
        const k = `${w.severity}|${w.function}|${w.argument}|${w.argKind}`;
        const had = warnings.get(k);
        if (had) had.count = (had.count ?? 0) + (w.count ?? 0);
        else warnings.set(k, { ...w });
      }
    }
    // A chunk with no rows may hint at a case mismatch; the whole answer may not be empty.
    this.job.warnings = [...warnings.values()].filter((w) => w.severity !== "hint" || out.length === 0);
    const items = out.slice(plan.offset, plan.limit === null ? undefined : plan.offset + plan.limit);
    return plan.form === "construct" ? { kind: "construct", triples: items } : { kind: "select", vars: vars ?? [], rows: items };
  }

  async load() {
    try {
      const state = JSON.parse(await readFile(join(this.dir, "state.json"), "utf8"));
      if (state.format !== FORMAT) return null;
      state.keys = state.hasKeys ? JSON.parse(await readFile(join(this.dir, "keys.json"), "utf8")) : null;
      // A chunk whose rows went missing runs again.
      const present = new Set(await readdir(this.dir));
      state.done = state.done.filter(([s, e]) => present.has(`c${s}-${e}.json`));
      return state;
    } catch {
      return null;
    }
  }

  /** Write the state — one write at a time, so the last one written is the newest. */
  save(state) {
    const text = JSON.stringify({ ...state, keys: undefined, hasKeys: state.keys !== null });
    this.saving = this.saving.then(() => writeAtomic(join(this.dir, "state.json"), text));
    return this.saving;
  }
}

/** Write a file whole or not at all: aside, then renamed over the old one. */
async function writeAtomic(path, text) {
  const part = `${path}.${randomBytes(4).toString("hex")}.part`;
  await writeFile(part, text);
  await rename(part, path);
}

/** Keys covered by the finished chunks. */
const covered = (done) => done.reduce((n, [s, e]) => n + (e - s), 0);

/** The ranges of [0, n) no finished chunk covers. */
function gaps(done, n) {
  const out = [];
  let at = 0;
  for (const [s, e] of [...done].sort((a, b) => a[0] - b[0])) {
    if (s > at) out.push([at, s]);
    at = Math.max(at, e);
  }
  if (at < n) out.push([at, n]);
  return out;
}
