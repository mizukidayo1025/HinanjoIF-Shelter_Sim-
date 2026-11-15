// The dedicated Worker that answers the screen's API in the browser (Issue #266): js/api_fetch.js
// starts it and hands it each `/api/v1/...` request; js/browser_api.mjs answers it from the origin
// private file system (OPFS) and WASM. A dedicated Worker, because OPFS's synchronous access
// handles, which a stored Run is read through, are a dedicated Worker's.
//
// Where things are (relative to this file): the WASM (`wasm-bindgen --target web`) in pkg/, the
// site's list of examples at `examplesUrl` (the page's `data-examples` on the script, see
// api_fetch.js; /sample/examples.json by default) and each example's manifest beside it. The store
// is OPFS's folder projects/; the marker of the examples it holds (browser_api.mjs) is beside it.
// Every request, each chunk of a computation and an example's install hold the Web Lock
// "sheltersim-browser-store", so two tabs never open the same files at once. Opening the page
// fetches no example: one is fetched when the home's list is chosen from (`install`, #411). An
// example the store holds of an earlier version of the site is removed at the start, as the site
// no longer offers it. An example whose Runs were all deleted on the home (#440) is offered again
// and fetched again over its Project (browser_api.mjs's exampleHeld). A computation a closed page left becomes `interrupted` (js/compute.mjs,
// #267); a branch that computes holds the Web Lock of its name (`lockName`) until its computation
// ends.
//
// Messages in:  { type: "start", examplesUrl } once, then { type: "request", id, method, path,
//               search, body, llm? } (llm: the relay's passphrase, sent with a Scenario preview, #421), { type: "examples", id }, { type: "install", id, projectId } or
//               { type: "stats", id }
// Messages out: { type: "ready", ms } or { type: "failed", code, message } once
//               (code `browser_storage_unavailable` when the browser refuses the store, #393,
//               otherwise `browser_unavailable`; the message is what the screen shows);
//               { type: "answer", id, status, contentType, body } for each request;
//               { type: "examples", id, examples } (the list, each with `installed`);
//               { type: "progress", id, phase, bytes, total } while an example is installed, then
//               { type: "installed", id, installed, ms } or { type: "install-failed", id, message };
//               { type: "stats", id, stats } for { type: "stats", id } (checks).

import { BrowserApi, checkExamples, exampleHeld, installSample, markerExamples, openStore, SAMPLE_MARKER } from "./browser_api.mjs";
import { recoverInterrupted } from "./compute.mjs";
import { opfsTree } from "./project_store.mjs";
import { RegionProviderError } from "./research.mjs";

const LOCK = "sheltersim-browser-store";
const locked = (task) => (navigator.locks ? navigator.locks.request(LOCK, task) : task());

/// Holds the Web Lock `name` when no one else does: a function that lets it go, or null.
function branchLock(name) {
  if (!navigator.locks) return Promise.resolve(() => {});
  return new Promise((resolve, reject) => {
    navigator.locks
      .request(name, { ifAvailable: true }, (lock) => {
        if (!lock) {
          resolve(null);
          return null;
        }
        return new Promise((release) => resolve(() => release()));
      })
      .catch(reject);
  });
}

async function readFile(folder, name) {
  try {
    const handle = await folder.getFileHandle(name);
    return new Uint8Array(await (await handle.getFile()).arrayBuffer());
  } catch (error) {
    if (error?.name === "NotFoundError") return null;
    throw error;
  }
}

async function writeFile(folder, name, bytes) {
  const handle = await folder.getFileHandle(name, { create: true });
  const access = await handle.createSyncAccessHandle();
  try {
    access.truncate(0);
    let at = 0;
    while (at < bytes.length) at += access.write(bytes.subarray(at), { at });
    access.flush();
  } finally {
    access.close();
  }
}

