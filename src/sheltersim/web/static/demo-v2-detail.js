/* The records page: the Run's record, models, sources and the LLM's use. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const {
    state, $, json, llmModeLabel, disasterName, time, shortBranch, runStatusLabel, num, el, base,
    runBase, branchBase, api, optional, sourceRows, openRunInPlace, refreshRuns,
  } = app;
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
    const own = Boolean(app.ownId) && state.project === app.ownId;
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
  Object.assign(app, { copyScenario, loadDetail });
})();
