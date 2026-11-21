/* The open Run on the simulation page: its frame, events and indicators, playback and
   computation. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const {
    state, reference, lower, upper, duration, remember, $, enc, metricDefs, eventNames, eventTone,
    time, shortBranch, runStatusLabel, num, el, uiIcon, lasting, base, runBase, branchBase, api, note,
    setPlayLabels, loading, action, show, controls, drawAll, runNames,
  } = app;
  const {
    renderPeople, invalidateIntervention, pairIds, loadComparison, resetChat,
  } = app.later;
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
    state.overlayHistory = [];
    $("report-overlay").value = "";
    state.context = null;
    state.selected = null;
    state.selectedGroup = [];
    state.mapPan = { x: 0, y: 0 };
    state.zoom = 1;
    state.report = null;
    state.summary = null;
    invalidateIntervention();
    resetChat();
    await refreshRuns();
    await refreshRun();
  }
  // What the screen shows of a Run, emptied when none is open.
  function clearRunView() {
    renderKpis($("sim-kpis"));
    renderReportSummary(null);
    $("seek-label").textContent = "開始前";
    $("branch-select").replaceChildren();
    $("compare-branch").replaceChildren();
    $("issues").replaceChildren();
    $("events").replaceChildren();
    $("events-more").hidden = true;
    $("people-list").replaceChildren();
    $("person-detail").replaceChildren();
    showOpenName("");
    state.scenario = null;
  }
  function renderKpis(container, kpis, defs = metricDefs) {
    container.replaceChildren();
    for (const d of defs) {
      const card = el("div", undefined, "kpi");
      card.dataset.metric = d.key;
      const image = el("span", undefined, "kpi-icon");
      image.append(uiIcon(d.displayIcon));
      card.append(image);
      card.title = d.note;
      const body = el("div");
      body.append(el("small", d.displayLabel));
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
  // The report's summary of the branch's whole computed range (#612, the core's
  // `history?kind=summary`): the three cards and every indicator's mean and greatest value over the
  // range, not one minute's; `summary` null while none is read.
  function renderReportSummary(summary) {
    const metrics = summary?.metrics || {};
    const scaled = (key, field) => {
      const d = metricDefs.find((metric) => metric.key === key);
      const value = metrics[key]?.[field];
      return typeof value === "number" ? value * d.factor : null;
    };
    const at = (key) => (typeof metrics[key]?.max_minute === "number" ? `（${time(metrics[key].max_minute)}）` : "");
    const support = "unresolved_support_request_count";
    const cards = [
      ["stress_mean", "避難者の負担", "people", `平均 ${num(scaled("stress_mean", "mean"))} /100`, `最大 ${num(scaled("stress_mean", "max"))} /100${at("stress_mean")}`],
      [support, "対応待ちの相談", "alert", `最大 ${num(scaled(support, "max"))} 件`, `対応待ちがあった時間：${lasting(metrics[support]?.positive_minutes)}`],
      ["staff_load_ratio", "スタッフの忙しさ", "staff", `平均 ${num(scaled("staff_load_ratio", "mean"))} %`, `最大 ${num(scaled("staff_load_ratio", "max"))} %${at("staff_load_ratio")}`],
    ];
    $("report-highlights").replaceChildren(...cards.map(([key, label, icon, main, detail]) => {
      const card = el("div", undefined, `highlight-card highlight-${key}`);
      const mark = el("span", undefined, "highlight-icon");
      mark.append(uiIcon(icon));
      const body = el("div");
      body.append(el("strong", label), el("span", main), el("small", detail), el("small", metricDefs.find((metric) => metric.key === key).note));
      card.append(mark, body);
      return card;
    }));
    $("report-kpis").replaceChildren(
      el("span", summary ? "計算した範囲の平均と最大" : "指標", "kpi-strip-when"),
      ...metricDefs.filter((d) => !summary || metrics[d.key]).map((d) => {
        const item = el("span", undefined, "kpi-strip-item");
        item.title = d.note;
        item.append(el("small", d.label), el("strong", num(scaled(d.key, "mean"))), el("em", d.unit), el("small", `最大 ${num(scaled(d.key, "max"))}`));
        return item;
      }),
    );
    $("report-frame-context").textContent = !summary
      ? "シナリオを開くと記録を表示します。"
      : summary.sample_count > 0
        ? `${time(summary.oldest_minute)}〜${time(summary.newest_minute)}の記録の全体（計算した範囲、1分ごと）`
        : "この分岐には指標の記録がありません。";
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
      example: !(app.ownId && state.project === app.ownId),
    });
    showOpenName((state.run && names.get(state.run)) || "");
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
    if (frame.kpis.unresolved_support_request_count > 0)
      issues.push([
        "対応待ちの支援要請",
        `${frame.kpis.unresolved_support_request_count}件がスタッフの対応待ちです。`,
      ]);
    if (frame.kpis.toilet_queue_length > 0)
      issues.push([
        "トイレの待機列",
        `${frame.kpis.toilet_queue_length}人が待機。直近24時間の平均待ち時間 ${num(frame.kpis.toilet_wait_mean_24h_minutes)}分。`,
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
        ? issues.map(([a, b]) => {
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
    if (clickable) $("events-more").hidden = true;
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
    const recent = [...grouped.values()].slice(-12);
    if (clickable) {
      recent.reverse();
      // Feature recorded problems and completed responses, with one card per kind first.
      const ordered = [...recent].sort((a, b) => Number(eventTone(b) !== "record") - Number(eventTone(a) !== "record"));
      const types = new Set();
      const featured = ordered.filter((event) => {
        if (types.has(event.event_type)) return false;
        types.add(event.event_type);
        return true;
      });
      const unique = new Set(featured);
      const brief = [...featured, ...ordered.filter((event) => !unique.has(event))].slice(0, 3);
      const briefSet = new Set(brief);
      recent.splice(0, recent.length, ...brief, ...recent.filter((e) => !briefSet.has(e)));
      $("events-more").hidden = recent.length <= 3;
    }
    for (const e of recent) {
      const node = el(
        clickable ? "button" : "div",
        undefined,
        `event-item event-${eventTone(e)}`,
      );
      const status = {
        applied: { resolved: "対応完了", unresolved: "課題の記録", record: "経過の記録" }[eventTone(e)],
        suppressed: "実行を見送り", rejected: "却下", failed: "失敗", scheduled: "予定",
      }[e.status] || e.status;
      node.append(
        el("span", `${time(e.minute)} · ${status}`, "event-time"),
        el("b", `${eventNames[e.event_type] || e.event_type}${e.count > 1 ? `（${e.count}件）` : ""}`),
      );
      node.title = `${e.event_id} / source: ${e.source_id || "未記録"}`;
      if (clickable) {
        node.disabled = e.minute < lower() || e.minute > upper();
        node.append(el("span", node.disabled ? "この分岐では移動できません" : "この時刻へ →", "event-action"));
        node.addEventListener("click", () =>
          action("イベントの時刻を取得しています…", async () => {
            await seek(e.minute);
            if (state.page === "simulation") {
              scrollTo(0, 0);
              $("simulation-map-heading").focus({ preventScroll: true });
            }
          }),
        );
      }
      box.append(node);
    }
    // The report keeps its compact, chronological log; the simulation shows the brief list.
    if (!clickable) box.scrollTop = box.scrollHeight;
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
  Object.assign(app, {
    stopPlayback, openRun, openRunInPlace, clearRunView, renderKpis, renderReportSummary, refreshRuns,
    refreshRun, seek, renderFrame, renderEvents, motionFraction, togglePlayback, computeToEnd,
    cancelCompute,
  });
})();
