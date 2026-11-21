/* Independent backup UI. WorldState and persisted API artifacts are authoritative.
   This file, loaded last, connects the page's controls to what the other demo-v2-*.js
   files define, and opens the page. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const {
    state, $, num, el, uiIcon, api, note, computeTargets, loading, action, on, show,
    controls, hits, drawAll, keepPassphrase, showApi, refreshApi, applyLlm, enableApi, openExamples,
    newScenario, invalidateScenario, previewScenario, approveScenario, stopPlayback, renderKpis,
    renderKpiStrip, refreshRuns, refreshRun, seek, renderFrame, togglePlayback, computeToEnd,
    cancelCompute, personSummary, renderPeople, selectPerson, invalidateIntervention, comparing,
    pickForkTime, previewIntervention, applyIntervention, loadComparison, loadMultiCompare,
    resetChat, loadReport, loadHistory, showInterventionForm, askQuestion, saveReport, copyScenario,
  } = app;
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
  $("header-api").addEventListener("click", () => show("home"));
  $("home-api-enable").addEventListener("click", () => {
    $("api-dialog-note").textContent = "";
    $("api-dialog").showModal();
    $("api-passphrase").focus();
  });
  $("home-api-disable").addEventListener("click", () => {
    keepPassphrase("");
    showApi(null, "解除しました。");
    // During a public demo the open mode turns it ON again (#601).
    void refreshApi();
  });
  $("api-form").addEventListener("submit", (event) => void enableApi(event));
  $("api-dialog-close").addEventListener("click", () => $("api-dialog").close());
  $("examples-dialog-close").addEventListener("click", () => {
    if (!app.fetching) $("examples-dialog").close();
  });
  // While an example is fetched, Esc does not close the dialog (its progress would be lost).
  $("examples-dialog").addEventListener("cancel", (event) => {
    if (app.fetching) event.preventDefault();
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
  $("intervention-builder").addEventListener("input", invalidateIntervention);
  // Another operator asks for other targets and amounts.
  $("intervention-op").addEventListener("change", controls);
  on("intervention-preview", () => previewIntervention("builder"));
  on("intervention-preview-text", () => previewIntervention("text"));
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
    $("intervention-op").focus({ preventScroll: true });
  });
  $("events-more").addEventListener("click", () => {
    const open = $("events").classList.toggle("expanded");
    $("events-more").setAttribute("aria-expanded", String(open));
    $("events-more").textContent = open ? "記録を折りたたむ" : "記録をもっと見る";
  });
  $("intervention-toggle").addEventListener("click", () => {
    showInterventionForm($("intervention-panel").hidden);
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
  for (const id of ["branch-select", "report-branch"]) {
    $(id).addEventListener("change", () =>
      action("選択した分岐を取得しています…", () => loading(async () => {
        stopPlayback();
        state.branch = $(id).value;
        state.scope++;
        state.context = null;
        state.selected = null;
        state.selectedGroup = [];
        state.history = [];
        state.overlayHistory = [];
        state.report = null;
        if (id === "report-branch") state.comparison = null;
        resetChat();
        invalidateIntervention();
        await refreshRun();
        if (id === "report-branch") await loadReport();
      })),
    );
  }
  $("report-overlay").addEventListener("change", () =>
    action("比較する分岐の推移を読み込んでいます…", loadHistory),
  );
  new ResizeObserver(() => requestAnimationFrame(drawAll)).observe(
    document.querySelector("main"),
  );
  async function init() {
    // The top shows whether the AI's features are ON (#581), whichever page opens; until it is
    // known, the features that need it are off (#580).
    applyLlm();
    void refreshApi();
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
  Object.assign(app, { loadFailure });
})();
