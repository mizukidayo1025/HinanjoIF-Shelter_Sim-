/* The scenario page: the conditions (and, with the API on, a text), their preview and approval. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const {
    ownProject, state, $, json, llmModeLabel, disasterName, num, el, ownBase, api, ensureProject,
    show, controls, drawMap, drawPopulation,
  } = app;
  const {
    stopPlayback, clearRunView, refreshRuns, refreshRun, invalidateIntervention, resetChat,
  } = app.later;
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
  // The conditions are the form's: 自治体・災害種別・避難者数 are required there (#583). A text
  // is read only with the API on, by the AI (its fieldset is disabled while off, and its value
  // is then not sent at all): the text with the form's basic conditions is read first, and its
  // request completed by the form's fields, which win.
  async function previewScenario() {
    if (!$("scenario-form").reportValidity()) return;
    const version = state.inputVersion;
    let req = scenarioRequest();
    const text = state.llm?.on === true ? $("scenario-text").value.trim() : "";
    await ensureProject(ownProject());
    if (text) {
      const hints = [
        `${req.municipality}で`,
        req.disaster,
        `${req.evacuees}人`,
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
          `文章を読み取れませんでした（不足項目：${(extracted.missing_fields || []).join("、")}）。`,
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
    const result = await api(`${ownBase()}/scenario-preview`, { text, request: req });
    if (version !== state.inputVersion)
      throw new Error(
        "入力が変更されました。最新の条件で再プレビューしてください。",
      );
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
    // Made from the form alone, or with a text the AI read (#421, #583): whether it read it.
    const usage = p.llm_usage,
      source = s?.provenance?.scenario_input?.extraction_source,
      read =
        source === "structured_form"
          ? ["項目から作成"]
          : [
              llmModeLabel(usage?.mode),
              source === "prepare_llm"
                ? `AIが文を読み取りました（約$${Number(usage.spent_usd).toFixed(3)}）`
                : "AIを使えず、文に明示された値だけで読み取りました",
            ];
    $("preview-mode").textContent = [...read, "補完値を要確認"].join(" · ");
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
  Object.assign(app, {
    newScenario, invalidateScenario, previewScenario, sourceRows, approveScenario,
  });
})();