async function start(examplesUrl) {
  const started = performance.now();
  // The store first: a browser that refuses it (a private window, #393) gets its own answer.
  const root = await openStore(navigator.storage);
  const wasm = await import(new URL("./pkg/sheltersim_wasm.js", import.meta.url).href);
  await wasm.default();
  const tree = opfsTree(await root.getDirectoryHandle("projects", { create: true }));
  const listUrl = new URL(examplesUrl, self.location.href);
  const examples = await fetch(listUrl, { cache: "no-cache" })
    .then((answer) => (answer.ok ? answer.json() : null))
    .then((list) => checkExamples(list).examples)
    .catch((error) => {
      console.warn("the site's list of examples cannot be read:", error);
      return [];
    });
  const readMarker = async () => {
    const bytes = await readFile(root, SAMPLE_MARKER);
    try {
      return markerExamples(bytes === null ? null : JSON.parse(new TextDecoder().decode(bytes)));
    } catch {
      return {};
    }
  };
  const writeMarker = (held) => writeFile(root, SAMPLE_MARKER, new TextEncoder().encode(`${JSON.stringify({ examples: held })}\n`));
  await locked(async () => {
    // An example of an earlier site (another sha256, or one this site no longer lists) is the
    // site's, not the person's: it goes, and the home offers this site's to fetch.
    const held = await readMarker();
    const offered = new Map(examples.map((example) => [example.project_id, example.sha256]));
    let changed = false;
    for (const [projectId, sha256] of Object.entries(held)) {
      if (offered.get(projectId) === sha256) continue;
      await tree.remove([projectId]);
      delete held[projectId];
      changed = true;
    }
    if (changed) await writeMarker(held);
    // What a page closed in the middle of a computation left.
    try {
      await recoverInterrupted({ wasm, tree, branchLock });
    } catch (error) {
      console.warn("interrupted computations are not recovered:", error);
    }
  });
  const listed = async () => {
    const held = await readMarker();
    const out = [];
    for (const example of examples) {
      out.push({ ...example, installed: await exampleHeld(tree, example, held) });
    }
    return out;
  };
  // Fetches and imports the example of `projectId`, telling `progress` how far it is.
  const install = (projectId, progress) =>
    locked(async () => {
      const example = examples.find((item) => item.project_id === projectId);
      if (!example) throw new Error(`このサイトには例「${projectId}」がありません`);
      const manifestUrl = new URL(example.manifest, listUrl);
      const response = await fetch(manifestUrl, { cache: "no-cache" });
      if (!response.ok) throw new Error(`計算済みの例の目録を読めません（HTTP ${response.status}）`);
      const manifest = await response.json();
      if (manifest?.sha256 !== example.sha256 || manifest?.project_id !== projectId) throw new Error("計算済みの例の目録が、例の一覧と合いません。ページを開き直してください");
      const held = await readMarker();
      // An example whose Runs were deleted on the home (#440) is fetched and imported again.
      const whole = await exampleHeld(tree, example, held);
      return installSample({
        wasm,
        tree,
        manifest,
        async fetchBytes(path) {
          const part = await fetch(new URL(path, manifestUrl));
          if (!part.ok) throw new Error(`計算済みの例の ${path} を読めません（HTTP ${part.status}）`);
          return new Uint8Array(await part.arrayBuffer());
        },
        readMarker: async () => (whole ? { sha256: held[projectId], project_id: projectId } : null),
        writeMarker: () => writeMarker({ ...held, [projectId]: manifest.sha256 }),
        progress,
      });
    });
  // The site's index of the GSI shelter CSV (scripts/build_static_site.py), read once; null when
  // the site has none (the Scenario preview then answers browser_unsupported). A municipality
  // code's rows are read when a lookup needs them (#332), and kept once read; one that cannot be
  // read is "unavailable" in the research, as Python's CSV download that fails.
  let shelters = null;
  const shelterIndex = () =>
    (shelters ??= fetch(new URL("./gsi-shelter-index.json", import.meta.url), { cache: "no-cache" })
      .then((answer) => (answer.ok ? answer.json() : null))
      .catch(() => null));
  const rowsByCode = new Map();
  const shelterRows = async (code) => {
    if (!rowsByCode.has(code)) {
      let answer;
      try {
        answer = await fetch(new URL(`./gsi-shelters/${code}.json`, import.meta.url), { cache: "no-cache" });
      } catch {
        throw new RegionProviderError("public-data endpoint could not be reached");
      }
      if (!answer.ok) throw new RegionProviderError(`public-data endpoint returned HTTP ${answer.status}`);
      try {
        rowsByCode.set(code, await answer.json());
      } catch {
        throw new RegionProviderError("public-data endpoint returned invalid JSON");
      }
    }
    return rowsByCode.get(code);
  };
  const api = new BrowserApi({
    wasm,
    tree,
    quota: async () => (await navigator.storage.estimate()).quota ?? null,
    locked,
    branchLock,
    scenario: { shelterIndex, shelterRows },
  });
  return { api, wasm, listed, install, ms: performance.now() - started };
}

let ready = null;

/// What a request is answered when the Worker could not start: the store's own answer when the
/// browser refused it (openStore), otherwise `browser_unavailable` with the reason.
function failure(error) {
  if (error?.code === "browser_storage_unavailable") return { code: error.code, message: error.message };
  return { code: "browser_unavailable", message: `ブラウザー版の準備に失敗しました：${error?.message ?? error}` };
}

self.onmessage = async ({ data }) => {
  if (data.type === "start") {
    ready = start(data.examplesUrl);
    ready.then(
      ({ ms }) => self.postMessage({ type: "ready", ms }),
      (error) => {
        if (error?.cause) console.warn("the browser's store cannot be opened:", error.cause);
        self.postMessage({ type: "failed", ...failure(error) });
      },
    );
    return;
  }
  if (data.type === "stats") {
    // What the answers cost so far, for checks: seeks, ticks stepped, their times, WASM's memory.
    const { api, wasm } = await ready;
    self.postMessage({ type: "stats", id: data.id, stats: { ...api.stats, computed: api.computing.done, memoryBytes: wasm.memoryBytes() } });
    return;
  }
  if (data.type === "examples") {
    let examples = [];
    try {
      examples = await (await ready).listed();
    } catch {
      // A Worker that could not start offers no example; its requests say why.
    }
    self.postMessage({ type: "examples", id: data.id, examples });
    return;
  }
  if (data.type === "install") {
    try {
      const { install } = await ready;
      const done = await install(data.projectId, (state) => self.postMessage({ type: "progress", id: data.id, ...state }));
      self.postMessage({ type: "installed", id: data.id, installed: done.installed, ms: done.ms });
    } catch (error) {
      self.postMessage({ type: "install-failed", id: data.id, message: `計算済みの例を取得できませんでした：${error?.message ?? error}` });
    }
    return;
  }
  if (data.type !== "request") return;
  let answer;
  try {
    const { api } = await ready;
    answer = await api.answer({ method: data.method, path: data.path, search: data.search, body: data.body, llm: typeof data.llm === "string" && data.llm ? data.llm : null });
  } catch (error) {
    const { code, message } = failure(error);
    answer = { status: 503, type: "application/json; charset=utf-8", body: JSON.stringify({ error: message, code }) };
  }
  self.postMessage({ type: "answer", id: data.id, status: answer.status, contentType: answer.type, body: answer.body });
};
