// Query jobs. Every SPARQL query runs on a worker thread (worker.mjs) as a
// job, so the MCP server's own thread never runs engine code and keeps
// answering while queries read: a status call, a quick lookup, a cancel.
//
// Why: a range read is a synchronous wait, so a query run on the server's
// thread froze the whole server until it finished. A client that gave up after
// 60 s could not even cancel it, and every later call queued behind the
// abandoned query.
//
// A job is waited on for a bounded time and otherwise keeps running. Its
// result stays here — rows as the engine's term tokens — to be paged through
// or saved, until KEEP_FINISHED newer jobs have finished.
import { Worker } from "node:worker_threads";

const MAX_WORKERS = 3; // jobs running at once; the rest wait their turn
const IDLE_MS = 10 * 60_000; // an idle worker, and the caches it holds, is let go after this
const KEEP_FINISHED = 50;

const TERMINAL = new Set(["done", "failed", "cancelled"]);

export class JobManager {
  #url;
  #slots = []; // { worker, counters, job, lastKey, idle }
  #queue = [];
  #jobs = new Map(); // id -> job, in submission order
  #seq = 0;

  /** @param {URL} workerUrl the worker entry (worker.mjs beside the server) */
  constructor(workerUrl, { maxWorkers = MAX_WORKERS } = {}) {
    this.#url = workerUrl;
    this.maxWorkers = maxWorkers;
  }

  /**
   * Queue a query. `key` names the graph for the workers' caches (it changes
   * when a local file's bytes may have); `target` is what open() takes.
   */
  submit({ dataset, key, target, query, reason }) {
    let settle;
    const settled = new Promise((resolve) => (settle = resolve));
    const job = {
      id: `q${++this.#seq}`,
      dataset,
      key,
      target,
      query,
      reason,
      state: "queued",
      createdAt: Date.now(),
      startedAt: null,
      endedAt: null,
      env: null, // the engine's result envelope, once done
      warnings: [],
      ms: null,
      stats: null,
      error: null,
      progress: null, // final {reads, bytes}, once the job has ended
      slot: null,
      base: null,
      settle,
      settled,
    };
    this.#jobs.set(job.id, job);
    this.#queue.push(job);
    this.#pump();
    return job;
  }

  get(id) {
    return this.#jobs.get(String(id ?? "").trim());
  }

  /** Every job still held, newest first. */
  list() {
    return [...this.#jobs.values()].reverse();
  }

  /** Reads and bytes this job has fetched so far. */
  progress(job) {
    if (job.progress) return job.progress;
    if (!job.slot) return { reads: 0, bytes: 0 };
    const [reads, bytes] = this.#read(job.slot);
    return { reads: reads - job.base[0], bytes: bytes - job.base[1] };
  }

  /**
   * Resolve once the job has ended, `ms` have passed, or `signal` aborts —
   * whichever is first. Never cancels anything: a wait that ends early leaves
   * the job running.
   */
  async wait(job, ms, signal) {
    if (TERMINAL.has(job.state) || ms <= 0 || signal?.aborted) return job;
    let timer;
    let onAbort;
    await Promise.race([
      job.settled,
      new Promise((resolve) => {
        timer = setTimeout(resolve, ms);
      }),
      new Promise((resolve) => {
        onAbort = resolve;
        signal?.addEventListener("abort", onAbort, { once: true });
      }),
    ]);
    clearTimeout(timer);
    signal?.removeEventListener("abort", onAbort);
    return job;
  }

  /** Stop a job. A running one is stopped by terminating its worker mid-read. */
  cancel(job) {
    if (TERMINAL.has(job.state)) return job;
    if (job.state === "queued") {
      this.#queue.splice(this.#queue.indexOf(job), 1);
    } else {
      const slot = job.slot;
      job.progress = this.progress(job);
      this.#drop(slot);
      slot.worker.terminate();
    }
    this.#end(job, "cancelled");
    this.#pump();
    return job;
  }

  isTerminal(job) {
    return TERMINAL.has(job.state);
  }

  // --- scheduling ----------------------------------------------------------

  #pump() {
    while (this.#queue.length) {
      const job = this.#queue[0];
      const idle = this.#slots.filter((s) => !s.job);
      // Prefer the worker that last had this graph open: its caches are warm.
      let slot = idle.find((s) => s.lastKey === job.key) ?? idle[0];
      if (!slot) {
        if (this.#slots.length >= this.maxWorkers) return;
        slot = this.#spawn();
      }
      this.#queue.shift();
      this.#start(slot, job);
    }
  }

  #start(slot, job) {
    clearTimeout(slot.idle);
    slot.job = job;
    job.slot = slot;
    job.base = this.#read(slot);
    job.state = "running";
    job.startedAt = Date.now();
    slot.worker.postMessage({
      job: job.id,
      key: job.key,
      target: job.target,
      query: job.query,
      reason: job.reason,
    });
  }

  #spawn() {
    const shared = new SharedArrayBuffer(16);
    const worker = new Worker(this.#url, { workerData: { counters: shared } });
    worker.unref(); // the stdio transport keeps the server alive, not its workers
    const slot = { worker, counters: new BigInt64Array(shared), job: null, lastKey: null, idle: null };
    worker.on("message", (msg) => this.#finish(slot, msg));
    worker.on("error", (error) => this.#crash(slot, error));
    worker.on("exit", (code) => this.#crash(slot, new Error(`the query worker stopped (exit code ${code})`)));
    this.#slots.push(slot);
    return slot;
  }

  #finish(slot, msg) {
    // A worker terminated by cancel() can still deliver a message it had
    // already posted; the job it was running stays cancelled.
    if (!this.#slots.includes(slot)) return;
    const job = slot.job;
    if (!job || job.id !== msg.job) return;
    job.progress = this.progress(job);
    slot.job = null;
    slot.lastKey = job.key;
    if (msg.ok) {
      job.env = msg.env;
      job.warnings = msg.warnings ?? [];
      job.ms = msg.ms;
      job.stats = msg.stats;
      this.#end(job, "done");
    } else {
      job.error = msg.error;
      this.#end(job, "failed");
    }
    slot.idle = setTimeout(() => {
      if (slot.job) return;
      this.#drop(slot);
      slot.worker.terminate();
    }, IDLE_MS);
    slot.idle.unref();
    this.#pump();
  }

  /** A worker died on its own (out of memory, a crash in the engine). */
  #crash(slot, error) {
    if (!this.#slots.includes(slot)) return; // terminated on purpose
    this.#drop(slot);
    const job = slot.job;
    if (job && !TERMINAL.has(job.state)) {
      job.progress = this.progress(job);
      job.error = error?.message ?? String(error);
      this.#end(job, "failed");
    }
    this.#pump();
  }

  #drop(slot) {
    clearTimeout(slot.idle);
    const i = this.#slots.indexOf(slot);
    if (i >= 0) this.#slots.splice(i, 1);
  }

  #end(job, state) {
    job.state = state;
    job.endedAt = Date.now();
    job.progress ??= { reads: 0, bytes: 0 };
    job.slot = null;
    job.settle();
    this.#trim();
  }

  #read(slot) {
    return [Number(Atomics.load(slot.counters, 0)), Number(Atomics.load(slot.counters, 1))];
  }

  /** Forget the oldest finished jobs beyond KEEP_FINISHED (their rows go with them). */
  #trim() {
    const finished = [...this.#jobs.values()].filter((j) => TERMINAL.has(j.state));
    for (const job of finished.slice(0, Math.max(0, finished.length - KEEP_FINISHED))) {
      this.#jobs.delete(job.id);
    }
  }
}
