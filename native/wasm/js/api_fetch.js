// The browser version's API (Issue #266): loaded by the browser build's demo-v2.html before the
// screen's scripts, it wraps `window.fetch` and hands every request to this site's `/api/v1/...`
// (the screen's `api()` and its PDF `fetch`) to a dedicated Worker (js/api_worker.mjs), which
// answers it from the browser's store and WASM with the Python server's status and JSON. Other
// requests go to the real fetch. The screen's JS is unchanged.
//
// The script's attributes: `data-worker` the Worker's URL (/browser/api_worker.mjs by default),
// `data-examples` the site's list of examples (/sample/examples.json), whose Projects the screen's
// home lists (#371), and `data-scenarios="unavailable"` on a site that cannot make Scenarios
// (built without the shelter index; the screen then says so, #372). Opening the page fetches no
// example (#411): the home lists the site's examples (`examples()`) and fetches the one chosen
// (`install`).
// `window.HinanjoBrowserApi` has `ready` (a Promise of { ms }), `site` ({ scenarios }),
// `examples()` (a Promise of the list, each with `installed`), `install(projectId, progress)`
// (a Promise of { installed, ms }; `progress({ phase, bytes, total })` as it goes, phase
// "fetch", "import" or "done") and `stats()` for checks.
//
// The first time the screen starts a computation (`POST …/runs/{run}/resume`, #267) the page asks
// the browser to keep the store (`navigator.storage.persist()`, the decision on #265: once, when
// the user starts keeping Runs of their own, never on opening the page). It asks once per browser
// (a key in localStorage) and goes on whatever the answer; `persist` is the page's, not a
// Worker's.
(() => {
  "use strict";
  const script = document.currentScript;
  const llmEnabled = script?.dataset.llm !== "disabled";
  const workerUrl = script?.dataset.worker || "/browser/api_worker.mjs";
  const examplesUrl = new URL(script?.dataset.examples || "/sample/examples.json", location.href).href;
  const site = {
    scenarios: script?.dataset.scenarios || "available",
  };
  const realFetch = window.fetch.bind(window);
  const pending = new Map();
  const progressOf = new Map();
  let next = 1;
  let noticeOurs = false;

  function notice(text, kind) {
    const element = document.getElementById("notice");
    if (!element) return;
    if (text === null) {
      if (noticeOurs) element.hidden = true;
      noticeOurs = false;
      return;
    }
    element.hidden = false;
    element.textContent = text;
    element.className = `notice ${kind}`;
    noticeOurs = true;
  }

  const worker = new Worker(workerUrl, { type: "module" });
  let settle;
  const ready = new Promise((resolve, reject) => {
    settle = { resolve, reject };
  });
  ready.catch(() => {});
  worker.onmessage = ({ data }) => {
    if (["answer", "stats", "examples", "installed", "install-failed"].includes(data.type)) {
      const waiting = pending.get(data.id);
      pending.delete(data.id);
      progressOf.delete(data.id);
      waiting?.(data);
    } else if (data.type === "progress") {
      progressOf.get(data.id)?.({ phase: data.phase, bytes: data.bytes, total: data.total });
    } else if (data.type === "ready") {
      if (noticeOurs) notice(null);
      settle.resolve({ ms: data.ms });
    } else if (data.type === "failed") {
      notice(data.message, "error");
      settle.reject(Object.assign(new Error(data.message), { code: data.code }));
    }
  };
  worker.onerror = (event) => {
    const message = event.message || "Worker を開始できませんでした";
    settle.reject(new Error(message));
    for (const [id, waiting] of pending) {
      pending.delete(id);
      waiting({ status: 503, contentType: "application/json; charset=utf-8", body: JSON.stringify({ error: `ブラウザー版の準備に失敗しました：${message}` }) });
    }
  };
  worker.postMessage({ type: "start", examplesUrl });

  // The passphrase the home keeps (demo-v2-home.js's LLM_KEY); without one, the open mode's token
  // while the home found the relay's open mode on (`HinanjoLlmOpen`, #601); otherwise null.
  function passphrase() {
    let kept = null;
    try {
      kept = localStorage.getItem("hinanjo-if-llm-passphrase") || null;
    } catch {
      kept = null;
    }
    return kept ?? (window.HinanjoLlmOpen === true ? "open" : null);
  }

  const ASKED = "sheltersim-storage-persist-asked";
  function askToKeep() {
    try {
      if (localStorage.getItem(ASKED)) return;
      localStorage.setItem(ASKED, new Date().toISOString());
    } catch {
      // Without localStorage the page asks each time; the browser remembers the user's answer.
    }
    try {
      navigator.storage
        ?.persisted?.()
        .then((kept) => (kept ? true : navigator.storage.persist()))
        .catch(() => false);
    } catch {
      // A browser without the Storage API keeps the store as it does.
    }
  }

  function ask(message) {
    const id = next++;
    return new Promise((resolve) => {
      pending.set(id, resolve);
      worker.postMessage({ ...message, id });
    });
  }

  window.fetch = function fetch(input, init) {
    let request;
    try {
      request = new Request(input, init);
    } catch {
      return realFetch(input, init);
    }
    const url = new URL(request.url);
    if (url.origin !== location.origin || !url.pathname.startsWith("/api/v1/")) return realFetch(input, init);
    if (request.method === "POST" && /\/runs\/[^/]+\/resume$/.test(url.pathname)) askToKeep();
    return (async () => {
      const body = request.method === "GET" || request.method === "HEAD" ? null : await request.text();
      // A Scenario preview, a question, an intervention's preview and the analysis report carry the
      // relay's passphrase (the home's 「API を有効にする」, #420): the Worker asks the LLM through
      // the relay (#421, #470). It is never stored there.
      const llm = llmEnabled && request.method === "POST" && /\/(scenario-preview|questions|intervention-preview|pdf-report)$/.test(url.pathname) ? passphrase() : null;
      const answer = await ask({ type: "request", method: request.method, path: url.pathname, search: url.search, body, llm });
      return new Response(answer.body, { status: answer.status, headers: { "Content-Type": answer.contentType, "Cache-Control": "no-store" } });
    })();
  };

  // The home's 「計算済みの例を取得する」 (#411): the site's examples, and the one chosen fetched.
  async function examples() {
    await ready;
    return (await ask({ type: "examples" })).examples;
  }
  async function install(projectId, progress = () => {}) {
    await ready;
    const id = next++;
    progressOf.set(id, progress);
    const done = await new Promise((resolve) => {
      pending.set(id, resolve);
      worker.postMessage({ type: "install", id, projectId });
    });
    if (done.type === "install-failed") throw new Error(done.message);
    return { installed: done.installed, ms: done.ms };
  }

  // A page shown again from the back-forward cache is loaded again, so a site updated since is
  // not left showing its old page.
  addEventListener("pageshow", (event) => {
    if (event.persisted) location.reload();
  });

  window.HinanjoBrowserApi = {
    ready,
    site,
    examples,
    install,
    stats: () => ask({ type: "stats" }).then((data) => data.stats),
  };
})();
