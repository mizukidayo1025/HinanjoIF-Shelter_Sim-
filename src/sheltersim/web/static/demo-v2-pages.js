/* The pages and what every page shares: the notice, loading, actions, the sidebar and the
   header's button. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const { site, state, reference, lower, upper, duration, remember, $, time } = app;
  const {
    drawAll, loadHome, stopPlayback, comparing, renderForkPicker, pairIds, openComparison,
    renderMultiChoices, multiChosen, loadReport, loadDetail, loadFailure,
  } = app.later;
  let noticeTimer;
  function note(text, kind = "") {
    clearTimeout(noticeTimer);
    $("notice").hidden = false;
    $("notice").textContent = text;
    $("notice").className = `notice ${kind}`;
    if (kind === "success")
      noticeTimer = setTimeout(() => {
        $("notice").hidden = true;
      }, 1800);
  }
  // The header's one button (#441): 「計算する」 (or 「続きから計算する」) while the shown branch
  // (on the comparison page, the pair) is not computed to its end, 「中止」 with how far it is
  // while the Run computes, and 「シミュレーションを再生」 once computed. On a narrow screen it shows
  // its icon alone.
  function computeTargets() {
    if (!state.run || !state.data) return [];
    const ids =
      state.page === "comparison" && comparing() ? pairIds() : [state.branch];
    return ids.filter((id) => reference(id) && reference(id).computed_until < duration());
  }
  const computedToEnd = (id = state.branch) =>
    Boolean(reference(id)) && reference(id).computed_until >= duration();
  function mainButtonMode() {
    if (state.data?.status === "running") return "cancel";
    return state.run && state.frame && computeTargets().length ? "compute" : "play";
  }
  function setPlayLabels() {
    const button = $("play"),
      mode = mainButtonMode();
    let icon = state.playing ? "Ⅱ" : "▶",
      label = state.playing ? "再生を一時停止" : "シミュレーションを再生";
    if (mode === "cancel") {
      const branch =
          (state.data.branches || []).find((b) => b.status === "running") ||
          reference(),
        span = Math.max(1, duration() - (branch?.fork_minute ?? 0)),
        done = (branch?.computed_until ?? 0) - (branch?.fork_minute ?? 0),
        percent = Math.max(0, Math.min(99, Math.floor((done / span) * 100)));
      icon = "■";
      label = `中止（計算中 ${percent}%）`;
    } else if (mode === "compute") {
      const resumed = computeTargets().some(
        (id) => reference(id).computed_until > reference(id).fork_minute,
      );
      label = resumed ? "続きから計算する" : "計算する";
    }
    button.dataset.mode = mode;
    button.classList.toggle("danger", mode === "cancel");
    button.classList.toggle("primary", mode !== "cancel");
    button.querySelector(".play-icon").textContent = icon;
    button.querySelector(".play-label").textContent = label;
    button.setAttribute("aria-label", label);
  }
  // Until the shown branch (on the comparison page, the pair) is computed to its end, the
  // simulation and comparison pages are dimmed and cannot be used (#482): no half-computed
  // view, only the header's button that computes it, and how far it is.
  function showUncomputed(mode) {
    const page = ["simulation", "comparison"].includes(state.page) && state.page,
      waiting = Boolean(page && state.run && state.data) && mode !== "play";
    document.body.classList.toggle("is-uncomputed", waiting);
    for (const id of ["simulation", "comparison"])
      $(`page-${id}`).inert = waiting && id === page;
    $("compute-first").hidden = !waiting;
    $("compute-first").textContent =
      mode === "cancel"
        ? "計算しています。最後まで計算すると、ここに結果が出ます。"
        : "右上の「計算する」で最後まで計算すると、ここに結果が出ます。";
  }
  // While something the screen shows is loading (#412), the open page is dimmed under a spinner
  // (body.is-loading, demo-v2.css) and <main> is aria-busy. The HTML starts in that state, so the
  // page does not look empty before init() ends. Loads may overlap; the last one to end clears it.
  let loads = 0;
  async function loading(task) {
    loads++;
    document.body.classList.add("is-loading");
    document.querySelector("main").setAttribute("aria-busy", "true");
    try {
      return await task();
    } finally {
      if (--loads === 0) {
        document.body.classList.remove("is-loading");
        document.querySelector("main").removeAttribute("aria-busy");
      }
    }
  }
  async function action(label, task) {
    if (state.busy) return;
    state.busy = true;
    stopPlayback();
    document.body.classList.add("is-busy");
    note(label, "loading");
    try {
      await task();
      // Done: the screen shows the result; the notice says nothing more (#442).
      if ($("notice").classList.contains("loading")) $("notice").hidden = true;
    } catch (error) {
      note(`操作を完了できませんでした：${error.message}`, "error");
    } finally {
      state.busy = false;
      document.body.classList.remove("is-busy");
      controls();
    }
  }
  const on = (id, callback) =>
    $(id).addEventListener("click", () =>
      action("処理中です。APIの応答を待っています…", callback),
    );
  const PAGES = ["home", "scenario", "simulation", "comparison", "report", "detail"];
  // Why a page has nothing to show now, or "" (#413): its sidebar item is then faded and cannot
  // be pressed, with the reason as its title, and show() opens another page instead.
  function pageBlocked(page) {
    const open = Boolean(state.run && state.data),
      branches = state.data?.branches || [],
      closed = "シナリオを開くと使えます";
    switch (page) {
      case "scenario":
        // A site built without the shelter index cannot make Scenarios (api_fetch.js).
        return site.scenarios === "unavailable"
          ? "このサイトでは条件から新しいシナリオを作れません"
          : "";
      case "simulation":
      case "detail":
        return open ? "" : closed;
      case "comparison":
        // A branch is made on the comparison page (「この時刻から分岐を作る」) from a branch
        // computed to its end, and a pair is computed there.
        if (!open) return closed;
        return state.forkFrom || branches.length > 1 || computedToEnd()
          ? ""
          : "シナリオを最後まで計算すると使えます";
      case "report":
        // The report is of a branch computed to its end (#482).
        if (!open) return closed;
        return computedToEnd() ? "" : "開いている分岐を最後まで計算すると使えます";
      default:
        return "";
    }
  }
  function show(page) {
    if (!PAGES.includes(page)) page = "home";
    if (pageBlocked(page))
      page = pageBlocked("simulation") ? "home" : "simulation";
    const changedPage = state.page !== page;
    state.page = page;
    if (page !== "comparison") state.forkFrom = null;
    document.body.dataset.view = page;
    stopPlayback();
    document
      .querySelectorAll(".page")
      .forEach((n) => n.classList.toggle("active", n.id === `page-${page}`));
    document.querySelectorAll("[data-page]").forEach((n) => {
      n.classList.toggle("active", n.dataset.page === page);
      n.setAttribute(
        "aria-current",
        n.dataset.page === page ? "page" : "false",
      );
    });
    if (changedPage) scrollTo(0, 0);
    remember();
    controls();
    requestAnimationFrame(drawAll);
    if (page === "report" && state.run)
      void action("レポートと記録済みの根拠を取得しています…", () =>
        loading(loadReport),
      );
    if (page === "home")
      loading(loadHome).catch((e) => note(loadFailure(e), "error"));
    if (page === "detail") {
      $("detail-empty").hidden = Boolean(state.run);
      $("detail-body").hidden = !state.run;
      if (state.run)
        void action("記録・モデル・出典を取得しています…", () =>
          loading(loadDetail),
        );
    }
    if (page === "comparison") {
      renderMultiChoices();
      openComparison();
    }
  }
  function controls() {
    const has = Boolean(state.run && state.frame),
      running = state.data?.status === "running",
      working = state.busy || running;
    $("intervention-preview").disabled = !has || working;
    $("intervention-preview-text").disabled = !has || working;
    $("branch-here").disabled = !has || working;
    $("intervention-panel").classList.toggle(
      "has-intervention",
      Boolean(state.intervention),
    );
    renderForkPicker(working);
    document.querySelectorAll(".nav-item[data-page]").forEach((n) => {
      const why = pageBlocked(n.dataset.page);
      n.disabled = Boolean(why);
      n.title = why;
    });
    const single = !has || state.busy || upper() <= lower(),
      mode = mainButtonMode();
    setPlayLabels();
    showUncomputed(mode);
    $("play").disabled =
      mode === "cancel"
        ? false
        : mode === "compute"
          ? !has || state.busy || state.computing
          : state.page === "simulation"
            ? single
            : state.page === "comparison"
              ? comparing()
                ? state.busy ||
                  state.comparison.common_range[1] <= state.comparison.common_range[0]
                : single
              : true;
    $("seek").disabled = !has || state.busy;
    $("branch-select").disabled = !has || working;
    $("approve").disabled =
      !state.preview?.can_start || !$("scenario-check").checked || working;
    $("intervention-apply").disabled =
      !state.intervention?.can_apply ||
      !$("intervention-check").checked ||
      working;
    for (const id of ["report-refresh", "save-report", "ask"])
      $(id).disabled = !has || state.busy;
    $("save-report").disabled = !has || working;
    const branches = state.data?.branches || [],
      pair = branches.length > 1;
    $("compare-branch").disabled = !pair || working;
    $("intervention-toggle").disabled = !has || working;
    $("report-branch").disabled = !has || working;
    $("report-overlay").disabled = working || $("report-overlay").options.length < 2;
    const many = multiChosen().length;
    $("multi-compare-run").disabled = many < 3 || many > 8 || working;
    $("compare-seek").disabled = comparing() ? state.busy : single;
    if (state.page === "comparison" && !comparing() && has) {
      $("compare-seek").min = lower();
      $("compare-seek").max = upper();
      $("compare-seek").value = state.minute;
      $("compare-time").textContent = time(state.minute);
    }
    if (has) {
      $("seek").min = lower();
      $("seek").max = upper();
      $("seek").value = state.minute;
      $("seek-label").textContent = time(state.minute);
      $("computed-label").textContent =
        `${time(upper())}まで閲覧できます`;
    }
  }
  Object.assign(app, { note, computeTargets, setPlayLabels, loading, action, on, show, controls });
})();
