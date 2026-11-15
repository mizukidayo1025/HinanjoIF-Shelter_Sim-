/* The API the screen calls: the server's HTTP routes, or the browser version's, which
   native/wasm/js/api_fetch.js answers in the page. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const { ownProject, state, enc } = app;
  const base = () => `/api/v1/projects/${enc(state.project)}`;
  // New Scenarios go to one's own Project, whichever Run is open.
  const ownBase = () => `/api/v1/projects/${enc(ownProject())}`;
  const runBase = () => `${base()}/runs/${enc(state.run)}`;
  const branchBase = (id = state.branch) => `${runBase()}/branches/${enc(id)}`;
  async function api(path, body) {
    const response = await fetch(path, {
      method: body === undefined ? "GET" : "POST",
      headers: {
        Accept: "application/json",
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    const result = await response.json();
    if (!response.ok)
      throw Object.assign(new Error(result.error || `HTTP ${response.status}`), {
        code: result.code,
      });
    return result;
  }
  async function ensureProject(id = state.project) {
    const list = await api("/api/v1/projects");
    if (!list.projects.some((p) => p.project_id === id))
      await api("/api/v1/projects", { project_id: id });
  }
  async function optional(path) {
    try {
      return await api(path);
    } catch (error) {
      if (error.code === "browser_unsupported") return null;
      throw error;
    }
  }
  Object.assign(app, { base, ownBase, runBase, branchBase, api, ensureProject, optional });
})();
