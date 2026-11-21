/* What is open (Project, Run, branch, minute) and the address that names it. The screen's
   files share their functions through window.HinanjoApp, which this file, loaded first, makes. */
(() => {
  "use strict";
  // A function of a file loaded later, as `const { name } = app.later`: it looks the
  // function up when called, as the files call each other only once all have loaded.
  const app = (window.HinanjoApp = {
    later: new Proxy({}, { get: (_, name) => (...args) => app[name](...args) }),
  });
  const query = new URLSearchParams(location.search);
  let saved = {};
  try {
    saved = JSON.parse(localStorage.getItem("hinanjo-if-backup") || "{}");
  } catch {
    /* Invalid local preference only. */
  }
  // The browser version (native/wasm/js/api_fetch.js) marks itself. The examples' Projects are
  // `sample` and `sample-…` (scripts/build_project_sample.py's EXAMPLES): the browser version
  // fetches one only when the home's 「計算済みの例を取得する」 is chosen from (#411); the server's
  // are in its store when it has them.
  const site = window.HinanjoBrowserApi?.site || {};
  const isExample = (id) => /^sample(-|$)/.test(id);
  // The Project of the person's own Scenarios: new ones are made there, never in an example's.
  // A Project the address names is it, unless it is an example's.
  const askedProject = query.get("project");
  let ownId =
    askedProject && !isExample(askedProject)
      ? askedProject
      : saved.own || (saved.project && !isExample(saved.project) ? saved.project : null);
  // A new id is made only when a Scenario is first made: opening a Run uses no random id (the
  // browser checks pin crypto.randomUUID's sequence for the Run and branch ids).
  const ownProject = () => (ownId ??= `backup-${crypto.randomUUID()}`);
  const state = {
    project: askedProject || ownId,
    run: query.get("run") || null,
    branch: query.get("branch") || "main",
    minute: 0,
    data: null,
    frame: null,
    preview: null,
    intervention: null,
    comparison: null,
    history: [],
    report: null,
    evidence: null,
    zoom: 1,
    mapPan: { x: 0, y: 0 },
    selectedGroup: [],
    selected: null,
    playing: false,
    sceneMode: "schematic",
    motion: null,
    playToken: 0,
    replay: null,
    visualMs: 0,
    busy: false,
    page: "home",
    inputVersion: 0,
    seekVersion: 0,
    scope: 0,
    chatVersion: 0,
    context: null,
    target: null,
    poll: null,
    mode: null,
    // Whether the AI's (LLM's) features are ON ({ on }; null until known, #581).
    llm: null,
    homeVersion: 0,
    forkFrom: null,
    // The minute picked in 「この時刻から分岐を作る」 ({ branch, minute }), when it is not the
    // shown one (#441).
    forkPick: null,
    // A computation this page started to the end (「計算する」), and whether 「中止」 was pressed.
    computing: false,
    cancelled: false,
    multiTouched: false,
  };
  const reference = (id = state.branch) =>
    state.data?.branches.find((b) => b.branch_id === id);
  const lower = () => reference()?.fork_minute ?? 0;
  const upper = () => reference()?.computed_until ?? 0;
  const duration = () => state.total ?? upper();
  function remember() {
    try {
      localStorage.setItem(
        "hinanjo-if-backup",
        JSON.stringify({
          own: ownId,
          project: state.project,
          run: state.run,
          branch: state.branch,
        }),
      );
    } catch {
      /* The address below still holds what is open. */
    }
    // The address names what is open; the home of one's own Project with nothing open is the
    // bare /, which opens the home. The screen is at / since #376 (/demo-v2 before).
    const params = new URLSearchParams();
    if (state.project && (state.run || state.project !== ownId))
      params.set("project", state.project);
    if (state.run) {
      params.set("run", state.run);
      params.set("branch", state.branch);
    }
    const search = params.size ? `?${params}` : "";
    const hash = search || state.page !== "home" ? `#${state.page}` : "";
    history.replaceState(null, "", `/${search}${hash}`);
  }
  Object.assign(app, {
    site, isExample, ownProject, state, reference, lower, upper, duration, remember,
  });
  Object.defineProperty(app, "ownId", { get: () => ownId });
})();
