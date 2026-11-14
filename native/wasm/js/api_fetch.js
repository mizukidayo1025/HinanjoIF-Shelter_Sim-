// The browser version's API (Issue #266): loaded by the browser build's demo-v2.html before the
// screen's scripts, it wraps `window.fetch` and hands every request to this site's `/api/v1/...`
// (the screen's `api()` and its PDF `fetch`) to a dedicated Worker (js/api_worker.mjs), which
// answers it from the browser's store and WASM with the Python server's status and JSON. Other
// requests go to the real fetch. The screen's JS is unchanged.
//
// The script's attributes: `data-worker` the Worker's URL (/browser/api_worker.mjs by default),
// `data-sample` the sample's manifest (/sample/sample.json), `data-example-project` the sample's
// Project and `data-compute-run` the Run the browser computes itself (compute.mjs's COMPUTE_RUN),
// which the screen's home lists (#371). While the sample is fetched and imported on the first
// visit, the screen's notice (#notice) says how far it is.
// `window.HinanjoBrowserApi` has `ready` (a Promise of { installed, ms }), `site` ({ exampleProject,
// computeRun }), `resetCompute()` (the home's 「計算をやり直す」) and `stats()` for checks.
//
// The first time the screen starts a computation (`POST …/runs/{run}/resume`, #267) the page asks
// the browser to keep the store (`navigator.storage.persist()`, the decision on #265: once, when
// the user starts keeping Runs of their own, never on opening the page). It asks once per browser
// (a key in localStorage) and goes on whatever the answer; `persist` is the page's, not a
// Worker's.
(() => {
  "use strict";
  const script = document.currentScript;
  const workerUrl = script?.dataset.worker || "/browser/api_worker.mjs";
  const sampleUrl = new URL(script?.dataset.sample || "/sample/sample.json", location.href).href;
  const site = {
    exampleProject: script?.dataset.exampleProject || "sample",
    computeRun: script?.dataset.computeRun || "run-browser-compute",
  };
  const realFetch = window.fetch.bind(window);
  const pending = new Map();
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
  const mib = (bytes) => (bytes / 2 ** 20).toFixed(1);

  const worker = new Worker(workerUrl, { type: "module" });
  let settle;
  const ready = new Promise((resolve, reject) => {
    settle = { resolve, reject };
  });
  ready.catch(() => {});
  worker.onmessage = ({ data }) => {
    if (data.type === "answer" || data.type === "stats") {
      const waiting = pending.get(data.id);
      pending.delete(data.id);
      waiting?.(data);
    } else if (data.type === "progress") {
      if (data.phase === "fetch") notice(`計算済みの例を読み込んでいます… ${mib(data.bytes)} / ${mib(data.total)} MiB`, "loading");
      else if (data.phase === "import") notice("計算済みの例をこのブラウザーに保存しています…", "loading");
    } else if (data.type === "ready") {
      if (noticeOurs) notice(null);
      settle.resolve({ installed: data.installed, ms: data.ms });
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
  worker.postMessage({ type: "start", sampleUrl });

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
      const answer = await ask({ type: "request", method: request.method, path: url.pathname, search: url.search, body });
      return new Response(answer.body, { status: answer.status, headers: { "Content-Type": answer.contentType, "Cache-Control": "no-store" } });
    })();
  };

  // 「計算をやり直す」: removes the folder of the Run the browser computes (api_worker.mjs's store,
  // `projects/<example>/runs/<compute run>`) under the store's Web Lock. The Worker makes that Run
  // again, not computed, when the page opens next (compute.mjs's ensureComputeRun), so the screen
  // opens the page again after it. The sample is kept. A Run being computed (in this tab or
  // another) has its files open, and is not removed.
  async function resetCompute() {
    const path = ["projects", site.exampleProject, "runs", site.computeRun];
    const remove = async () => {
      let folder = await navigator.storage.getDirectory();
      for (const name of path.slice(0, -1)) folder = await folder.getDirectoryHandle(name);
      await folder.removeEntry(path[path.length - 1], { recursive: true });
    };
    try {
      await (navigator.locks ? navigator.locks.request("sheltersim-browser-store", remove) : remove());
      return { removed: true, message: "消しました。このブラウザーでの計算を、0分からやり直せます。" };
    } catch (error) {
      const name = error?.name;
      if (name === "NotFoundError") return { removed: false, message: "まだ計算していないので、消すものはありません。" };
      if (name === "NoModificationAllowedError" || name === "InvalidModificationError") {
        return {
          removed: false,
          message: "計算中のため消せません。計算を止めるか、計算しているタブを閉じてから、もう一度押してください。",
        };
      }
      return { removed: false, message: `消せませんでした：${error?.message ?? error}` };
    }
  }

  // A page shown again from the back-forward cache is loaded again, so a site updated since is
  // not left showing its old page.
  addEventListener("pageshow", (event) => {
    if (event.persisted) location.reload();
  });

  window.HinanjoBrowserApi = {
    ready,
    site,
    resetCompute,
    stats: () => ask({ type: "stats" }).then((data) => data.stats),
  };
})();
