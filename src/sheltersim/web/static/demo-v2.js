/* Independent backup UI. WorldState and persisted API artifacts are authoritative. */
(() => {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const enc = encodeURIComponent;
  const json = (value) => JSON.stringify(value, null, 2);
  const llmModeLabel = (mode) =>
    ({
      record: "Claude API接続",
      offline: "API未接続",
      replay: "記録応答リプレイ",
    })[mode] ||
    mode ||
    "未記録";
  const query = new URLSearchParams(location.search);
  let saved = {};
  try {
    saved = JSON.parse(localStorage.getItem("hinanjo-if-backup") || "{}");
  } catch {
    /* Invalid local preference only. */
  }
  // The browser version (native/wasm/js/api_fetch.js) marks itself: its example Project, which it
  // fetches only when the home's 「計算済みの例を取得する」 is chosen from (#411). The server's
  // example is the Project `sample` (scripts/build_project_sample.py), when it has one.
  const site = window.HinanjoBrowserApi?.site || {};
  const exampleProject = site.exampleProject || "sample";
  // The Project of the person's own Scenarios: new ones are made there, never in the example.
  // A Project the address names is it, unless it is the example's.
  const askedProject = query.get("project");
  let ownId =
    askedProject && askedProject !== exampleProject
      ? askedProject
      : saved.own ||
        (saved.project && saved.project !== exampleProject ? saved.project : null);
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
  const metricDefs = [
    {
      key: "stress_mean",
      label: "平均ストレス",
      unit: "/100",
      factor: 100,
      icon: "♟",
      note: "避難者全員のストレスの平均（0〜100、高いほどつらい）",
    },
    {
      key: "toilet_wait_max_minutes",
      label: "最大トイレ待機時間",
      unit: "分",
      factor: 1,
      icon: "◷",
      note: "ここまでで一番長く待った人の待ち時間",
    },
    {
      key: "staff_load_ratio",
      label: "スタッフ負荷",
      unit: "%",
      factor: 100,
      icon: "♟",
      note: "スタッフのうち、いま対応中の人の割合",
    },
    {
      key: "unresolved_support_request_count",
      label: "未解決の支援要請",
      unit: "件",
      factor: 1,
      icon: "!",
      note: "助けを求めて、まだ対応されていない件数",
    },
    {
      key: "population",
      label: "現在の避難者数",
      unit: "人",
      factor: 1,
      icon: "♟",
      note: "この時刻に避難所にいる人数",
    },
    {
      key: "toilet_queue_length",
      label: "トイレ待機人数",
      unit: "人",
      factor: 1,
      icon: "◷",
      note: "いまトイレの列に並んでいる人数",
    },
  ];
  const zoneNames = {
    sleeping: "体育館・避難スペース",
    quiet: "福祉・休養スペース",
    medical: "救護室",
    food: "物資配布",
    water: "給水",
    toilet: "トイレ",
    intake: "受付",
    staff: "スタッフ本部",
    corridor: "通路",
  };
  const eventNames = {
    resource_reserve: "物資の受け取りを予約",
    resource_consume: "物資を使用（在庫が減少）",
    resource_service_complete: "食事・水の受け取りが完了",
    resource_service_cancel: "食事・水の受け取りを中止",
    resource_delivery: "物資が届いた",
    delivery_delayed: "物資の到着が遅れている",
    staff_operation_transition: "スタッフ業務の段階が変わった",
    staff_operation_arrive: "スタッフが現場に到着",
    staff_operation_complete: "スタッフ業務が完了",
    staff_operation_relief: "スタッフが交代",
    staff_operation_preempted: "スタッフ業務を中断（別の対応を優先）",
    staff_support_complete: "スタッフによる避難者支援が完了",
    staff_task_complete: "スタッフの作業が完了",
    staff_task_start: "スタッフの作業を開始",
    staff_unavailable: "スタッフが対応できなくなった",
    staff_restored: "スタッフが対応に復帰",
    toilet_service_complete: "トイレの利用が完了",
    facility_queue_transition: "待ち列の状況が変わった",
    facility_queue_abandon: "待ち列をあきらめて離れた",
    support_request: "避難者が支援を求めた（対応待ち）",
    support_request_timeout: "支援要請が対応されないまま時間切れ",
    social_interaction: "避難者どうしの会話・助け合い",
    care_support_transition: "介助・見守り支援の段階が変わった",
    facility_outage: "設備が使えなくなった",
    facility_recovery: "設備が使えるようになった",
    facility_failure: "設備が故障",
    facility_repair: "設備の修理が完了",
    water_outage: "断水が発生",
    water_restored: "断水が復旧",
    agent_action_lifecycle: "避難者の行動の開始・終了",
    agent_travel_complete: "避難者の移動が完了",
    agent_travel_start: "避難者が移動を開始",
    zone_edge_capacity_wait: "通路が混んで移動待ち",
    arrival: "避難者が到着",
    agent_arrival: "避難者が到着",
    population_approach_complete: "避難者が避難所に到着",
    population_return_admission: "一時外出した避難者が戻った",
    information: "情報を伝えた",
    information_contact_update: "情報が避難者に伝わった",
    proximity_contact_update: "避難者どうしが近くで接触",
    intervention: "介入（もしもの条件）を適用",
    effect_program_apply: "介入の効果が始まった",
    effect_program_expire: "介入の効果が終わった",
  };
  // 色分け：resolved=青（完了・解決）、unresolved=赤（未解決・問題）、それ以外=灰（経過の記録）
  const eventTones = {
    resource_service_complete: "resolved",
    resource_delivery: "resolved",
    staff_operation_complete: "resolved",
    staff_support_complete: "resolved",
    staff_task_complete: "resolved",
    staff_restored: "resolved",
    toilet_service_complete: "resolved",
    facility_recovery: "resolved",
    facility_repair: "resolved",
    water_restored: "resolved",
    resource_service_cancel: "unresolved",
    delivery_delayed: "unresolved",
    staff_operation_preempted: "unresolved",
    staff_unavailable: "unresolved",
    facility_queue_abandon: "unresolved",
    support_request: "unresolved",
    support_request_timeout: "unresolved",
    facility_outage: "unresolved",
    facility_failure: "unresolved",
    water_outage: "unresolved",
  };
  const eventTone = (e) =>
    ["rejected", "failed"].includes(e.status)
      ? "unresolved"
      : eventTones[e.event_type] || "record";
  const activityNames = {
    moving: "移動中",
    queued: "待機中",
    sleeping: "休息中",
    idle: "待機",
    resting: "休息中",
    seeking_help: "支援を求めている",
    receiving_support: "支援対応中",
    using_facility: "施設利用中",
    eating: "食事中",
    drinking: "給水中",
    toilet: "トイレ利用中",
  };
  // The disaster as the screen says it: the text parser records an English family name
  // (src/sheltersim/llm/region_prepare.py), the form records what was chosen.
  const disasterName = (value) =>
    ({
      flood: "洪水",
      earthquake: "地震",
      tsunami: "津波",
      landslide: "土砂災害",
      storm_surge: "高潮",
      typhoon: "台風",
      heavy_snow: "大雪",
      heatwave: "猛暑",
    })[value] || value;
  // How the model can carry out an intervention (EffectProgram's support_mode), in words.
  const supportModeName = (mode) =>
    ({
      native: "モデルでそのまま再現",
      composed: "作用の組み合わせで再現",
      approximate: "近似して再現",
      clarification_required: "対象の確認が必要",
      not_simulatable: "モデルでは再現できない",
    })[mode] ||
    mode ||
    "再現の方法は未記録";
  const base = () => `/api/v1/projects/${enc(state.project)}`;
  // New Scenarios go to one's own Project, whichever Run is open.
  const ownBase = () => `/api/v1/projects/${enc(ownProject())}`;
  const runBase = () => `${base()}/runs/${enc(state.run)}`;
  const branchBase = (id = state.branch) => `${runBase()}/branches/${enc(id)}`;
  const reference = (id = state.branch) =>
    state.data?.branches.find((b) => b.branch_id === id);
  const lower = () => reference()?.fork_minute ?? 0;
  const upper = () => reference()?.computed_until ?? 0;
  const duration = () => state.total ?? upper();
  const time = (m) =>
    `${Math.floor(m / 1440) + 1}日目 ${String(Math.floor((m % 1440) / 60)).padStart(2, "0")}:${String(m % 60).padStart(2, "0")}`;
  const shortBranch = (id) =>
    id === "main" ? "元のシナリオ" : `介入分岐 ${id.slice(-6)}`;
  const runStatusLabel = (status) =>
    ({
      running: "計算中",
      paused: "計算待機",
      completed: "計算完了",
      failed: "計算失敗",
      interrupted: "計算中断",
      created: "作成済み",
    })[status] ||
    status ||
    "未作成";
  const num = (n) =>
    typeof n === "number" && Number.isFinite(n)
      ? Math.abs(n) >= 100
        ? Math.round(n).toLocaleString("ja-JP")
        : Number(
            n.toFixed(Math.abs(n) > 0 && Math.abs(n) < 1 ? 4 : 1),
          ).toLocaleString("ja-JP")
      : "—";
  function el(tag, text, cls) {
    const node = document.createElement(tag);
    if (text !== undefined && text !== null) node.textContent = String(text);
    if (cls) node.className = cls;
    return node;
  }
  const iconPaths = {
    scenario: ["M6 3h9l4 4v14H5V3z", "M14 3v5h5M8 12h8M8 16h6"],
    simulation: [
      "M4 19a9 9 0 1 1 16 0",
      "M12 13l5-6",
      "M6 15h1M7 8l1 1M12 5v1M18 14h1",
    ],
    comparison: [
      "M6 5v14M6 12h6l6-6",
      "M8 5a2 2 0 1 1-4 0 2 2 0 0 1 4 0z",
      "M8 19a2 2 0 1 1-4 0 2 2 0 0 1 4 0z",
      "M20 5a2 2 0 1 1-4 0 2 2 0 0 1 4 0z",
    ],
    report: ["M4 20h17M6 17v-6h3v6M11 17V5h3v12M16 17V8h3v9"],
    detail: ["M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0z", "M12 11v6M12 7.5h.01"],
    people: [
      "M9 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
      "M3 21v-5a6 6 0 0 1 12 0v5z",
      "M17 4a3 3 0 0 1 0 6M17 13a5 5 0 0 1 5 5v3h-4",
    ],
    clock: ["M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0z", "M12 6v6l4 2"],
    staff: [
      "M12 10a4 4 0 1 0 0-8 4 4 0 0 0 0 8z",
      "M4 22v-5a8 8 0 0 1 16 0v5z",
      "M9 15l3 3 3-3",
    ],
    alert: ["M12 3l10 18H2L12 3z", "M12 9v5M12 17h.01"],
    home: ["M2 11L12 2l10 9", "M5 9v13h14V9M9 22v-8h6v8"],
    queue: [
      "M5 7a2 2 0 1 0 0-4 2 2 0 0 0 0 4z",
      "M2 16v-5a3 3 0 0 1 6 0v5M3 16v6M7 16v6M12 5h10M12 11h10M12 17h10",
    ],
  };
  function uiIcon(name) {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
    svg.setAttribute("fill", "none");
    svg.setAttribute("stroke", "currentColor");
    svg.setAttribute("stroke-width", "1.9");
    svg.setAttribute("stroke-linecap", "round");
    svg.setAttribute("stroke-linejoin", "round");
    for (const d of iconPaths[name] || iconPaths.people) {
      const path = document.createElementNS(svg.namespaceURI, "path");
      path.setAttribute("d", d);
      svg.append(path);
    }
    return svg;
  }
  const chatWelcome = $("chat").firstElementChild.cloneNode(true);
  function resetChat() {
    $("chat").replaceChildren(chatWelcome.cloneNode(true));
  }
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
  function stopPlayback(preserve = false) {
    state.playing = false;
    state.playToken++;
    if (state.replay) {
      if (preserve) state.replay.pause();
      else state.replay.dispose();
    }
    if (!preserve) {
      state.replay = null;
      state.motion = null;
      state.visualMs = 0;
    }
    setPlayLabels();
    if (window.HinanjoReplayView)
      window.HinanjoReplayView = {
        ...window.HinanjoReplayView,
        playing: false,
      };
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
  // A Run's name on the home and in the header: where, what disaster, how many people, how long.
  function runTitle(run, scenarios) {
    const scenario = scenarios.find(
        (s) =>
          s.scenario_id === run.scenario_id &&
          s.scenario_version === run.scenario_version,
      )?.scenario,
      request =
        scenario?.provenance?.natural_language_input ||
        scenario?.provenance?.initial_scenario_request,
      people = scenario?.profiles?.length,
      days = scenario?.duration_minutes / 1440;
    return {
      name: request
        ? `${request.municipality} · ${disasterName(request.disaster)}`
        : run.scenario_id,
      size: [
        typeof people === "number" ? `${people}人` : null,
        Number.isFinite(days) ? `${num(days)}日間` : null,
      ]
        .filter(Boolean)
        .join(" · "),
      total: scenario?.duration_minutes,
    };
  }
  // The screen calls every Run a シナリオ (#373), named by its conditions. One's own Runs of the
  // same conditions are told apart by when they were made (#447): 「11/15 10:30 作成」 in local
  // time, from the record's `created_at` (a Run made before #447 has none). An example's Runs are
  // named by their conditions alone: its rows carry the 「例」 mark instead.
  function runNames(runs, scenarios, { example = false } = {}) {
    const titled = runs.map((r) => {
      const { name, size } = runTitle(r, scenarios);
      return { run: r, name: [name, size].filter(Boolean).join(" · ") };
    });
    const count = new Map();
    for (const t of titled) count.set(t.name, (count.get(t.name) || 0) + 1);
    return new Map(
      titled.map((t) => {
        const made = example || count.get(t.name) < 2 ? null : createdLabel(t.run.created_at);
        return [t.run.run_id, made ? `${t.name} · ${made}` : t.name];
      }),
    );
  }
  // When a Run was made, in local time: 「11/15 10:30 作成」; null without a creation time.
  function createdLabel(createdAt) {
    const date = createdAt ? new Date(createdAt) : null;
    if (!date || Number.isNaN(date.getTime())) return null;
    const two = (n) => String(n).padStart(2, "0");
    return `${date.getMonth() + 1}/${date.getDate()} ${two(date.getHours())}:${two(date.getMinutes())} 作成`;
  }
  // How far a Run is computed, in words.
  function runProgress(run, total) {
    const main = run.branches?.find((b) => b.branch_id === "main"),
      until = main?.computed_until ?? 0;
    if (run.status === "completed" || (total && until >= total))
      return "最後まで計算済み";
    if (run.status === "running") return `計算中 · ${time(until)}まで`;
    if (!until) return "まだ計算していません";
    return `${time(until)}まで計算済み`;
  }
  function runLink(projectId, run, scenarios, label, named) {
    const { name: title, size, total } = runTitle(run, scenarios),
      name = named || title,
      params = new URLSearchParams({
        project: projectId,
        run: run.run_id,
        branch: "main",
      }),
      link = el("a", undefined, "home-run");
    link.href = `/?${params}#simulation`;
    link.append(
      el("strong", label || name),
      el("span", [label ? name : null, named ? null : size].filter(Boolean).join(" · ")),
      el("small", runProgress(run, total)),
    );
    link.addEventListener("click", (event) => {
      if (event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey)
        return;
      event.preventDefault();
      void openRun(projectId, run.run_id);
    });
    return link;
  }
  // The home: the examples' Runs and one's own Scenarios. It reads the Projects and changes
  // nothing. The browser version holds an example once it is fetched (#411): the home lists the
  // examples it holds and offers the others to fetch.
  async function loadHome() {
    const scope = ++state.homeVersion,
      browser = window.HinanjoBrowserApi,
      offered = browser ? await browser.examples() : [],
      exampleIds = browser
        ? offered.filter((e) => e.installed).map((e) => e.project_id)
        : [exampleProject],
      listed = new Set(
        (await api("/api/v1/projects")).projects.map((p) => p.project_id),
      ),
      read = async (id) => (listed.has(id) ? api(`/api/v1/projects/${enc(id)}`) : null),
      [examples, own] = await Promise.all([
        Promise.all(exampleIds.map(async (id) => [id, await read(id)])),
        !ownId || exampleIds.includes(ownId) ? null : read(ownId),
      ]);
    if (scope !== state.homeVersion) return;
    // One list of the saved Scenarios (#440): the examples', marked 「例」, then one's own.
    const rows = [
      ...examples.flatMap(([id, project]) =>
        homeRows(id, project, { example: true, deletable: Boolean(browser) }),
      ),
      ...(own ? homeRows(ownId, own, { example: false, deletable: true }) : []),
    ];
    $("home-list").replaceChildren(
      ...(rows.length ? rows : [el("p", "まだシナリオはありません。", "quiet-message")]),
    );
    $("home-fetch").hidden = !offered.some((e) => !e.installed);
    $("home-api").hidden = !browser;
    if (browser) void refreshApi();
    $("home-browser-notes").hidden = !browser;
    $("home-new").disabled = site.scenarios === "unavailable";
    $("home-new-note").hidden = site.scenarios !== "unavailable";
  }
  // A Project's Runs as rows of the home's list: each opens its Run, and has 「削除」 when it may
  // be deleted. The server's example cannot be fetched again, so it has none there (#440).
  function homeRows(projectId, project, { example, deletable }) {
    const runs = project?.runs || [],
      names = runNames(runs, project?.scenarios || [], { example });
    return runs.map((run) => {
      const name = names.get(run.run_id),
        row = el("div", undefined, "home-item"),
        link = runLink(projectId, run, project.scenarios, undefined, name);
      if (example) link.querySelector("strong").append(" ", el("span", "例", "home-tag"));
      row.append(link);
      if (deletable) {
        const remove = el("button", "削除", "danger compact home-delete");
        remove.type = "button";
        remove.setAttribute("aria-label", `「${name}」を削除`);
        remove.addEventListener("click", () =>
          action("シナリオを削除しています…", () =>
            deleteRun(projectId, run.run_id, name),
          ),
        );
        row.append(remove);
      }
      return row;
    });
  }
  // 「計算済みの例を取得する」 (#411): the site's examples, what each holds and its size; the one
  // chosen is fetched and saved in this browser, then opened.
  const mb = (bytes) => `約${num(Math.max(1, Math.round(bytes / 1e6)))} MB`;
  function exampleTitle(example) {
    const days = example.duration_minutes / 1440;
    return {
      name: [
        example.municipality,
        example.disaster ? disasterName(example.disaster) : null,
      ]
        .filter(Boolean)
        .join(" · ") || example.project_id,
      size: [
        Number.isFinite(example.people) ? `${example.people}人` : null,
        Number.isFinite(days) ? `${num(days)}日間` : null,
        example.branches > 1 ? `分岐 ${example.branches}本` : null,
      ]
        .filter(Boolean)
        .join(" · "),
    };
  }
  // 「API を有効にする」 (#420): the passphrase handed out for the browser site's LLM relay
  // (deploy/workers/relay.mjs, #419), kept in this browser only. The relay checks it and says
  // whose it is and what is left of the site's daily cap; the screen never sees an API key.
  const LLM_KEY = "hinanjo-if-llm-passphrase";
  const storedPassphrase = () => {
    try {
      return localStorage.getItem(LLM_KEY) || "";
    } catch {
      return "";
    }
  };
  function keepPassphrase(value) {
    try {
      if (value) localStorage.setItem(LLM_KEY, value);
      else localStorage.removeItem(LLM_KEY);
    } catch {
      /* Not kept: it is asked again on the next visit. */
    }
  }
  async function llmSession(passphrase) {
    let response;
    try {
      response = await fetch("/llm/v1/session", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${passphrase}`,
          "Content-Type": "application/json",
        },
        body: "{}",
      });
    } catch {
      throw Object.assign(new Error("中継に接続できませんでした。"), { code: "llm_unreachable" });
    }
    let body = null;
    try {
      body = await response.json();
    } catch {
      /* A site without the relay answers its 404 page. */
    }
    if (!response.ok)
      throw Object.assign(
        new Error(body?.error || "このサイトではAPIを有効にできません。"),
        { code: body?.code || `http_${response.status}` },
      );
    return body;
  }
  function showApi(session, message) {
    $("home-api-status").textContent = session
      ? session.available
        ? `有効です（${session.recipient}）。今日のサイト全体の残り：$${Number(session.remaining_usd).toFixed(2)}`
        : `合言葉は確認できました（${session.recipient}）が、中継にAPIキーがまだ設定されていません。`
      : message || "まだ有効にしていません。";
    $("home-api-status").classList.toggle("on", Boolean(session?.available));
    $("home-api-enable").hidden = Boolean(session);
    $("home-api-disable").hidden = !session;
  }
  async function refreshApi() {
    const passphrase = storedPassphrase();
    if (!passphrase) return showApi(null);
    try {
      showApi(await llmSession(passphrase));
    } catch (error) {
      // A passphrase no longer let in is forgotten; a relay out of reach keeps it.
      if (["llm_passphrase_invalid", "llm_passphrase_required"].includes(error.code))
        keepPassphrase("");
      showApi(null, error.message);
    }
  }
  async function enableApi(event) {
    event.preventDefault();
    const passphrase = $("api-passphrase").value.trim();
    if (!passphrase) return;
    $("api-submit").disabled = true;
    $("api-dialog-note").textContent = "確認しています…";
    try {
      const session = await llmSession(passphrase);
      keepPassphrase(passphrase);
      $("api-passphrase").value = "";
      $("api-dialog-note").textContent = "";
      $("api-dialog").close();
      showApi(session);
    } catch (error) {
      $("api-dialog-note").textContent = error.message;
    } finally {
      $("api-submit").disabled = false;
    }
  }
  let fetching = false;
  async function openExamples() {
    const dialog = $("examples-dialog");
    $("examples-progress").hidden = true;
    $("examples-list").replaceChildren(el("p", "例の一覧を読み込んでいます…", "quiet-message"));
    if (!dialog.open) dialog.showModal();
    let examples;
    try {
      examples = await window.HinanjoBrowserApi.examples();
    } catch (error) {
      $("examples-list").replaceChildren(
        el("p", `例の一覧を読めませんでした：${error.message}`, "quiet-message"),
      );
      return;
    }
    $("examples-list").replaceChildren(
      ...(examples.length
        ? examples.map((example) => {
            const { name, size } = exampleTitle(example),
              row = el("div", undefined, "home-run examples-item"),
              button = el(
                "button",
                example.installed ? "開く" : "取得する",
                example.installed ? undefined : "primary",
              );
            button.type = "button";
            button.addEventListener("click", () => void chooseExample(example));
            row.append(
              el("strong", name),
              el("span", size),
              el("small", example.installed ? "取得済み" : `${mb(example.bytes)}を読み込みます`),
              button,
            );
            return row;
          })
        : [el("p", "このサイトには計算済みの例がありません。", "quiet-message")]),
    );
  }
  async function chooseExample(example) {
    if (fetching) return;
    fetching = true;
    const buttons = [...$("examples-list").querySelectorAll("button")],
      bar = $("examples-progress-bar"),
      text = $("examples-progress-text");
    buttons.forEach((b) => (b.disabled = true));
    try {
      if (!example.installed) {
        $("examples-progress").hidden = false;
        bar.removeAttribute("value");
        text.textContent = "読み込みを始めています…";
        await window.HinanjoBrowserApi.install(example.project_id, ({ phase, bytes, total }) => {
          if (phase === "fetch") {
            bar.max = Math.max(1, total);
            bar.value = bytes;
            text.textContent = `読み込んでいます… ${mb(bytes)} / ${mb(total)}`;
          } else if (phase === "import") {
            bar.removeAttribute("value");
            text.textContent = "このブラウザーに保存しています…";
          }
        });
      }
      const project = await api(`/api/v1/projects/${enc(example.project_id)}`),
        run = (project.runs || []).find((r) => r.status === "completed") || project.runs?.[0];
      $("examples-dialog").close();
      if (run) await openRun(example.project_id, run.run_id);
      else show("home");
    } catch (error) {
      text.textContent = error.message;
      $("examples-progress").hidden = false;
      bar.value = 0;
    } finally {
      fetching = false;
      buttons.forEach((b) => (b.disabled = false));
    }
  }
  async function openRun(projectId, runId) {
    await action("シナリオを開いています…", () =>
      loading(() => openRunInPlace(runId, projectId)),
    );
    if (state.run === runId && state.data) show("simulation");
  }
  async function openRunInPlace(runId, projectId = state.project) {
    clearTimeout(state.poll);
    state.scope++;
    state.project = projectId;
    state.run = runId;
    state.branch = "main";
    state.minute = 0;
    state.loadedRun = null;
    state.data = null;
    state.frame = null;
    state.comparison = null;
    state.history = [];
    state.context = null;
    state.selected = null;
    state.selectedGroup = [];
    state.mapPan = { x: 0, y: 0 };
    state.zoom = 1;
    state.report = null;
    invalidateIntervention();
    resetChat();
    await refreshRuns();
    await refreshRun();
  }
  function newScenario() {
    clearTimeout(state.poll);
    stopPlayback();
    state.scope++;
    state.project = ownProject();
    state.run = null;
    state.branch = "main";
    state.data = null;
    state.frame = null;
    state.loadedRun = null;
    state.comparison = null;
    state.history = [];
    state.context = null;
    state.report = null;
    invalidateIntervention();
    resetChat();
    clearRunView();
    show("scenario");
    void refreshRuns().catch(() => {
      /* The Project is made on the first preview. */
    });
  }
  function invalidateScenario() {
    state.inputVersion++;
    state.preview = null;
    $("scenario-check").checked = false;
    $("scenario-check").disabled = true;
    $("approve").disabled = true;
    $("preview-result").hidden = true;
    $("preview-empty").hidden = false;
    $("preview-step").classList.remove("current");
    $("blockers").replaceChildren(
      el("p", "入力が変更されました。再プレビューしてください。", "muted"),
    );
  }
  // What the screen shows of a Run, emptied when none is open.
  function clearRunView() {
    renderKpis($("sim-kpis"));
    renderKpiStrip($("report-kpis"));
    $("seek-label").textContent = "開始前";
    $("branch-select").replaceChildren();
    $("compare-branch").replaceChildren();
    $("issues").replaceChildren();
    $("events").replaceChildren();
    $("people-list").replaceChildren();
    $("person-detail").replaceChildren();
    showOpenName("");
    state.scenario = null;
    showMode(null);
  }
  // How the open Run answers questions (its LLM mode), on the report's chat.
  function showMode(llm) {
    state.mode = llm?.mode ?? null;
    $("chat-mode").textContent = !llm
      ? "回答の作り方は、シナリオを開くと表示します"
      : llm.mode === "record"
        ? "Claude APIに接続して回答を生成します"
        : llm.mode === "replay"
          ? "記録したAPIの応答を再生して回答します"
          : "APIキーのない環境のため、質問にはAIの回答を作れません。レポートと記録は確かめられます";
  }
  // The open Run's conditions on the records page (#502): the scenario page makes new ones only.
  function renderScenarioRecord() {
    if (!state.scenario) return;
    const scenario = state.scenario,
      request =
        scenario.provenance?.natural_language_input ||
        scenario.provenance?.initial_scenario_request ||
        {};
    $("scenario-open-chips").replaceChildren(
      ...[
        request.municipality,
        request.disaster ? disasterName(request.disaster) : null,
        scenario.profiles ? `避難者 ${scenario.profiles.length}人` : null,
        scenario.staff ? `スタッフ ${scenario.staff.length}人` : null,
        scenario.duration_minutes
          ? `${num(scenario.duration_minutes / 1440)}日間`
          : null,
        typeof scenario.seed === "number" ? `seed ${scenario.seed}` : null,
      ]
        .filter(Boolean)
        .map((t) => el("span", t, "chip")),
    );
    $("scenario-open-json").textContent = json(scenario);
    // One's own Scenario can be made again (#376); deleting is on the home's list (#440).
    const own = Boolean(ownId) && state.project === ownId;
    $("scenario-copy").hidden = !own;
    $("scenario-actions-note").textContent = own
      ? "同じ条件で作ると、同じseedから0分の状態で始まります。シナリオの削除は、ホームの一覧から行います。"
      : "計算済みの例は、作り直せません。";
  }
  // 「同じ条件で新しいシナリオを作る」: a new Run of the open Run's Scenario and seed (POST runs).
  async function copyScenario() {
    const runId = `run-${crypto.randomUUID()}`;
    await api(`${base()}/runs`, {
      run_id: runId,
      scenario_id: state.data.scenario_id,
      scenario_version: state.data.scenario_version,
    });
    await refreshRuns();
    await openRunInPlace(runId);
  }
  // 「削除」 on the home (#440): after the person confirms with the Scenario's name
  // (POST runs/{run}/delete). Deleting the open Run closes it.
  async function deleteRun(projectId, runId, name) {
    if (
      !confirm(
        `「${name}」を削除します。計算の記録、分岐、質問の記録も消え、元に戻せません。よろしいですか？`,
      )
    )
      return;
    await api(`/api/v1/projects/${enc(projectId)}/runs/${enc(runId)}/delete`, {
      confirmation: runId,
    });
    if (state.project === projectId && state.run === runId) {
      clearTimeout(state.poll);
      state.scope++;
      state.run = null;
      state.data = null;
      state.frame = null;
      state.loadedRun = null;
      state.comparison = null;
      invalidateIntervention();
      resetChat();
      clearRunView();
      await refreshRuns();
      remember();
    }
    await loadHome();
  }
  function invalidateIntervention() {
    state.intervention = null;
    $("intervention-check").checked = false;
    $("intervention-check").disabled = true;
    $("intervention-result").replaceChildren();
    $("intervention-apply").disabled = true;
  }
  function renderKpis(container, kpis, defs = metricDefs) {
    container.replaceChildren();
    for (const d of defs) {
      const card = el("div", undefined, "kpi");
      const image = el("span", undefined, "kpi-icon");
      image.append(
        uiIcon(
          {
            stress_mean: "people",
            toilet_wait_max_minutes: "clock",
            staff_load_ratio: "staff",
            unresolved_support_request_count: "alert",
            population: "people",
            toilet_queue_length: "queue",
          }[d.key],
        ),
      );
      card.append(image);
      card.title = d.note;
      const body = el("div");
      body.append(el("small", d.label));
      body.append(
        el(
          "strong",
          num(
            typeof kpis?.[d.key] === "number" ? kpis[d.key] * d.factor : null,
          ),
        ),
        el("em", d.unit),
        el("div", d.note, "fine"),
      );
      card.append(body);
      container.append(card);
    }
  }
  // The report's line of the same indicators (#375): the cards' definitions and values, small,
  // with the minute they are of; the report's chart and summary carry the rest.
  function renderKpiStrip(container, kpis) {
    container.replaceChildren(
      el("span", kpis ? `${time(state.minute)}の指標` : "指標", "kpi-strip-when"),
      ...metricDefs.map((d) => {
        const item = el("span", undefined, "kpi-strip-item");
        item.title = d.note;
        item.append(
          el("small", d.label),
          el(
            "strong",
            num(typeof kpis?.[d.key] === "number" ? kpis[d.key] * d.factor : null),
          ),
          el("em", d.unit),
        );
        return item;
      }),
    );
  }
  function controls() {
    const has = Boolean(state.run && state.frame),
      running = state.data?.status === "running",
      working = state.busy || running;
    $("intervention-preview").disabled = !has || working;
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
    const many = multiChosen().length;
    $("multi-compare-run").disabled = many < 2 || many > 8 || working;
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
        `実計算の記録 · ${upper()}分まで計算済み`;
    }
  }
  // Whether the comparison page shows two branches; it shows the open branch alone while there
  // is no pair or a branch is being made from the simulation page's minute (#374).
  const comparing = () => Boolean(state.comparison && !state.forkFrom);
  // Where a new branch starts: the minute the simulation page chose (「この時刻から分岐を作る」),
  // or on the comparison page the shown pair's A at its minute, or the open branch and minute.
  // The minute picked in the panel (#441) replaces it while it is of the same branch.
  function forkBase() {
    if (!state.run || !state.frame) return null;
    if (state.forkFrom) return state.forkFrom;
    if (state.page === "comparison" && comparing())
      return { branch: pairIds()[0], minute: state.comparison.minute };
    return { branch: state.branch, minute: state.minute };
  }
  function interventionBase() {
    const base = forkBase(),
      pick = state.forkPick;
    if (!base) return null;
    const range = pick && pick.branch === base.branch && forkRange(pick.branch);
    return range && pick.minute >= range[0] && pick.minute <= range[1] ? pick : base;
  }
  // The minutes a branch can be made from: the base branch's computed ones.
  function forkRange(branch) {
    const ref = reference(branch);
    return ref ? [ref.fork_minute, ref.computed_until] : null;
  }
  let forkDays = "";
  function renderForkPicker(working) {
    const from = interventionBase(),
      range = from && forkRange(from.branch),
      day = $("fork-day"),
      clock = $("fork-clock");
    day.disabled = clock.disabled = !range || working;
    if (!range) {
      $("intervention-base").textContent = "シナリオを開くと使えます";
      $("fork-range").textContent = "";
      return;
    }
    const [lo, hi] = range,
      first = Math.floor(lo / 1440) + 1,
      last = Math.floor(hi / 1440) + 1,
      key = `${first}-${last}`;
    if (key !== forkDays) {
      forkDays = key;
      day.replaceChildren(
        ...Array.from({ length: last - first + 1 }, (_, i) => {
          const option = el("option", `${first + i}日目`);
          option.value = String(first + i);
          return option;
        }),
      );
    }
    day.value = String(Math.floor(from.minute / 1440) + 1);
    if (document.activeElement !== clock)
      clock.value = time(from.minute).split(" ")[1];
    $("intervention-base").textContent = `元：${shortBranch(from.branch)}`;
    $("fork-range").textContent = `${time(lo)}〜${time(hi)}の間で選べます`;
  }
  function pickForkTime() {
    const base = forkBase(),
      range = base && forkRange(base.branch);
    if (!range) return;
    const [hours, minutes] = $("fork-clock").value.split(":").map(Number),
      minute = (Number($("fork-day").value) - 1) * 1440 + hours * 60 + minutes;
    if (!Number.isInteger(minute) || minute < range[0] || minute > range[1]) {
      note(`分岐できるのは、計算済みの${time(range[0])}〜${time(range[1])}です。`, "error");
      controls();
      return;
    }
    state.forkPick = { branch: base.branch, minute };
    invalidateIntervention();
    controls();
    // Without a pair on show, the open branch is shown at the picked minute.
    if (!comparing() && base.branch === state.branch && minute !== state.minute) {
      if (state.forkFrom) {
        state.forkFrom = { ...state.forkFrom, minute };
        $("comparison-instruction").textContent =
          `${time(minute)}から分岐を作ります。左の欄に介入の指示を書いてください。`;
      }
      // Not an action: a later pick replaces an earlier one still loading (seek keeps the last).
      loading(() => seek(minute)).catch((e) => note(e.message, "error"));
    }
  }
  async function ensureProject(id = state.project) {
    const list = await api("/api/v1/projects");
    if (!list.projects.some((p) => p.project_id === id))
      await api("/api/v1/projects", { project_id: id });
  }
  // The open Run's name in the header, the one place it is shown (#442): as the home names it,
  // one's own told apart from Runs of the same conditions. Nothing is shown while no Run is open.
  function showOpenName(name) {
    $("scenario-name").textContent = name;
    $("scenario-name").title = name;
    $("header-scenario").hidden = !name;
  }
  async function refreshRuns() {
    const project = await api(base());
    const names = runNames(project.runs, project.scenarios, {
      example: !(ownId && state.project === ownId),
    });
    showOpenName((state.run && names.get(state.run)) || "");
  }
  function scenarioRequest() {
    const req = {},
      origins = {};
    for (const [id, key, type] of [
      ["municipality", "municipality", "string"],
      ["disaster", "disaster", "string"],
      ["evacuees", "evacuees", "number"],
      ["duration", "duration_days", "number"],
      ["staff-count", "staff_count", "number"],
      ["area", "gross_area_m2", "number"],
      ["toilets", "accessible_toilet_count", "number"],
    ]) {
      const value = $(id).value.trim();
      if (value !== "") {
        req[key] = type === "number" ? Number(value) : value;
        origins[key] = "user";
      }
    }
    if ($("facility-type").value) {
      req.facility_conditions = [
        `施設種別（利用者入力）: ${$("facility-type").value}`,
      ];
      origins.facility_conditions = "user";
    }
    if ($("food").value !== "" || $("water").value !== "") {
      req.operational_conditions = {
        initial_resources: {
          status:
            $("food").value !== "" && $("water").value !== ""
              ? "entered"
              : "incomplete",
          food_meal_units:
            $("food").value === "" ? null : Number($("food").value),
          water_ml:
            $("water").value === ""
              ? null
              : Math.round(Number($("water").value) * 1000),
          source_kind: "operator",
        },
      };
      origins.operational_conditions = "user";
    }
    req.field_origins = origins;
    return req;
  }
  async function previewScenario() {
    if (!$("scenario-form").reportValidity()) return;
    const version = state.inputVersion;
    let req = scenarioRequest();
    const text = $("scenario-text").value.trim();
    const hasFields = Object.keys(req).some((k) => k !== "field_origins");
    if (
      hasFields &&
      (!req.municipality || !req.disaster || !req.evacuees) &&
      !text
    )
      throw new Error(
        "自治体・災害種別・避難者数を項目か文章で指定してください。",
      );
    if (!hasFields && !text)
      throw new Error("自然言語または基本設定を入力してください。");
    await ensureProject(ownProject());
    if (hasFields && (!req.municipality || !req.disaster || !req.evacuees)) {
      const hints = [
        req.municipality ? `${req.municipality}で` : null,
        req.disaster,
        req.evacuees ? `${req.evacuees}人` : null,
        req.duration_days ? `${req.duration_days}日間` : null,
      ]
        .filter(Boolean)
        .join("、");
      const extracted = await api(`${ownBase()}/scenario-preview`, {
        text: `${text}\n入力欄の指定: ${hints}`,
      });
      if (version !== state.inputVersion)
        throw new Error("入力が変更されました。再プレビューしてください。");
      if (!extracted.request)
        throw new Error(
          `不足項目：${(extracted.missing_fields || []).join("、")}。基本設定を確認してください。`,
        );
      req = {
        ...extracted.request,
        ...req,
        field_origins: {
          ...extracted.request.field_origins,
          ...req.field_origins,
        },
      };
    }
    const result = await api(`${ownBase()}/scenario-preview`, {
      text,
      ...(hasFields ? { request: req } : {}),
    });
    if (version !== state.inputVersion)
      throw new Error(
        "入力が変更されました。最新の条件で再プレビューしてください。",
      );
    if (!result.scenario) {
      $("blockers").replaceChildren(
        el(
          "p",
          `不足項目：${(result.missing_fields || []).join("、")}。基本設定を揃えて再プレビューしてください。`,
          "warning-item",
        ),
      );
      throw new Error(
        "必要な条件を読み取れませんでした。自治体・災害種別・避難者数を入力してください。",
      );
    }
    state.preview = result;
    renderPreview();
  }
  function sourceRows(box, rows) {
    box.replaceChildren();
    for (const row of rows || []) {
      const card = el("div", undefined, "source-row");
      card.append(el("strong", row.title || row.subject || row.id || "出典"));
      card.append(
        el(
          "span",
          `${row.source_kind || row.classification || "仮定・適用範囲は原文参照"} · ${row.reason || row.description || row.source_id || ""}`,
        ),
      );
      if (row.url) {
        try {
          const url = new URL(row.url);
          if (url.protocol === "https:" || url.protocol === "http:") {
            const a = el("a", " 出典を開く ↗");
            a.href = url.href;
            a.target = "_blank";
            a.rel = "noopener noreferrer";
            card.append(a);
          }
        } catch {
          /* Preserve as text only. */
        }
      }
      if (row.value !== undefined) card.append(el("pre", json(row.value)));
      box.append(card);
    }
  }
  function renderPreview() {
    const p = state.preview;
    if (!p) return;
    const r = p.request,
      s = p.scenario,
      f = p.facility_design;
    $("preview-result").hidden = false;
    $("preview-empty").hidden = true;
    $("preview-step").classList.add("current");
    // With the API on (#421), whether the AI read the text or the text was read without it.
    const usage = p.llm_usage,
      source = s?.provenance?.scenario_input?.extraction_source,
      read =
        usage?.mode !== "record"
          ? null
          : source === "prepare_llm"
            ? `AIが文を読み取りました（約$${Number(usage.spent_usd).toFixed(3)}）`
            : "AIを使えず、文に明示された値だけで読み取りました";
    $("preview-mode").textContent = [llmModeLabel(usage?.mode), read, "補完値を要確認"]
      .filter(Boolean)
      .join(" · ");
    $("preview-summary").textContent =
      `${r.municipality}での${disasterName(r.disaster)}を想定し、${r.evacuees}人・${num(r.duration_days)}日間の避難所運営を試算します。施設・人口は合成モデルです。`;
    $("condition-chips").replaceChildren(
      ...[
        r.municipality,
        disasterName(r.disaster),
        `${r.evacuees}人`,
        `${num(r.duration_days)}日間`,
      ].map((t) => el("span", t, "chip")),
    );
    $("facility-summary").replaceChildren(
      ...[
        ["避難スペース容量", `${f?.capacity_supported ?? "—"}人`],
        ["スタッフ", `${s.staff?.length ?? "—"}人`],
      ].map(([label, value]) => {
        const d = el("div", label);
        d.append(el("strong", value));
        return d;
      }),
    );
    $("scale-summary").replaceChildren(
      ...[
        ["想定避難者数", `${r.evacuees}人`],
        ["想定期間", `${num(r.duration_days)}日間`],
      ].map(([label, value]) => {
        const d = el("div", label);
        d.append(el("strong", value));
        return d;
      }),
    );
    $("input-origins").replaceChildren();
    for (const [key, label] of [
      ["municipality", "自治体"],
      ["disaster", "災害"],
      ["evacuees", "人数"],
      ["duration_days", "期間（日）"],
      ["gross_area_m2", "延床面積"],
      ["staff_count", "スタッフ数"],
      ["accessible_toilet_count", "車いす対応トイレ"],
    ]) {
      const row = el("div", undefined, "origin-row");
      row.append(
        el(
          "span",
          `${label}：${r[key] ?? (key === "staff_count" ? `${s.staff.length}人（生成）` : key === "gross_area_m2" ? `${num(f.geometry.width_m * f.geometry.height_m)} m²（生成）` : "未指定（生成仮定を参照）")}`,
        ),
        el(
          "span",
          r.field_origins?.[key] === "user"
            ? "入力値"
            : r.field_origins?.[key] === "llm_extracted"
              ? "文章から抽出"
              : "補完・仮定",
          "badge",
        ),
      );
      $("input-origins").append(row);
    }
    sourceRows($("preview-sources"), [
      ...(p.research?.sources || []),
      ...(s.provenance?.sources || []),
      ...(s.provenance?.assumptions || []),
      ...(f?.assumptions || []),
    ]);
    $("scenario-json").textContent = json(s);
    $("warnings").replaceChildren(
      ...(p.warnings || []).map((t) => el("p", t, "warning-item")),
    );
    $("blockers").replaceChildren(
      ...(p.approval_blockers?.length
        ? p.approval_blockers.map((t) => el("p", t, "warning-item"))
        : [
            el(
              "p",
              "承認ブロッカーはありません。補完値と警告を確認してください。",
              "quiet-message",
            ),
          ]),
    );
    $("scenario-check").checked = false;
    $("scenario-check").disabled = !p.can_start;
    state.mode = p.llm_usage?.mode ?? "未記録";
    drawMap(
      $("preview-map"),
      {
        geometry: f?.geometry,
        zones: f?.zones,
        agents: [],
        people: [],
        facilities: (f?.facilities || []).map((x) => ({
          ...x,
          point: f.geometry.facility_points[x.id],
        })),
        staff: [],
      },
      "normal",
      1,
    );
    drawPopulation(s);
    controls();
  }
  async function approveScenario() {
    const p = state.preview;
    if (!p?.can_start || !$("scenario-check").checked)
      throw new Error("確認と明示承認が必要です。");
    const runId = `run-${crypto.randomUUID()}`;
    await api(`${ownBase()}/scenario-approve`, {
      preview_id: p.preview_id,
      approved_hash: p.approved_hash,
      approved: true,
      run_id: runId,
    });
    clearTimeout(state.poll);
    state.project = ownProject();
    state.loadedRun = null;
    state.data = null;
    state.frame = null;
    state.run = runId;
    state.branch = "main";
    state.minute = 0;
    state.scope++;
    state.history = [];
    state.comparison = null;
    state.context = null;
    state.selected = null;
    state.selectedGroup = [];
    state.mapPan = { x: 0, y: 0 };
    state.zoom = 1;
    invalidateIntervention();
    state.preview = null;
    $("scenario-check").checked = false;
    $("scenario-check").disabled = true;
    $("start-step").classList.add("done");
    await refreshRuns();
    await refreshRun();
    show("simulation");
  }
  async function refreshRun() {
    const scope = state.scope,
      previousHead = reference()?.checkpoint_hash,
      data = await api(runBase());
    if (scope !== state.scope) return;
    if (state.loadedRun !== state.run) {
      const project = await api(base());
      if (scope !== state.scope) return;
      state.scenario = project.scenarios.find(
        (s) =>
          s.scenario_id === data.scenario_id &&
          s.scenario_version === data.scenario_version,
      )?.scenario;
      state.total = state.scenario?.duration_minutes;
      state.loadedRun = state.run;
    }
    state.data = data;
    const branches = data.branches || [];
    if (!branches.some((b) => b.branch_id === state.branch))
      state.branch = branches[0]?.branch_id || "main";
    $("branch-select").replaceChildren(
      ...branches.map((b) => {
        const o = el("option", shortBranch(b.branch_id));
        o.value = b.branch_id;
        o.selected = b.branch_id === state.branch;
        return o;
      }),
    );
    const previous = $("compare-branch").value;
    $("compare-branch").replaceChildren(
      ...branches
        .filter((b) => b.parent_branch_id || b.branch_id !== "main")
        .map((b) => {
          const o = el("option", shortBranch(b.branch_id));
          o.value = b.branch_id;
          o.selected =
            b.branch_id === previous ||
            (!previous && b.branch_id === state.branch);
          return o;
        }),
    );
    if (!$("compare-branch").value && branches.length > 1)
      $("compare-branch").value = branches[1].branch_id;
    state.minute = Math.max(lower(), Math.min(upper(), state.minute));
    // A saved boundary is immutable. Poll status cheaply while its checkpoint
    // and displayed target stay unchanged; full reconstruction can be expensive.
    // While the branch computes, the page is dimmed (#482) and its minute does not follow
    // the computed head; the frame is read again once the computation ends.
    if (
      !state.frame ||
      state.frame.run_id !== state.run ||
      state.frame.branch_id !== state.branch ||
      (data.status !== "running" &&
        (state.frame.minute !== state.minute ||
          previousHead !== reference()?.checkpoint_hash))
    ) await seek(state.minute);
    remember();
    controls();
    if (data.status === "running") {
      clearTimeout(state.poll);
      state.poll = setTimeout(() => {
        refreshRun().catch((e) => note(e.message, "error"));
      }, 1200);
    }
  }
  async function seek(minute, expectedPlayToken) {
    if (!Number.isInteger(minute) || minute < lower() || minute > upper())
      throw new Error("計算済み時刻だけを表示できます。");
    const serial = ++state.seekVersion,
      scope = state.scope,
      branch = state.branch;
    const frame = await api(`${branchBase()}/timeline?minute=${minute}`);
    if (
      (expectedPlayToken !== undefined &&
        expectedPlayToken !== state.playToken) ||
      serial !== state.seekVersion ||
      scope !== state.scope ||
      branch !== state.branch
    )
      return;
    if (state.minute !== minute) {
      invalidateIntervention();
      state.context = null;
    }
    state.frame = frame;
    state.minute = frame.minute;
    renderFrame();
  }
  // A Run of an older model is view-only and shows saved points only (D2).
  function savedPointNote(frame) {
    return [frame.model_view?.message, frame.saved_point_rounding?.message]
      .filter(Boolean)
      .join(" ");
  }
  function renderFrame() {
    const frame = state.frame;
    if (!frame) return;
    if (
      state.sceneMode === "schematic" &&
      frame.geometry &&
      !window.HinanjoScene.supported(frame)
    ) {
      state.sceneMode = "physical";
      document
        .querySelectorAll("[data-scene-mode]")
        .forEach((s) => (s.value = "physical"));
      document
        .querySelectorAll(".map-caption")
        .forEach(
          (n) => (n.textContent = "実座標 · この施設形状は模式図の対象外です"),
        );
    }
    renderKpis($("sim-kpis"), frame.kpis);
    renderKpiStrip($("report-kpis"), frame.kpis);
    const geometryOrigin = {
      synthetic: "合成施設",
      measured: "実測形状",
      operator: "入力された形状",
      inferred: "推定された形状",
    };
    $("map-location").textContent = frame.geometry
      ? `${geometryOrigin[frame.geometry.source_kind] || "施設"} / 記録された形状`
      : "物理座標なし";
    const missing = frame.agents.filter(
      (a) => !a.point || a.position_kind === "display_only",
    ).length;
    $("position-note").textContent =
      state.sceneMode === "schematic" && window.HinanjoScene.supported(frame)
        ? "模式表示：個別の配置は所属を表す記号です。距離・動線は実寸ではありません。"
        : `実座標 ${frame.agents.length - missing}/${frame.agents.length}人記録 · 同じ位置は集合表示 · ドラッグで移動`;
    const issues = [];
    // Shown first and kept while the frame is shown, unlike a notice.
    const viewNote = savedPointNote(frame);
    if (viewNote) issues.push(["閲覧のみのシナリオ", viewNote]);
    if (frame.kpis.toilet_queue_length > 0)
      issues.push([
        "トイレの待機列",
        `${frame.kpis.toilet_queue_length}人が待機。累積最大待機時間 ${num(frame.kpis.toilet_wait_max_minutes)}分。`,
      ]);
    if (frame.kpis.unresolved_support_request_count > 0)
      issues.push([
        "未解決の支援要請",
        `${frame.kpis.unresolved_support_request_count}件がスタッフの対応待ちです。対応が完了すると下の「イベントタイムライン」に青で、対応されないまま時間切れになると赤で表示されます。`,
      ]);
    for (const f of frame.facilities.filter(
      (f) =>
        typeof f.working_units === "number" && f.working_units < f.capacity,
    ))
      issues.push([
        `${f.id} の稼働制限`,
        `稼働 ${f.working_units}/${f.capacity}`,
      ]);
    if (frame.kpis.staff_load_ratio >= 0.8)
      issues.push([
        "スタッフ負荷が高い状態",
        `稼働比率 ${num(frame.kpis.staff_load_ratio * 100)}%`,
      ]);
    $("issues").replaceChildren(
      ...(issues.length
        ? issues.slice(0, 5).map(([a, b]) => {
            const n = el("div", undefined, "issue");
            n.append(el("strong", a), el("span", b));
            return n;
          })
        : [
            el(
              "p",
              "この時点では表示対象の問題は記録されていません。",
              "quiet-message",
            ),
          ]),
    );
    renderEvents($("events"), frame.events, true);
    renderPeople();
    controls();
    drawAll();
  }
  function renderEvents(box, events, clickable = false) {
    box.replaceChildren();
    if (!events?.length) {
      box.append(el("p", "この取得範囲にイベント記録はありません。", "fine"));
      return;
    }
    const grouped = new Map();
    for (const event of [...events].slice(-100)) {
      const key = `${event.minute}:${event.event_type}:${event.status}`;
      const item = grouped.get(key);
      if (item) item.count++;
      else grouped.set(key, { ...event, count: 1 });
    }
    for (const e of [...grouped.values()].slice(-12)) {
      const node = el(
        clickable ? "button" : "div",
        undefined,
        `event-item event-${eventTone(e)}`,
      );
      node.append(
        el(
          "b",
          `${eventNames[e.event_type] || e.event_type}${e.count > 1 ? ` ×${e.count}件` : ""}`,
        ),
        el(
          "span",
          `${time(e.minute)} · ${{ applied: "適用済み", suppressed: "適用抑制", rejected: "却下", failed: "失敗", scheduled: "予定" }[e.status] || e.status}`,
        ),
      );
      node.title = `${e.event_id} / source: ${e.source_id || "未記録"}`;
      if (clickable) {
        node.disabled = e.minute < lower() || e.minute > upper();
        node.addEventListener("click", () =>
          action("イベントの時刻を取得しています…", () => seek(e.minute)),
        );
      }
      box.append(node);
    }
    // 欄の高さは固定で、記録は古い順に並ぶ。最新の記録が見えるよう末尾へ送る（#377）
    box.scrollTop = box.scrollHeight;
  }

  function personSummary(p) {
    return `${typeof p.traits?.age_years === "number" ? p.traits.age_years + "歳" : "年齢未記録"} · ${activityNames[p.activity] || p.activity} · ストレス ${num(p.stress * 100)}/100`;
  }
  function renderPeople() {
    const f = state.frame;
    $("people-list").replaceChildren();
    // Everyone, in the panel's height (#496); a group chosen on the map, alone.
    const group = state.selectedGroup?.length
      ? f.people.filter((p) => state.selectedGroup.includes(p.id))
      : f.people;
    if (state.selectedGroup?.length > 1)
      $("people-list").append(
        el("span", `同じ記録位置の${group.length}人`, "group-caption"),
      );
    for (const p of group) {
      const button = el("button", p.id, "person-chip");
      button.classList.toggle("selected", p.id === state.selected);
      button.addEventListener("click", () =>
        selectPerson(p.id, state.selectedGroup, anchorOf(button)),
      );
      $("people-list").append(button);
    }
    if (state.selected) {
      const p = f.people.find((p) => p.id === state.selected);
      const staff = f.staff.find((s) => s.id === state.selected);
      const item = p || staff;
      $("person-detail").replaceChildren();
      if (item) {
        const box = el("div", undefined, "person-state");
        box.append(
          el("strong", `♟ ${item.id} ${staff ? "（スタッフ）" : ""}`),
          el(
            "p",
            p
              ? personSummary(p)
              : `担当 ${staff.active_task_id || "なし"} / ${staff.zone_id}`,
          ),
        );
        const detail = el("details");
        detail.append(
          el("summary", "記録された状態の詳細"),
          el("pre", json(item)),
        );
        box.append(detail);
        $("person-detail").append(box);
      } else
        $("person-detail").append(
          el("p", "選択した人物はこの時点の記録に含まれません。", "fine"),
        );
    } else {
      $("person-detail").replaceChildren();
      for (const p of [...f.people]
        .sort((a, b) => (b.stress || 0) - (a.stress || 0))
        .slice(0, 2)) {
        const card = el("button", undefined, "person-preview");
        const avatar = el("span", undefined, "avatar");
        avatar.append(uiIcon("people"));
        const body = el("span");
        body.append(el("strong", p.id), el("small", personSummary(p)));
        card.append(avatar, body);
        card.addEventListener("click", () =>
          selectPerson(p.id, [], anchorOf(card)),
        );
        $("person-detail").append(card);
      }
    }
  }
  function selectPerson(id, group = [], anchor = null) {
    state.selected = id;
    state.selectedGroup = group;
    renderPeople();
    drawAll();
    openPersonDialog(id, anchor);
  }
  // クリックした要素の位置を、吹き出しの基準点にする
  const anchorOf = (node) => {
    const r = node.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.bottom };
  };
  const mobilityNames = {
    independent: "自力で移動できる",
    assisted: "介助があれば移動できる",
    wheelchair: "車いすで移動",
    bedridden: "寝たきり",
  };
  // 0〜1 の値を、ラベル付きの横棒で表示する（high=true は値が大きいほど悪い指標）
  function meter(label, value, high = true) {
    const row = el("div", undefined, "pd-meter");
    if (typeof value !== "number" || !Number.isFinite(value)) {
      row.append(el("span", label), el("em", "未記録"));
      return row;
    }
    const pct = Math.max(0, Math.min(100, value * 100));
    const bad = high ? pct : 100 - pct;
    const bar = el("span", undefined, "pd-bar");
    const fill = el(
      "i",
      undefined,
      bad >= 66 ? "pd-bad" : bad >= 33 ? "pd-mid" : "pd-good",
    );
    fill.style.width = `${pct}%`;
    bar.append(fill);
    row.append(el("span", label), bar, el("em", `${num(pct)}`));
    return row;
  }
  function pdSection(title, ...children) {
    const s = el("section", undefined, "pd-section");
    s.append(el("h3", title), ...children);
    return s;
  }
  function pdFacts(pairs) {
    const dl = el("dl", undefined, "pd-facts");
    for (const [k, v] of pairs) dl.append(el("dt", k), el("dd", v ?? "未記録"));
    return dl;
  }
  function openPersonDialog(id, anchor = null) {
    const f = state.frame,
      dialog = $("person-dialog");
    if (!f || !dialog) return;
    const p = f.people.find((x) => x.id === id),
      staff = f.staff.find((x) => x.id === id),
      body = $("person-dialog-body");
    body.replaceChildren();
    if (p) {
      $("person-dialog-title").textContent = `避難者 ${p.id}`;
      body.append(
        el("p", `${time(state.minute)} 時点の記録`, "fine"),
        pdFacts([
          ["年齢", typeof p.traits?.age_years === "number" ? `${p.traits.age_years}歳` : null],
          ["いる場所", zoneNames[p.zone_id] || p.zone_id],
          ["いまの行動", activityNames[p.activity] || p.activity],
          ["移動", mobilityNames[p.mobility] || p.mobility],
          ["世帯", `${p.household_id || "—"}${p.traits?.household_role === "alone" ? "（単身）" : ""}`],
          ["言語", (p.traits?.languages || []).join("、") || null],
          ["配慮が必要なこと", (p.traits?.accessibility_needs || []).join("、") || "なし"],
          ["家族と離れている", p.family_separated ? "はい" : "いいえ"],
        ]),
        pdSection(
          "こころの状態",
          meter("ストレス", p.stress),
          meter("安心感", p.body?.reassurance, false),
          meter("不安", p.body?.uncertainty),
          meter("不満", p.body?.discontent),
          meter("前向きな気持ち", p.body?.positive_affect, false),
          meter("落ち込み", p.body?.negative_affect),
        ),
        pdSection(
          "からだの状態・ニーズ",
          meter("支援の必要度", p.support_need),
          meter("空腹", p.needs?.hunger),
          meter("のどの渇き", p.needs?.hydration_need),
          meter("トイレに行きたい", p.needs?.toilet_urgency),
          meter("疲れ", p.needs?.fatigue),
          meter("衛生面の不快", p.body?.hygiene_need),
          pdFacts([
            [
              "睡眠不足",
              typeof p.body?.sleep_debt_minutes === "number"
                ? `${num(p.body.sleep_debt_minutes / 60)}時間分`
                : null,
            ],
          ]),
        ),
      );
    } else if (staff) {
      $("person-dialog-title").textContent = `スタッフ ${staff.id}`;
      body.append(
        el("p", `${time(state.minute)} 時点の記録`, "fine"),
        pdFacts([
          ["いる場所", zoneNames[staff.zone_id] || staff.zone_id],
          ["担当中の対応", staff.active_task_id || "なし"],
          ["連続勤務", `${staff.dynamics?.consecutive_work_minutes ?? "—"}分`],
          ["できること", (staff.skills || []).join("、") || null],
        ]),
        pdSection("負担", meter("疲れ", staff.dynamics?.fatigue)),
      );
    } else {
      $("person-dialog-title").textContent = id;
      body.append(el("p", "この人物はこの時点の記録に含まれません。", "fine"));
    }
    const raw = el("details");
    raw.append(el("summary", "記録データ（開発者向け）"), el("pre", json(p || staff || {})));
    body.append(raw);
    // パネルの overflow に切られないよう、body 直下に置く
    if (dialog.parentElement !== document.body) document.body.append(dialog);
    if (!dialog.open) dialog.show();
    placeBubble(dialog, anchor);
  }
  // 吹き出しをクリック位置の近くに置き、画面からはみ出さないように調整する
  function placeBubble(dialog, anchor) {
    const w = dialog.offsetWidth,
      h = dialog.offsetHeight,
      pad = 12,
      at = anchor || { x: innerWidth / 2, y: innerHeight / 3 };
    let left = at.x - w / 2,
      top = at.y + 14,
      below = true;
    if (top + h > innerHeight - pad) {
      top = at.y - h - 14;
      below = false;
    }
    left = Math.max(pad, Math.min(innerWidth - w - pad, left));
    top = Math.max(pad, Math.min(innerHeight - h - pad, top));
    dialog.style.left = `${left}px`;
    dialog.style.top = `${top}px`;
    dialog.style.setProperty(
      "--tail-x",
      `${Math.max(16, Math.min(w - 16, at.x - left))}px`,
    );
    dialog.classList.toggle("tail-top", below);
    dialog.classList.toggle("tail-bottom", !below);
  }
  function replayTime(minute) {
    const seconds = Math.floor(minute * 60 + 0.00001),
      m = Math.floor(seconds / 60);
    return time(m) + ":" + String(seconds % 60).padStart(2, "0");
  }
  function motionFraction() {
    return state.motion?.fraction || 0;
  }
  async function togglePlayback() {
    if (state.playing) {
      stopPlayback(true);
      return;
    }
    const comparing = state.page === "comparison" && Boolean(state.comparison) && !state.forkFrom;
    const hi = comparing ? state.comparison.common_range[1] : upper();
    let current = comparing ? state.comparison.minute : state.minute;
    if (current >= hi) {
      state.replay?.dispose();
      state.replay = null;
      if (comparing) await loadComparison(state.comparison.common_range[0]);
      else await seek(lower());
      current = comparing ? state.comparison.minute : state.minute;
    }
    if (!state.replay) {
      const scope = state.scope,
        root = runBase(),
        branch = branchBase(),
        ids = comparing ? pairIds() : [state.branch];
      state.replay = new window.HinanjoReplayPlayer({
        minute: current,
        hi,
        rate: Number($("speed").value),
        payload: comparing ? state.comparison : state.frame,
        mode: () => state.sceneMode,
        fetch: (minute) =>
          comparing
            ? api(
                `${root}/compare?branch_ids=${enc(ids.join(","))}&minute=${minute}`,
              )
            : api(`${branch}/timeline?minute=${minute}`),
        fetchRange: (start, end) =>
          api(
            `${root}/replay-frames?branch_ids=${enc(ids.join(","))}&start=${start}&end=${end}`,
          ).then((r) => r.samples),
        onFrame: (payload, minute) => {
          if (scope !== state.scope) return;
          state.motion = null;
          if (comparing) void loadComparison(minute, undefined, payload);
          else {
            state.frame = payload;
            state.minute = minute;
            invalidateIntervention();
            state.context = null;
            renderFrame();
          }
        },
        onDraw: (player) => {
          state.motion = player.motion;
          state.visualMs = player.visualMs;
          window.HinanjoReplayView = {
            minute: player.clock.position,
            rate: player.clock.rate,
            bufferedUntil: player.head,
            fraction: player.motion?.fraction || 0,
            from: player.motion?.from.map((f) => f.minute) || [],
            to: player.motion?.to.map((f) => f.minute) || [],
            playing: state.playing,
            comparison: comparing,
          };
          drawAll();
          // The time shows under the map, by the time bar (#484).
          $(comparing ? "compare-time" : "seek-label").textContent = replayTime(
            player.clock.position,
          );
        },
        onState: (status) => {
          if (status === "paused" || status === "end") {
            state.playing = false;
            if (window.HinanjoReplayView)
              window.HinanjoReplayView = {
                ...window.HinanjoReplayView,
                playing: false,
              };
            setPlayLabels();
          }
        },
        onError: (e) => note(e.message, "error"),
        onStats: (stats) => {
          window.HinanjoPlayback = { ...stats, comparison: comparing };
        },
      });
    }
    state.playing = true;
    state.replay.setRate(Number($("speed").value));
    setPlayLabels();
    state.replay.play();
  }
  // 「計算する」 (#441): each branch of `ids` to the end, one after the other (the server and the
  // browser compute one at a time). The page stays usable, so 「中止」 can stop it; a cancelled
  // computation keeps what it computed, and 「続きから計算する」 goes on from there.
  async function computeToEnd(ids) {
    if (state.computing) return;
    state.computing = true;
    state.cancelled = false;
    stopPlayback();
    controls();
    try {
      for (const id of ids) {
        if (reference(id).computed_until >= duration()) continue;
        await api(`${runBase()}/resume`, {
          branch_id: id,
          target_minute: duration(),
        });
        await refreshRun();
        await followComputing(id);
      }
      await refreshRun();
      if (state.page === "comparison" && !state.forkFrom && pairIds().length > 1)
        await loadComparison();
    } catch (error) {
      // A cancel needs no notice (#442): the button now says 「続きから計算する」.
      if (!state.cancelled) note(`計算できませんでした：${error.message}`, "error");
      await refreshRun().catch(() => {});
    } finally {
      state.computing = false;
      controls();
    }
  }
  // Waits for the computation of `branch` to end, reading the Run as the screen does (refreshRun),
  // so the header's button shows how far it is.
  async function followComputing(branch) {
    const scope = state.scope;
    for (;;) {
      await new Promise((resolve) => setTimeout(resolve, 900));
      if (scope !== state.scope) throw new Error("表示するシナリオが変更されました。");
      await refreshRun();
      if (state.data?.status === "running") continue;
      if (reference(branch).computed_until < duration())
        throw new Error(`計算は「${runStatusLabel(state.data?.status)}」で止まりました。`);
      return;
    }
  }
  async function cancelCompute() {
    state.cancelled = true;
    try {
      await api(`${runBase()}/cancel`, {});
      await refreshRun();
    } catch (error) {
      note(error.message, "error");
    }
  }
  async function waitForked(child) {
    // Other branches may compute meanwhile, so the fork has ended when its child is
    // listed and does not compute, or the Run reports the fork failed (as /demo, #48).
    for (;;) {
      const scope = state.scope,
        data = await api(runBase());
      if (scope !== state.scope)
        throw new Error("表示するシナリオが変更されました。");
      state.data = data;
      const created = data.branches.find((b) => b.branch_id === child),
        computing = created?.status
          ? created.status === "running"
          : data.status === "running";
      if (created && !computing) return;
      if (
        data.last_fork?.status === "failed" ||
        (!created && data.status !== "running")
      )
        throw new Error(
          `分岐を作成できませんでした: ${data.last_fork?.error || "理由を取得できませんでした"}`,
        );
      await new Promise((resolve) => setTimeout(resolve, 900));
    }
  }
  async function previewIntervention() {
    if (state.data.status === "running")
      throw new Error("計算を停止してから介入してください。");
    const text = $("intervention-text").value.trim();
    if (!text) throw new Error("介入内容を入力してください。");
    const from = interventionBase();
    if (!from) throw new Error("シナリオを開いてから介入してください。");
    const captured = {
      text,
      minute: from.minute,
      branch: from.branch,
      scope: state.scope,
    };
    const result = await api(`${branchBase(from.branch)}/intervention-preview`, {
      text,
      minute: from.minute,
    });
    const now = interventionBase();
    if (
      text !== $("intervention-text").value.trim() ||
      captured.minute !== now?.minute ||
      captured.branch !== now?.branch ||
      captured.scope !== state.scope
    )
      return;
    state.intervention = { ...result, captured };
    $("intervention-result").replaceChildren();
    const box = el("div", undefined, "subcard");
    box.append(
      el(
        "strong",
        result.can_apply
          ? "検証済み · 承認すると独立した分岐を作成"
          : "適用できません · 条件の修正が必要",
      ),
      el(
        "p",
        result.explanation || result.mechanism_summary || result.status,
        "fine",
      ),
    );
    box.append(
      el(
        "span",
        `${supportModeName(result.support_mode)} / 代替処理 ${result.fallback_used === true ? "あり" : result.fallback_used === false ? "なし" : "未記録"} / ${time(captured.minute)}から`,
        "badge",
      ),
    );
    for (const question of result.questions || [])
      box.append(el("p", question, "fine"));
    if (result.status === "clarification_required" && result.choices?.length) {
      const choices = el("div", undefined, "chips");
      for (const choice of result.choices) {
        const button = el(
          "button",
          `${zoneNames[choice] || choice} (${choice})`,
          "compact",
        );
        button.addEventListener("click", () =>
          action("選択した対象で作用を再プレビューしています…", async () => {
            $("intervention-text").value =
              `${text.replace(/\nzone_id: [^\n]+/g, "")}\nzone_id: ${choice}`;
            invalidateIntervention();
            await previewIntervention();
          }),
        );
        choices.append(button);
      }
      box.append(choices);
    }
    for (const issue of [
      ...(result.validation_issues || []),
      ...(result.limitations || []),
    ])
      box.append(
        el("p", typeof issue === "string" ? issue : json(issue), "fine"),
      );
    const d = el("details");
    d.append(
      el("summary", "変換された作用・対象・出典を確認"),
      el("pre", json(result.program || result)),
    );
    box.append(d);
    $("intervention-result").append(box);
    $("intervention-check").checked = false;
    $("intervention-check").disabled = !result.can_apply;
    controls();
  }
  async function applyIntervention() {
    const i = state.intervention,
      from = interventionBase();
    if (
      !i?.can_apply ||
      !$("intervention-check").checked ||
      i.captured.text !== $("intervention-text").value.trim() ||
      i.captured.minute !== from?.minute ||
      i.captured.branch !== from?.branch
    )
      throw new Error(
        "現在の条件で再プレビューし、作用を確認して承認してください。",
      );
    const child = `branch-${crypto.randomUUID()}`;
    const parent = i.captured.branch;
    await api(`${branchBase(parent)}/intervention-apply`, {
      preview_id: i.preview_id,
      approved_program_hash: i.approved_program_hash,
      approved: true,
      child_id: child,
    });
    // The server forks in its own process: the child is listed once that ends.
    await waitForked(child);
    state.branch = child;
    state.minute = i.captured.minute;
    state.forkFrom = null;
    state.scope++;
    state.comparison = null;
    state.history = [];
    state.context = null;
    resetChat();
    $("comparison-instruction").textContent =
      `${time(i.captured.minute)}に分岐：${i.captured.text}`;
    $("comparison-effect").replaceChildren(
      el("span", "作用検証を通過・明示承認済み", "badge teal-badge"),
      effectSummary(i.program),
    );
    sessionStorage.setItem(
      `hinanjo-if-intervention-${child}`,
      json({
        parent,
        text: i.captured.text,
        minute: i.captured.minute,
        program: i.program,
      }),
    );
    state.forkPick = null;
    invalidateIntervention();
    await refreshRun();
    $("compare-branch").value = child;
    await loadComparison();
    show("comparison");
    // The new branch is computed to the end at once (#441), after this action has ended so
    // 「中止」 can be pressed meanwhile; the comparison follows when it ends.
    setTimeout(() => void computeToEnd([child]), 0);
  }
  function effectSummary(program) {
    const box = el("div"),
      names = {
        add_toilet: "トイレを追加",
        add_staff: "スタッフを追加",
        facility_outage: "設備の稼働を停止",
        staff_task_order: "スタッフへ対応を指示",
        resource_delivery: "物資を搬入",
      };
    for (const e of program?.effects || []) {
      const target = e.target_selector?.target_id,
        amount = e.parameters?.count ?? e.parameters?.units;
      box.append(
        el(
          "p",
          `${names[e.op] || e.op}${typeof amount === "number" ? `（${amount}）` : ""} · 対象 ${zoneNames[target] || target || "作用詳細を参照"} · ${time(e.start_minute)}`,
          "effect-row",
        ),
      );
    }
    const details = el("details");
    details.append(
      el("summary", "変換された作用・出典の詳細"),
      el("pre", json(program)),
    );
    box.append(details);
    return box;
  }
  function pairIds() {
    const child = $("compare-branch").value,
      b = reference(child);
    if (!b) throw new Error("介入後の分岐を選択してください。");
    return [b.parent_branch_id || "main", child];
  }
  async function loadComparison(minute, expectedPlayToken, providedResult) {
    const ids = pairIds(),
      refs = ids.map(reference),
      lo = Math.max(...refs.map((r) => r.fork_minute)),
      hi = Math.min(...refs.map((r) => r.computed_until));
    if (lo > hi)
      throw new Error(
        "共通の計算済み範囲がありません。両分岐を計算してください。",
      );
    const m = minute ?? hi;
    if (m < lo || m > hi)
      throw new Error("共通の計算済み範囲だけ比較できます。");
    const scope = state.scope,
      selected = $("compare-branch").value;
    const result =
      providedResult ||
      (await api(
        `${runBase()}/compare?branch_ids=${enc(ids.join(","))}&minute=${m}`,
      ));
    if (
      (expectedPlayToken !== undefined &&
        expectedPlayToken !== state.playToken) ||
      scope !== state.scope ||
      selected !== $("compare-branch").value
    )
      return;
    if (state.comparison?.minute !== result.minute) invalidateIntervention();
    state.comparison = result;
    state.forkFrom = null;
    if (
      state.sceneMode === "schematic" &&
      !result.frames.every(window.HinanjoScene.supported)
    ) {
      state.sceneMode = "physical";
      document
        .querySelectorAll("[data-scene-mode]")
        .forEach((s) => (s.value = "physical"));
    }
    $("compare-seek").min = lo;
    $("compare-seek").max = hi;
    $("compare-seek").value = m;
    $("comparison-range").textContent = `共通計算済み範囲 ${lo}–${hi} 分`;
    $("compare-time").textContent = time(m);
    $("branch-a-name").textContent = shortBranch(ids[0]);
    $("branch-b-name").textContent = shortBranch(ids[1]);
    $("branch-b-badge").hidden = false;
    const defs = metricDefs.slice(0, 4);
    renderKpis($("compare-kpis-a"), result.frames[0].kpis, defs);
    renderKpis($("compare-kpis-b"), result.frames[1].kpis, defs);
    $("difference-summary").replaceChildren();
    for (const d of defs) {
      const raw = result.differences[0]?.metrics[d.key],
        a = result.frames[0].kpis[d.key],
        b = result.frames[1].kpis[d.key],
        good = typeof raw === "number" && raw < 0,
        bad = raw > 0;
      const card = el(
        "div",
        undefined,
        `difference-card ${good ? "good" : bad ? "bad" : ""}`,
      );
      card.append(
        el("span", d.label),
        el(
          "strong",
          typeof raw === "number"
            ? `${raw > 0 ? "+" : ""}${num(raw * d.factor)} ${d.unit === "/100" || d.unit === "%" ? "pt" : d.unit}`
            : "取得不可",
        ),
        el(
          "span",
          `${num(a * d.factor)} → ${num(b * d.factor)} · ${typeof raw !== "number" ? "未提供" : good ? "減少" : bad ? "増加" : "変化なし"}`,
        ),
      );
      const track = el("div", undefined, "difference-track"),
        bar = el("i");
      bar.style.width = `${typeof raw === "number" ? Math.min(100, (Math.abs(raw) / Math.max(Math.abs(a), Math.abs(b), 1)) * 100) : 0}%`;
      track.append(bar);
      card.append(track);
      $("difference-summary").append(card);
    }
    for (const [index, key] of ["a", "b"].entries()) {
      const frame = result.frames[index],
        missing = frame.facilities.filter(
          (f) => !f.point && !f.service_queues?.some((q) => q.point),
        ).length;
      $(`compare-conditions-${key}`).textContent =
        `${state.sceneMode === "schematic" && window.HinanjoScene.supported(frame) ? "模式図（距離非対応）" : "実座標"} · 稼働トイレ ${num(frame.kpis.toilet_working_units)}基 · 実座標未記録設備 ${missing}件`;
    }
    if (state.page === "comparison") renderMultiChoices();
    try {
      const record = JSON.parse(
        sessionStorage.getItem(`hinanjo-if-intervention-${ids[1]}`) || "null",
      );
      if (record) {
        $("comparison-instruction").textContent =
          `${time(record.minute)}：${record.text}`;
        $("comparison-effect").replaceChildren(
          el("span", "明示承認して適用した作用", "badge teal-badge"),
          effectSummary(record.program),
        );
      } else {
        $("comparison-instruction").textContent =
          `${time(refs[1].fork_minute)} に分岐`;
        $("comparison-effect").replaceChildren(
          el(
            "p",
            `稼働トイレ ${num(result.frames[0].kpis.toilet_working_units)} → ${num(result.frames[1].kpis.toilet_working_units)}基 · 介入文の端末記録なし。保存された分岐と施設条件を比較しています。`,
            "fine",
          ),
        );
      }
    } catch {
      /* Session preference is optional. */
    }
    controls();
    drawAll();
  }
  // The comparison page as it opens (#374): the pair is read at once when there is one, and the
  // open branch is shown alone (with where the new branch will go) when there is none or a
  // branch is being made from the simulation page's minute.
  function openComparison() {
    const pair = (state.data?.branches || []).length > 1;
    if (!state.forkFrom && pair && !state.comparison && state.run)
      void action("共通の計算済み範囲を取得しています…", () =>
        loading(() => {
          const ids = pairIds(),
            refs = ids.map(reference),
            lo = Math.max(...refs.map((r) => r.fork_minute)),
            hi = Math.min(...refs.map((r) => r.computed_until));
          return loadComparison(
            state.minute >= lo && state.minute <= hi ? state.minute : undefined,
          );
        }),
      );
    $("branch-b-badge").hidden = !comparing();
    if (comparing()) return;
    $("branch-a-name").textContent = shortBranch(state.branch);
    $("branch-b-name").textContent = "これから作る分岐";
    $("compare-kpis-a").replaceChildren();
    $("compare-kpis-b").replaceChildren();
    $("compare-conditions-a").textContent = "";
    $("compare-conditions-b").textContent = "";
    $("comparison-instruction").textContent = state.forkFrom
      ? `${time(state.forkFrom.minute)}から分岐を作ります。左の欄に介入の指示を書いてください。`
      : pair
        ? "比べる分岐を読み込んでいます…"
        : "まだ分岐がありません。左の欄で、この時刻から分岐を作れます。";
    $("comparison-effect").replaceChildren(
      el("p", "分岐を作ると、適用した作用と検証結果を表示します。", "muted"),
    );
    $("difference-summary").replaceChildren(
      el("p", "分岐を作って比べると、同じ時刻の指標の違いを表示します。", "muted"),
    );
  }
  // 2–8 branches at one minute (#376): the branches to set side by side, and the table of their
  // indicators from `compare` (its differences are each branch's against the first).
  function renderMultiChoices() {
    // The original Scenario first (the table's differences are against the first), then by the
    // minute each branch starts.
    const branches = [...(state.data?.branches || [])].sort(
        (a, b) =>
          (a.branch_id === "main" ? -1 : 0) - (b.branch_id === "main" ? -1 : 0) ||
          a.fork_minute - b.fork_minute,
      ),
      box = $("multi-compare-choices"),
      chosen = new Set(state.multiTouched ? multiChosen() : []);
    if (!chosen.size)
      for (const id of comparing() ? pairIds() : [state.branch]) chosen.add(id);
    box.replaceChildren(
      ...branches.map((b) => {
        const label = el("label", undefined, "checkbox multi-choice"),
          input = el("input");
        input.type = "checkbox";
        input.value = b.branch_id;
        input.checked = chosen.has(b.branch_id);
        input.addEventListener("change", () => {
          state.multiTouched = true;
          controls();
        });
        label.append(
          input,
          el(
            "span",
            `${shortBranch(b.branch_id)}${b.fork_minute ? `（${time(b.fork_minute)}から）` : ""} · ${time(b.computed_until)}まで計算`,
          ),
        );
        return label;
      }),
    );
    $("multi-compare").hidden = branches.length < 2;
  }
  function multiChosen() {
    return [...$("multi-compare-choices").querySelectorAll("input:checked")].map(
      (i) => i.value,
    );
  }
  async function loadMultiCompare() {
    const ids = multiChosen();
    if (ids.length < 2 || ids.length > 8)
      throw new Error("比べる分岐を2〜8つ選んでください。");
    const refs = ids.map(reference),
      lo = Math.max(...refs.map((r) => r.fork_minute)),
      hi = Math.min(...refs.map((r) => r.computed_until));
    if (lo > hi)
      throw new Error(
        "選んだ分岐に共通の計算済みの時刻がありません。分岐を計算してください。",
      );
    const shown = state.comparison?.minute ?? state.minute,
      minute = shown >= lo && shown <= hi ? shown : hi,
      result = await api(
        `${runBase()}/compare?branch_ids=${enc(ids.join(","))}&minute=${minute}`,
      );
    const table = $("multi-compare-table"),
      head = el("tr");
    head.append(el("th", `${time(result.minute)}の指標`));
    for (const frame of result.frames)
      head.append(el("th", shortBranch(frame.branch_id)));
    const rows = metricDefs.map((d) => {
      const row = el("tr");
      row.append(el("th", `${d.label}（${d.unit}）`));
      result.frames.forEach((frame, index) => {
        const value = frame.kpis[d.key],
          diff = index ? result.differences[index - 1]?.metrics[d.key] : null,
          cell = el(
            "td",
            typeof value === "number" ? num(value * d.factor) : "—",
          );
        if (typeof diff === "number" && diff !== 0) {
          cell.append(
            el(
              "small",
              ` ${diff > 0 ? "+" : ""}${num(diff * d.factor)}`,
              diff < 0 ? "good" : "bad",
            ),
          );
        }
        row.append(cell);
      });
      return row;
    });
    table.replaceChildren(head, ...rows);
    $("multi-compare-note").textContent =
      `共通の計算済みの時刻 ${time(lo)}〜${time(hi)} のうち、${time(result.minute)}で比べました。`;
  }
  async function loadHistory() {
    const lo = lower(),
      hi = upper(),
      step = Math.max(1, Math.ceil((hi - lo) / 90)),
      params = new URLSearchParams({
        kind: "kpis",
        start: String(lo),
        end: String(hi),
        step: String(step),
        limit: "100",
      });
    const rows = [];
    let cursor = null;
    do {
      if (cursor) params.set("cursor", cursor);
      const page = await api(`${branchBase()}/history?${params}`);
      rows.push(...page.items);
      cursor = page.next_cursor;
    } while (cursor && rows.length < 1000);
    state.history = rows;
    $("history-note").title =
      "グラフは各分の計算の後、マップは各分の計算の前の記録です。避難者数の時系列は、今の指標の記録に含まれていません。";
    $("history-note").textContent = rows.length
      ? `記録 ${rows.length}点 · ${step}分間隔 · 経過 ${lo}–${hi}分（記録時点の詳細はここにマウスを重ねて確認）`
      : "指標の記録はまだありません。少なくとも1分計算して更新してください。";
    drawHistory();
  }
  async function loadReport() {
    const scope = state.scope;
    const [report, evidence, llm] = await Promise.all([
      api(`${branchBase()}/report`),
      api(`${runBase()}/evidence`),
      api(`${branchBase()}/llm-status`),
    ]);
    if (scope !== state.scope) return;
    state.report = report;
    state.evidence = evidence;
    showMode(llm);
    $("report-status").textContent =
      report.status === "rule_fallback"
          ? "規則による代替レポート"
          : report.status === "unavailable"
            ? "レポート生成不可"
            : report.status === "validated"
              ? "AIレポート生成済み"
              : report.status || "取得済み";
    $("report-raw").textContent = report.markdown || json(report);
    $("report-evidence").textContent = json({ evidence, llm });
    $("report-summary").replaceChildren();
    for (const [title, text] of [
      [
        "記録から確認できること",
        `現在の平均ストレス ${num(state.frame.kpis.stress_mean * 100)}/100。トイレ待機人数 ${num(state.frame.kpis.toilet_queue_length)}人。未解決支援要請 ${num(state.frame.kpis.unresolved_support_request_count)}件。`,
      ],
      [
        "課題と次の検討",
        state.frame.kpis.unresolved_support_request_count > 0
          ? "支援要請とスタッフの資格・稼働の記録を確認し、介入を分岐比較で検討してください。"
          : "取得範囲に記録された事象を確認し、別条件での結果を比較してください。",
      ],
      [
        "解釈上の限界",
        "これは合成人物とモデル内の試算です。実測で校正された予測ではありません。感染症リスク・情報到達率など未提供の指標は表示しません。",
      ],
    ]) {
      const box = el("div", undefined, "report-finding");
      box.append(el("strong", title), el("span", text));
      $("report-summary").append(box);
    }
    const review = report.review?.review;
    const recommended = {
      compare_capacity_intervention:
        "同じ条件を起点に、施設容量の介入を比較する",
      inspect_event_chain: "記録されたイベントと、その後の状態変化を確認する",
      repeat_across_seeds: "複数のseedで同じ分岐比較を繰り返す",
      calibrate_with_aggregate_data: "適用可能な集計データで仮定を校正する",
      review_branch_differences: "結論を出す前に、元の分岐との違いを確認する",
    };
    if (review?.recommendations?.length) {
      const box = el("div", undefined, "report-finding ai-finding");
      box.append(
        el(
          "strong",
          report.status === "validated"
            ? "AIが選んだ検討案（効果は未検証）"
            : "記録に基づく検討案",
        ),
      );
      const list = el("ul");
      for (const item of review.recommendations) {
        if (recommended[item.action])
          list.append(el("li", recommended[item.action]));
      }
      box.append(list);
      $("report-summary").insertBefore(box, $("report-summary").children[2]);
    }
    if (report.markdown)
      $("report-summary").append(
        el(
          "p",
          "分岐のレポートの生成状態・観測・仮定・提案は、以下の原文から確認できます。",
          "fine",
        ),
      );
    else
      $("report-summary").append(
        el(
          "p",
          report.limitation ||
            report.limits ||
            "AIレポートは生成されていません。記録済みの数値を確認してください。",
          "fine",
        ),
      );
    $("report-people").replaceChildren();
    for (const p of state.frame.people.slice(0, 4)) {
      const row = el("div", undefined, "person-row"),
        body = el("div");
      body.append(el("strong", p.id), el("span", personSummary(p)));
      const avatar = el("span", undefined, "avatar");
      avatar.append(uiIcon("people"));
      row.append(avatar, body);
      $("report-people").append(row);
    }
    for (const s of state.frame.staff.slice(0, 2)) {
      const row = el("div", undefined, "person-row");
      const avatar = el("span", undefined, "avatar");
      avatar.append(uiIcon("staff"));
      row.append(
        avatar,
        el(
          "span",
          `${s.id} / ${s.zone_id} / 担当 ${s.active_task_id || "なし"}`,
        ),
      );
      $("report-people").append(row);
    }
    renderEvents($("report-events"), state.frame.events);
    await loadConversations();
    await loadHistory();
    drawAll();
  }
  // The questions asked about this branch before and their answers (#376, GET conversations),
  // above the chat's new ones; read when the report opens, as the record keeps them.
  async function loadConversations() {
    const scope = state.scope,
      branch = state.branch,
      { turns } = await api(`${branchBase()}/conversations`);
    if (scope !== state.scope || branch !== state.branch) return;
    const chat = $("chat");
    chat.querySelector(".chat-history")?.remove();
    if (!turns.length) return;
    const past = el("div", undefined, "chat-history");
    past.append(el("small", `これまでの質問と回答（${turns.length}件）`, "chat-history-title"));
    for (const turn of turns) {
      past.append(el("div", turn.question, "chat-bubble user"));
      const answer = el("div", undefined, "chat-bubble answer");
      answer.append(
        el("small", `${time(turn.minute)}について`),
        el("p", turn.answer),
      );
      past.append(answer);
    }
    chat.querySelector(".chat-intro")?.remove();
    chat.prepend(past);
    chat.scrollTop = chat.scrollHeight;
  }
  // The detail page (#376, AGENTS.md's provenance on the screen): the Run's record and branches,
  // the models and coefficients and the sources and assumptions its evidence recorded, the
  // effects an intervention can use and the LLM's use. The routes the browser version does not
  // answer (contracts, usage) are said to be the server's.
  function facts(box, pairs) {
    box.replaceChildren();
    for (const [key, value] of pairs)
      box.append(el("dt", key), el("dd", value ?? "未記録"));
  }
  function table(box, header, rows) {
    const head = el("tr");
    for (const h of header) head.append(el("th", h));
    box.replaceChildren(
      head,
      ...rows.map((cells) => {
        const row = el("tr");
        for (const cell of cells) row.append(el("td", cell ?? "—"));
        return row;
      }),
    );
  }
  async function optional(path) {
    try {
      return await api(path);
    } catch (error) {
      if (error.code === "browser_unsupported") return null;
      throw error;
    }
  }
  async function loadDetail() {
    const scope = state.scope,
      [run, evidence, llm, contracts, usage] = await Promise.all([
        api(runBase()),
        api(`${runBase()}/evidence`),
        api(`${branchBase()}/llm-status`),
        optional(`${base()}/contracts`),
        optional(`${runBase()}/usage`),
      ]);
    if (scope !== state.scope) return;
    const scenario = state.scenario;
    facts($("detail-record"), [
      ["シナリオ", $("scenario-name").textContent],
      ["状態", runStatusLabel(run.status)],
      ["条件（Scenario）", `${run.scenario_id} の版 ${run.scenario_version}`],
      ["条件のhash", run.scenario_hash],
      ["seed", String(run.seed)],
      ["期間", scenario ? `${num(scenario.duration_minutes / 1440)}日間（${scenario.duration_minutes}分）` : null],
      ["記録のID", run.run_id],
      ["承認", "プレビューを確かめて明示承認した条件だけから作ります（承認の時刻は記録していません）"],
    ]);
    renderScenarioRecord();
    $("detail-versions").textContent = json(run.model_versions);
    table(
      $("detail-branches"),
      ["分岐", "元", "分岐した時刻", "計算済み", "保存した時点"],
      run.branches.map((b) => [
        shortBranch(b.branch_id),
        b.parent_branch_id ? shortBranch(b.parent_branch_id) : "—",
        b.fork_minute ? time(b.fork_minute) : "—",
        time(b.computed_until),
        b.checkpoint_hash ? b.checkpoint_hash.slice(0, 12) : "—",
      ]),
    );
    $("detail-legend").textContent = Object.entries(evidence.legend || {})
      .map(([grade, text]) => `${grade}：${text}`)
      .join(" ");
    table(
      $("detail-models"),
      ["領域", "モデル", "根拠の等級", "係数", "参照"],
      Object.entries(evidence.models || {}).map(([domain, m]) => [
        domain,
        m.id,
        m.evidence_grade,
        m.coefficient_status,
        m.reference,
      ]),
    );
    table(
      $("detail-parameters"),
      ["係数", "値", "単位", "範囲", "等級", "根拠"],
      (evidence.parameters || []).map((row) => [
        row.path,
        typeof row.value === "object" ? json(row.value) : String(row.value),
        row.unit,
        `${row.lower ?? "—"}〜${row.upper ?? "—"}`,
        row.evidence_grade,
        row.source_id,
      ]),
    );
    $("detail-interpretation").textContent = [
      evidence.interpretation,
      evidence.external_validation === false
        ? "外部の実測で確かめたモデルではありません。"
        : null,
    ]
      .filter(Boolean)
      .join(" ");
    sourceRows($("detail-sources"), evidence.sources || []);
    sourceRows($("detail-assumptions"), [
      ...(evidence.scenario_sources || []),
      ...(evidence.scenario_assumptions || []),
    ]);
    $("detail-effects").replaceChildren(
      ...(contracts
        ? (contracts.executable_effect_primitives || []).map((e) => {
            const row = el("div", undefined, "source-row");
            row.append(
              el("strong", e.op),
              el("span", `${e.description || ""}${e.timed ? "（期間あり）" : ""}`),
            );
            return row;
          })
        : [
            el(
              "p",
              "ブラウザー版は、作用の型の一覧を持っていません。介入のプレビューで、文から読み取った作用と検証の結果を確かめられます。",
              "fine",
            ),
          ]),
    );
    facts($("detail-llm"), [
      ["回答の作り方", llmModeLabel(llm.mode)],
      ["代替処理の回数", String(llm.outcomes?.fallback_count ?? "—")],
      [
        "APIの呼び出し",
        usage
          ? `${usage.totals?.calls ?? 0}回 · 入力 ${num(usage.totals?.input_tokens ?? 0)} / 出力 ${num(usage.totals?.output_tokens ?? 0)} トークン`
          : "ブラウザー版はAPIを呼びません",
      ],
    ]);
  }
  async function askQuestion() {
    const question = $("question").value.trim();
    if (!question) throw new Error("質問を入力してください。");
    const version = ++state.chatVersion,
      scope = state.scope,
      minute = state.minute,
      branch = state.branch;
    $("chat").querySelector(".chat-intro")?.remove();
    $("chat").append(el("div", question, "chat-bubble user"));
    const ids =
      state.comparison?.frames
        .map((f) => f.branch_id)
        .filter(
          (id) =>
            id !== state.branch &&
            minute >= reference(id).fork_minute &&
            minute <= reference(id).computed_until,
        ) || [];
    const answer = await api(`${branchBase()}/questions`, {
      question,
      minute,
      context_id: state.context,
      comparison_branch_ids: ids,
      ...(state.selected &&
      state.frame.people.some((p) => p.id === state.selected)
        ? { agent_id: state.selected }
        : {}),
    });
    if (
      version !== state.chatVersion ||
      scope !== state.scope ||
      minute !== state.minute ||
      branch !== state.branch
    )
      return;
    state.context = answer.context_id;
    const bubble = el("div", undefined, "chat-bubble answer");
    const usage = answer.usage || {};
    bubble.append(
      el(
        "small",
        `${llmModeLabel(answer.mode)} · ${usage.fallback_used ? "代替処理あり" : usage.outcome === "recorded" ? "API応答を記録・検証" : "記録を参照"} / ${time(minute)}`,
      ),
    );
    if (answer.facts?.length)
      bubble.append(
        el("strong", "記録された事実"),
        ...answer.facts.map((c) => el("p", c.text)),
      );
    if (answer.explanations?.length)
      bubble.append(
        el("strong", "AIによる解釈・提案（記録そのものではありません）"),
        ...answer.explanations.map((c) =>
          el(
            "p",
            `${c.kind === "synthetic_dialogue" ? "合成の声（実発言ではありません）：" : ""}${c.text}`,
          ),
        ),
      );
    if (!answer.facts?.length && !answer.explanations?.length)
      bubble.append(el("p", answer.answer));
    const sourceChips = el("div", undefined, "source-chips");
    for (const source of answer.sources || []) {
      const chip = el(
        "span",
        `${source.alias} · ${source.label}`,
        "source-chip",
      );
      chip.title = json(source.reference);
      sourceChips.append(chip);
    }
    bubble.append(sourceChips);
    const detail = el("details");
    detail.append(
      el(
        "summary",
        `出典・生成情報・回答の限界（${answer.sources?.length || 0}件の出典）`,
      ),
      el(
        "pre",
        json({
          sources: answer.sources || [],
          mode: answer.mode,
          model: answer.model,
          usage,
          limitations: answer.limitations || [],
        }),
      ),
    );
    bubble.append(detail);
    $("chat").append(bubble);
    $("chat").scrollTop = $("chat").scrollHeight;
    $("question").value = "";
  }
  async function saveReport() {
    const branchIds = state.comparison ? pairIds() : [state.branch],
      minute = state.comparison ? state.comparison.minute : state.minute,
      button = $("save-report");
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
    note("Opusによる分析とPDFの生成中です。保存済みのレポートがあれば再利用します…", "loading");
    let blob;
    try {
      const response = await fetch(`${runBase()}/pdf-report`, {
        method: "POST",
        headers: { Accept: "application/pdf", "Content-Type": "application/json" },
        body: JSON.stringify({ branch_ids: branchIds, minute }),
      });
      if (!response.ok) {
        const result = await response.json();
        throw new Error(result.error || `PDF生成に失敗しました (HTTP ${response.status})`);
      }
      if (!response.headers.get("Content-Type")?.startsWith("application/pdf"))
        throw new Error("PDFを取得できませんでした。再試行してください。");
      blob = await response.blob();
    } finally {
      button.removeAttribute("aria-busy");
    }
    const url = URL.createObjectURL(blob),
      a = el("a");
    a.href = url;
    a.download = `hinanjo-if-${branchIds.join("-vs-")}-${minute}.pdf`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 5000);
  }
  // Canvas rendering uses geometry, physical points, and recorded staff positions only.
  function surface(canvas) {
    const r = canvas.getBoundingClientRect();
    if (!r.width || !r.height) return null;
    const dpr = Math.min(devicePixelRatio || 1, 2);
    const width = Math.round(r.width * dpr),
      height = Math.round(r.height * dpr);
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    const ctx = canvas.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return { ctx, w: r.width, h: r.height };
  }
  const hits = new Map();
  function drawMap(canvas, frame, layer = "normal", zoom = state.zoom, extra = {}) {
    const sf = surface(canvas);
    if (!sf) return;
    const interactive = [
      "simulation-map",
      "compare-map-a",
      "compare-map-b",
    ].includes(canvas.id);
    hits.set(
      canvas,
      window.HinanjoMap.draw(sf, frame, {
        viewportBounds:
          state.page === "comparison" &&
          state.sceneMode === "physical" &&
          state.comparison
            ? {
                width: Math.max(
                  ...state.comparison.frames.map(
                    (f) => f.geometry?.width_m || 1,
                  ),
                ),
                height: Math.max(
                  ...state.comparison.frames.map(
                    (f) => f.geometry?.height_m || 1,
                  ),
                ),
              }
            : undefined,
        mode: state.sceneMode,
        animate:
          state.playing &&
          !matchMedia("(prefers-reduced-motion: reduce)").matches,
        time: state.visualMs,
        reducedMotion: matchMedia("(prefers-reduced-motion: reduce)").matches,
        gaitDistances: state.replay?.gaits.get(frame?.branch_id),
        layer,
        zoom,
        selected: state.selected,
        pan: interactive ? state.mapPan : { x: 0, y: 0 },
        ...extra,
      }),
    );
  }
  function drawPopulation(scenario) {
    const sf = surface($("population-chart"));
    if (!sf) return;
    const { ctx, w, h } = sf,
      people = Object.values(scenario.model_configs?.population?.people || {}),
      groups = [
        ["子ども", 0, "#ffb449"],
        ["成人", 0, "#3c98ff"],
        ["高齢者", 0, "#f26b8c"],
        ["未記録", 0, "#bdcbd7"],
      ];
    for (const p of people) {
      const a = p.age_years;
      groups[typeof a !== "number" ? 3 : a < 18 ? 0 : a >= 65 ? 2 : 1][1]++;
    }
    const total = people.length;
    ctx.font = "10px sans-serif";
    if (!total) {
      ctx.fillText("年齢構成の記録なし", 8, h / 2);
      return;
    }
    let angle = -Math.PI / 2;
    const cx = w / 2,
      cy = h * 0.36,
      r = Math.min(w * 0.28, h * 0.23);
    for (const [label, count, color] of groups) {
      ctx.beginPath();
      ctx.arc(cx, cy, r, angle, angle + (count / total) * Math.PI * 2);
      ctx.arc(
        cx,
        cy,
        r * 0.6,
        angle + (count / total) * Math.PI * 2,
        angle,
        true,
      );
      ctx.closePath();
      ctx.fillStyle = color;
      ctx.fill();
      angle += (count / total) * Math.PI * 2;
      const index = groups.findIndex((g) => g[0] === label);
      ctx.fillRect(10, h * 0.68 + index * 13, 7, 7);
      ctx.fillStyle = "#36577c";
      ctx.fillText(`${label} ${count}人`, 22, h * 0.68 + index * 13 + 7);
    }
  }
  function drawHistory() {
    const sf = surface($("history-chart"));
    if (!sf) return;
    const { ctx, w, h } = sf;
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, w, h);
    const key = $("chart-metric").value,
      overview = key === "overview";
    const definitions = overview
      ? [
          {
            key: "stress_mean",
            label: "平均ストレス /100",
            factor: 100,
            color: "#ff365e",
            axis: "left",
          },
          {
            key: "staff_load_ratio",
            label: "スタッフ負荷 %",
            factor: 100,
            color: "#efa023",
            axis: "left",
          },
        ]
      : [
          {
            ...metricDefs.find((d) => d.key === key),
            color:
              key === "stress_mean"
                ? "#ff365e"
                : key === "staff_load_ratio"
                  ? "#efa023"
                  : "#0878ff",
            axis: "left",
          },
        ];
    const raw = state.history
      .map((row) => row.data || row.snapshot || row)
      .filter((r) => typeof r.minute_index === "number");
    const series = definitions
      .map((d) => ({
        ...d,
        points: raw.filter((r) => typeof r[d.key] === "number"),
      }))
      .filter((d) => d.points.length);
    ctx.font = "11px sans-serif";
    ctx.fillStyle = "#6783a4";
    if (!series.length) {
      ctx.fillText("この指標の時系列記録はありません", 20, h / 2);
      return;
    }
    const left = 43,
      right = 18,
      top = 40,
      bottom = 32,
      pw = w - left - right,
      ph = h - top - bottom;
    const minX = Math.min(...raw.map((p) => p.minute_index)),
      maxX = Math.max(minX + 1, ...raw.map((p) => p.minute_index));
    const leftValues = series
      .filter((d) => d.axis === "left")
      .flatMap((d) => d.points.map((p) => p[d.key] * d.factor));
    const high = Math.max(1, ...leftValues),
      maxY = overview
        ? 100
        : high >= 10
          ? Math.ceil((high * 1.05) / 10) * 10
          : high * 1.1;
    let legendX = left;
    for (const d of series) {
      ctx.strokeStyle = d.color;
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.moveTo(legendX, 13);
      ctx.lineTo(legendX + 16, 13);
      ctx.stroke();
      ctx.fillStyle = "#47658f";
      ctx.textAlign = "left";
      ctx.font = "10px sans-serif";
      ctx.fillText(d.label, legendX + 21, 17);
      legendX += ctx.measureText(d.label).width + 38;
    }
    ctx.lineWidth = 1;
    for (let i = 0; i <= 4; i++) {
      const y = top + (ph * i) / 4;
      ctx.strokeStyle = "#e5eef7";
      ctx.beginPath();
      ctx.moveTo(left, y);
      ctx.lineTo(w - right, y);
      ctx.stroke();
      ctx.textAlign = "right";
      ctx.fillStyle = "#7186a8";
      ctx.font = "10px sans-serif";
      ctx.fillText(num(maxY * (1 - i / 4)), left - 7, y + 4);
    }
    const xOf = (m) => left + ((m - minX) / (maxX - minX)) * pw;
    for (const d of series) {
      const pos = (p) => [
        xOf(p.minute_index),
        top +
          ph -
          ((p[d.key] * d.factor) / (d.axis === "right" ? 100 : maxY)) * ph,
      ];
      ctx.beginPath();
      d.points.forEach((p, i) => {
        const [x, y] = pos(p);
        i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
      });
      ctx.strokeStyle = d.color;
      ctx.lineWidth = 2.3;
      ctx.stroke();
      if (d.key === "population") {
        const last = pos(d.points.at(-1)),
          first = pos(d.points[0]);
        ctx.lineTo(last[0], top + ph);
        ctx.lineTo(first[0], top + ph);
        ctx.closePath();
        ctx.fillStyle = "#0878ff0b";
        ctx.fill();
      }
      for (
        let i = 0;
        i < d.points.length;
        i += Math.max(1, Math.ceil(d.points.length / 14))
      ) {
        const [x, y] = pos(d.points[i]);
        ctx.fillStyle = d.color;
        ctx.beginPath();
        ctx.arc(x, y, 2.1, 0, Math.PI * 2);
        ctx.fill();
      }
    }
    ctx.textAlign = "center";
    ctx.font = "10px sans-serif";
    ctx.fillStyle = "#647fa3";
    for (let i = 0; i <= 3; i++) {
      const m = Math.round(minX + ((maxX - minX) * i) / 3);
      ctx.textAlign = i === 0 ? "left" : i === 3 ? "right" : "center";
      ctx.fillText(time(m), left + (pw * i) / 3, h - 10);
    }
  }
  function drawAll() {
    const visible = (frame, index = 0) =>
      state.motion
        ? window.HinanjoScene.sample(
            state.motion.from[index],
            state.motion.to[index],
            motionFraction(),
            state.sceneMode,
          )
        : frame;
    if (state.page === "scenario" && state.preview) {
      const f = state.preview.facility_design;
      drawMap(
        $("preview-map"),
        {
          geometry: f?.geometry,
          zones: f?.zones,
          agents: [],
          people: [],
          facilities: (f?.facilities || []).map((x) => ({
            ...x,
            point: f.geometry.facility_points[x.id],
          })),
          staff: [],
        },
        "normal",
        1,
      );
      drawPopulation(state.preview.scenario);
    }
    if (state.page === "simulation")
      drawMap($("simulation-map"), visible(state.frame), $("layer").value);
    if (state.page === "comparison" && comparing()) {
      drawMap(
        $("compare-map-a"),
        visible(state.comparison.frames[0], 0),
        $("compare-layer").value,
      );
      drawMap(
        $("compare-map-b"),
        visible(state.comparison.frames[1], 1),
        $("compare-layer").value,
      );
    } else if (state.page === "comparison") {
      // The open branch alone, and B's place for the branch to be made.
      drawMap($("compare-map-a"), visible(state.frame), $("compare-layer").value);
      drawMap($("compare-map-b"), null, $("compare-layer").value, state.zoom, {
        emptyText: state.run
          ? "この時刻から分岐を作ると、ここに並びます"
          : "シナリオを開くと施設が表示されます",
      });
    }
    if (state.page === "report") drawHistory();
  }
  document
    .querySelectorAll("[data-page]")
    .forEach((b) => b.addEventListener("click", () => show(b.dataset.page)));
  // The logo opens the home, keeping what is open.
  document.querySelector(".brand").addEventListener("click", (event) => {
    if (event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey)
      return;
    event.preventDefault();
    show("home");
  });
  $("home-new").addEventListener("click", newScenario);
  on("scenario-copy", async () => {
    await copyScenario();
    show("simulation");
  });
  $("home-fetch").addEventListener("click", () => void openExamples());
  $("home-api-enable").addEventListener("click", () => {
    $("api-dialog-note").textContent = "";
    $("api-dialog").showModal();
    $("api-passphrase").focus();
  });
  $("home-api-disable").addEventListener("click", () => {
    keepPassphrase("");
    showApi(null, "解除しました。");
  });
  $("api-form").addEventListener("submit", (event) => void enableApi(event));
  $("api-dialog-close").addEventListener("click", () => $("api-dialog").close());
  $("examples-dialog-close").addEventListener("click", () => {
    if (!fetching) $("examples-dialog").close();
  });
  // While an example is fetched, Esc does not close the dialog (its progress would be lost).
  $("examples-dialog").addEventListener("cancel", (event) => {
    if (fetching) event.preventDefault();
  });
  $("scenario-form").addEventListener("submit", (e) => {
    e.preventDefault();
    void action(
      "条件を構造化し、施設・人口・出典をプレビューしています…",
      previewScenario,
    );
  });
  $("scenario-form").addEventListener("input", invalidateScenario);
  $("scenario-form").addEventListener("change", invalidateScenario);
  $("example").addEventListener("click", () => {
    $("scenario-text").value =
      "墨田区で地震が発生。避難者100人、7日間。高齢者や子ども連れの支援ニーズも見たい。";
    for (const id of [
      "municipality",
      "disaster",
      "evacuees",
      "duration",
      "staff-count",
      "area",
      "toilets",
      "food",
      "water",
      "facility-type",
    ])
      $(id).value = "";
    invalidateScenario();
  });
  $("scenario-check").addEventListener("change", controls);
  on("approve", approveScenario);
  $("play").addEventListener("click", () => {
    const mode = $("play").dataset.mode;
    if (mode === "cancel") void cancelCompute();
    else if (mode === "compute") void computeToEnd(computeTargets());
    else togglePlayback().catch((e) => note(e.message, "error"));
  });
  $("speed").addEventListener("change", () =>
    state.replay?.setRate(Number($("speed").value)),
  );
  document.addEventListener("visibilitychange", () => {
    if (document.hidden && state.playing) stopPlayback(true);
  });
  $("seek").addEventListener("change", () => {
    stopPlayback();
    void action("記録された時刻へ移動しています…", () =>
      seek(Number($("seek").value)),
    );
  });
  $("fork-day").addEventListener("change", pickForkTime);
  $("fork-clock").addEventListener("change", pickForkTime);
  $("intervention-text").addEventListener("input", invalidateIntervention);
  on("intervention-preview", previewIntervention);
  $("intervention-check").addEventListener("change", controls);
  on("intervention-apply", applyIntervention);
  $("person-dialog-close").addEventListener("click", () =>
    $("person-dialog").close(),
  );
  // 吹き出しの外をクリックすると閉じる（マップ上の人物クリックは開き直す）
  document.addEventListener("pointerdown", (e) => {
    const d = $("person-dialog");
    if (d.open && !d.contains(e.target) && e.target.tagName !== "CANVAS")
      d.close();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      $("person-dialog").close();
      $("people-panel").classList.remove("expanded");
    }
  });
  // 「避難者・スタッフの状態」をページ内で大きく表示する
  $("people-expand").addEventListener("click", () => {
    const panel = $("people-panel"),
      open = panel.classList.toggle("expanded");
    $("people-expand").textContent = open ? "× 閉じる" : "⤢ 大きく表示";
    if (state.frame) renderPeople();
  });
  // 「この時刻から分岐を作る」: the comparison page with the simulation page's minute (#374).
  $("branch-here").addEventListener("click", () => {
    stopPlayback();
    state.forkFrom = { branch: state.branch, minute: state.minute };
    state.forkPick = null;
    state.comparison = null;
    invalidateIntervention();
    show("comparison");
    scrollTo(0, 0);
    $("intervention-text").focus({ preventScroll: true });
  });
  on("multi-compare-run", loadMultiCompare);
  $("compare-branch").addEventListener("change", () => {
    state.comparison = null;
    state.forkPick = null;
    state.forkFrom = null;
    drawAll();
    void action("共通の計算済み範囲を取得しています…", () =>
      loading(() => loadComparison()),
    );
  });
  $("compare-seek").addEventListener("change", () => {
    stopPlayback();
    state.forkPick = null;
    const minute = Number($("compare-seek").value);
    if (!comparing()) {
      if (state.forkFrom) state.forkFrom = { ...state.forkFrom, minute };
      void action("記録された時刻へ移動しています…", () => seek(minute));
      return;
    }
    void action("両分岐の同じ時刻を取得しています…", () => loadComparison(minute));
  });
  function syncLayerTabs() {
    document
      .querySelectorAll("[data-layer]")
      .forEach((b) =>
        b.setAttribute(
          "aria-pressed",
          String(b.dataset.layer === $("layer").value),
        ),
      );
    const layer = $("layer").value,
      colored = ["stress", "health", "crowd"].includes(layer);
    for (const id of ["legend-agents", "legend-queue", "legend-staff"])
      $(id).hidden = colored;
    $("layer-scale").hidden = !colored;
    $("layer-scale").textContent =
      layer === "stress"
        ? "ストレス 低 → 高（0–100）"
        : layer === "health"
          ? "記録ニーズ 低 → 高（診断ではありません）"
          : "区画内の人数 / 面積（高密度ほど赤）";
  }
  for (const id of ["layer", "compare-layer", "chart-metric"])
    $(id).addEventListener("change", () => {
      syncLayerTabs();
      drawAll();
    });
  document.querySelectorAll("[data-layer]").forEach((b) =>
    b.addEventListener("click", () => {
      $("layer").value = b.dataset.layer;
      syncLayerTabs();
      drawAll();
    }),
  );
  document.querySelectorAll("[data-zoom]").forEach((b) =>
    b.addEventListener("click", () => {
      if (b.dataset.zoom === "reset") state.mapPan = { x: 0, y: 0 };
      state.zoom =
        b.dataset.zoom === "reset"
          ? 1
          : Math.max(
              0.6,
              Math.min(
                3,
                state.zoom * (b.dataset.zoom === "in" ? 1.2 : 1 / 1.2),
              ),
            );
      drawAll();
    }),
  );
  for (const id of ["simulation-map", "compare-map-a", "compare-map-b"]) {
    const canvas = $(id);
    let drag = null;
    canvas.tabIndex = 0;
    const hitAt = (e) => {
      const rect = canvas.getBoundingClientRect();
      return (hits.get(canvas) || [])
        .map((p) => ({
          ...p,
          d: Math.hypot(
            e.clientX - rect.left - (p.hitX ?? p.x),
            e.clientY - rect.top - (p.hitY ?? p.y),
          ),
        }))
        .filter((p) => p.d <= p.r)
        .sort((a, b) => a.d - b.d)[0];
    };
    canvas.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      drag = {
        x: e.clientX,
        y: e.clientY,
        px: state.mapPan.x,
        py: state.mapPan.y,
        moved: false,
      };
      canvas.setPointerCapture(e.pointerId);
    });
    canvas.addEventListener("pointermove", (e) => {
      if (drag) {
        const dx = e.clientX - drag.x,
          dy = e.clientY - drag.y;
        if (Math.hypot(dx, dy) > 4) drag.moved = true;
        if (drag.moved) {
          state.mapPan = { x: drag.px + dx, y: drag.py + dy };
          canvas.classList.add("dragging");
          drawAll();
        }
      } else {
        const hit = hitAt(e);
        canvas.title =
          hit?.label || "ドラッグで移動 · Ctrl＋ホイールで拡大縮小";
        canvas.style.cursor = hit ? "pointer" : "grab";
      }
    });
    canvas.addEventListener("pointerup", (e) => {
      if (drag && !drag.moved) {
        const hit = hitAt(e);
        if (hit?.type === "person") {
          if (id === "simulation-map")
            selectPerson(hit.id, hit.ids, { x: e.clientX, y: e.clientY });
          else {
            state.selected = hit.id;
            drawAll();
          }
          if (id !== "simulation-map") {
            const f = state.comparison
              ? state.comparison.frames[id === "compare-map-a" ? 0 : 1]
              : state.frame;
            const p = f.people.find((p) => p.id === hit.id),
              a = f.agents.find((a) => a.agent_id === hit.id);
            let detail = canvas.parentElement.querySelector(".map-inspector");
            if (!detail) {
              detail = el("div", undefined, "map-inspector");
              canvas.parentElement.append(detail);
            }
            detail.replaceChildren(
              el("strong", hit.id),
              el("span", p ? personSummary(p) : "スタッフ"),
              el(
                "small",
                `所属 ${a?.zone_id || "—"} · 記録座標 ${a?.point ? num(a.point.x) + ", " + num(a.point.y) + "m" : "未提供"}`,
              ),
            );
          }
        } else if (hit?.type === "facility") {
          $("person-detail").replaceChildren(
            el("div", hit.label, "person-state"),
          );
          state.selected = hit.id;
          drawAll();
        }
      }
      drag = null;
      canvas.classList.remove("dragging");
    });
    canvas.addEventListener("pointercancel", () => {
      drag = null;
      canvas.classList.remove("dragging");
    });
    canvas.addEventListener(
      "wheel",
      (e) => {
        if (!e.ctrlKey && !e.metaKey) return;
        e.preventDefault();
        state.zoom = Math.max(
          0.6,
          Math.min(3, state.zoom * (e.deltaY < 0 ? 1.12 : 1 / 1.12)),
        );
        drawAll();
      },
      { passive: false },
    );
    canvas.addEventListener("keydown", (e) => {
      const moves = {
        ArrowLeft: [24, 0],
        ArrowRight: [-24, 0],
        ArrowUp: [0, 24],
        ArrowDown: [0, -24],
      };
      if (moves[e.key]) {
        e.preventDefault();
        state.mapPan.x += moves[e.key][0];
        state.mapPan.y += moves[e.key][1];
        drawAll();
      } else if (e.key === "Home") {
        e.preventDefault();
        state.mapPan = { x: 0, y: 0 };
        state.zoom = 1;
        drawAll();
      } else if (e.key === "Escape") {
        state.selected = null;
        state.selectedGroup = [];
        document.querySelectorAll(".map-inspector").forEach((n) => n.remove());
        renderPeople();
        drawAll();
      }
    });
  }
  on("report-refresh", loadReport);
  on("save-report", saveReport);
  $("question-form").addEventListener("submit", (e) => {
    e.preventDefault();
    void action("結果Q&Aが記録済みの根拠を確認しています…", askQuestion);
  });
  document.querySelectorAll("[data-question]").forEach((b) =>
    b.addEventListener("click", () => {
      $("question").value = b.dataset.question;
      $("question").focus();
    }),
  );
  $("branch-select").addEventListener("change", () =>
    action("選択した分岐を取得しています…", () => loading(async () => {
      state.branch = $("branch-select").value;
      state.scope++;
      state.context = null;
      state.selected = null;
      state.selectedGroup = [];
      state.history = [];
      resetChat();
      invalidateIntervention();
      await refreshRun();
    })),
  );
  new ResizeObserver(() => requestAnimationFrame(drawAll)).observe(
    document.querySelector("main"),
  );
  async function init() {
    const requestedPage = location.hash.slice(1);
    const template = document.querySelector("[data-scene-mode]").parentElement;
    document
      .querySelector(".compare-controls")
      .append(template.cloneNode(true));
    for (const id of ["compare-map-a", "compare-map-b"])
      $(id).parentElement.classList.add("map-host");
    document.querySelectorAll("[data-scene-mode]").forEach((select) =>
      select.addEventListener("change", () => {
        state.sceneMode = select.value;
        state.replay?.gaits.clear();
        document
          .querySelectorAll("[data-scene-mode]")
          .forEach((s) => (s.value = state.sceneMode));
        state.mapPan = { x: 0, y: 0 };
        state.zoom = 1;
        if (state.frame) renderFrame();
        else drawAll();
        updateSceneCaption();
      }),
    );
    function updateSceneCaption() {
      document
        .querySelectorAll(".map-caption")
        .forEach(
          (n) =>
            (n.textContent =
              state.sceneMode === "schematic"
                ? "模式図 · 部屋形状・人物の個別配置は表示用／距離は非対応"
                : "実座標 · 床材と背景は表示上の演出"),
        );
    }
    updateSceneCaption();
    syncLayerTabs();
    document
      .querySelectorAll("[data-icon]")
      .forEach((n) => n.replaceChildren(uiIcon(n.dataset.icon)));
    document.querySelectorAll(".compare-controls [data-zoom]").forEach((b) => {
      b.title =
        "両マップを" +
        (b.dataset.zoom === "in"
          ? "拡大"
          : b.dataset.zoom === "out"
            ? "縮小"
            : "リセット");
      b.setAttribute("aria-label", b.title);
      b.textContent =
        b.dataset.zoom === "in" ? "＋" : b.dataset.zoom === "out" ? "−" : "⌖";
    });
    renderKpis($("sim-kpis"));
    renderKpiStrip($("report-kpis"));
    const projects = await api("/api/v1/projects");
    if (projects.projects.some((p) => p.project_id === state.project)) {
      await refreshRuns();
      if (state.run) {
        await refreshRun();
        showMode(await api(`${branchBase()}/llm-status`));
      }
    } else if (state.run) {
      state.run = null;
      note(
        "指定したシナリオが保存先にありません。ホームから開き直すか、新しいシナリオを作成してください。",
        "error",
      );
    }
    show(requestedPage || (state.run ? "simulation" : "home"));
    controls();
  }
  // What the screen says when it cannot open: the browser version's own failures (its store,
  // its Worker, #393) say what to do themselves, and the URL is not the cause.
  function loadFailure(error) {
    if (String(error?.code || "").startsWith("browser_")) return error.message;
    return `読み込みに失敗しました：${error.message}。URLのproject/run指定を確認してください。`;
  }
  loading(init).catch((e) => {
    note(loadFailure(e), "error");
    controls();
  });
})();
