/* The report page: the indicators over time, the report and the questions about it. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const {
    state, reference, lower, upper, $, json, llmModeLabel, time, num, el, uiIcon, runBase,
    branchBase, api, note, drawHistory, drawAll, renderEvents, personSummary, pairIds,
  } = app;
  const { printReport } = app.later;
  const chatWelcome = $("chat").firstElementChild.cloneNode(true);
  function resetChat() {
    $("chat").replaceChildren(chatWelcome.cloneNode(true));
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
    // Opus's analysis only where it is chosen and the API is ON (#582): else no LLM is asked.
    const analysis = state.llm?.on === true && $("report-analysis").checked;
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
    note(
      analysis
        ? "レポートとOpusの考察を作っています。保存済みのレポートがあれば再利用します…"
        : "レポートを作っています（Opusの考察なし）…",
      "loading",
    );
    let report;
    try {
      // The report's data, facts and, where asked, the checked analysis (#470), laid out on the
      // print page.
      report = await api(`${runBase()}/pdf-report`, { branch_ids: branchIds, minute, analysis });
    } finally {
      button.removeAttribute("aria-busy");
    }
    note("レポートを開きました。印刷画面で「PDFに保存」を選ぶと、PDFとして保存できます。", "success");
    printReport(report);
  }
  Object.assign(app, { resetChat, loadReport, askQuestion, saveReport });
})();
