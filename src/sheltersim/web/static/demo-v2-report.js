/* The report page: the indicators over time, the report and the questions about it. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const {
    state, reference, duration, shortBranch, $, json, llmModeLabel, time, num, el, uiIcon, lasting, runBase,
    branchBase, api, note, drawHistory, drawAll, renderEvents, personSummary, pairIds, renderReportSummary,
  } = app;
  const { printReport } = app.later;
  const chatWelcome = $("chat").firstElementChild.cloneNode(true);
  function resetChat() {
    $("chat").replaceChildren(chatWelcome.cloneNode(true));
  }
  function syncReportBranches() {
    const branches = (state.data?.branches || []).filter((branch) =>
      branch.branch_id === state.branch || branch.computed_until >= duration(),
    );
    const options = (excludeCurrent) => branches
      .filter((branch) => !excludeCurrent || branch.branch_id !== state.branch)
      .map((branch) => {
        const option = el("option", shortBranch(branch.branch_id));
        option.value = branch.branch_id;
        return option;
      });
    const overlay = $("report-overlay"), previous = overlay.value;
    $("report-branch").replaceChildren(...options(false));
    $("report-branch").value = state.branch;
    overlay.replaceChildren(el("option", "重ねない"), ...options(true));
    overlay.options[0].value = "";
    overlay.value = [...overlay.options].some((o) => o.value === previous) ? previous : "";
  }
  async function fetchHistory(branch, start, end, step) {
    const params = new URLSearchParams({ kind: "kpis", start, end, step, limit: "100" });
    const rows = [];
    let cursor = null;
    do {
      if (cursor) params.set("cursor", cursor);
      const page = await api(`${branchBase(branch)}/history?${params}`);
      rows.push(...page.items);
      cursor = page.next_cursor;
    } while (cursor && rows.length < 1000);
    return rows;
  }
  let historyRequest = 0;
  async function loadHistory() {
    const request = ++historyRequest, scope = state.scope, branch = state.branch;
    const overlay = $("report-overlay").value;
    const metric = $("chart-metric");
    metric.options[0].disabled = Boolean(overlay);
    if (overlay && metric.value === "overview") metric.value = "stress_mean";
    const series = [branch, overlay].filter(Boolean).map(reference);
    const start = Math.max(...series.map((ref) => ref.fork_minute));
    const end = Math.min(...series.map((ref) => ref.computed_until));
    const step = Math.max(1, Math.ceil((end - start) / 90));
    state.history = [];
    state.overlayHistory = [];
    $("history-note").textContent = "指標の記録を読み込んでいます…";
    drawHistory();
    const rows = start < end
      ? await Promise.allSettled(series.map((ref) => fetchHistory(ref.branch_id, start, end, step)))
      : series.map(() => ({ status: "fulfilled", value: [] }));
    if (request !== historyRequest || scope !== state.scope || overlay !== $("report-overlay").value) return;
    state.history = rows[0].status === "fulfilled" ? rows[0].value : [];
    state.overlayHistory = rows[1]?.status === "fulfilled" ? rows[1].value : [];
    const noOverlay = overlay && !state.overlayHistory.length;
    $("history-note").textContent = rows[0].status === "rejected"
      ? "レポートの分岐の記録を取得できませんでした。結果を更新してください。"
      : overlay && start >= end
        ? "推移を重ねられる共通の期間がありません。「重ねない」に戻すと、この分岐の記録を確認できます。"
        : noOverlay
          ? "重ねる分岐の記録を表示できません。"
          : state.history.length
            ? `${time(start)}〜${time(end)}の記録（約${step}分間隔）`
            : "この分岐には指標の記録がありません。";
    drawHistory();
  }
  async function loadReport() {
    const scope = state.scope;
    syncReportBranches();
    // The summary of the branch's whole computed range (#612), not of the minute shown.
    const range = reference(state.branch);
    const whole = new URLSearchParams({ kind: "summary", start: range.fork_minute, end: range.computed_until });
    const llmEnabled = document.body.dataset.llm !== "disabled";
    const [report, evidence, llm, summary] = await Promise.all([
      api(`${branchBase()}/report`),
      api(`${runBase()}/evidence`),
      llmEnabled ? api(`${branchBase()}/llm-status`) : null,
      api(`${branchBase()}/history?${whole}`),
    ]);
    if (scope !== state.scope) return;
    state.summary = summary;
    renderReportSummary(summary);
    showQuestionScope();
    state.report = report;
    state.evidence = evidence;
    // A report the rules made from the record is the browser's every report (#610): no AI is
    // asked for it, which is not a failure.
    $("report-status").textContent =
      report.status === "rule_fallback"
          ? "記録から作成（AIなし）"
          : report.status === "unavailable"
            ? "レポート生成不可"
            : report.status === "validated"
              ? "AIレポート生成済み"
              : report.status || "取得済み";
    $("report-raw").textContent = report.markdown || json(report);
    $("report-evidence").textContent = json(llmEnabled ? { evidence, llm } : { evidence });
    $("report-summary").replaceChildren();
    // What the record shows over the branch's whole computed range (#610, the summary of #612), not
    // at the minute shown.
    const metrics = summary.metrics || {};
    const waited = (key) => (metrics[key]?.positive_minutes ?? 0) > 0;
    const recorded = summary.sample_count > 0
      ? `${time(summary.oldest_minute)}〜${time(summary.newest_minute)}の全体で、避難者の負担は平均 ${num(metrics.stress_mean?.mean * 100)}/100（最大 ${num(metrics.stress_mean?.max * 100)}）。トイレの列は最大 ${num(metrics.toilet_queue_length?.max)}人で、列があった時間は${lasting(metrics.toilet_queue_length?.positive_minutes)}。対応待ちの相談は最大 ${num(metrics.unresolved_support_request_count?.max)}件で、対応待ちがあった時間は${lasting(metrics.unresolved_support_request_count?.positive_minutes)}。`
      : "この分岐には指標の記録がありません。";
    for (const [title, text, wide] of [
      ["記録から確認できること", recorded, true],
      [
        "課題と次の検討",
        waited("unresolved_support_request_count")
          ? "対応待ちの相談が続いた時間帯の、支援要請とスタッフの資格・稼働の記録を確認し、介入を分岐比較で検討してください。"
          : "記録された事象を確認し、別の条件での結果と比較してください。",
      ],
      [
        "解釈上の限界",
        "これは合成人物とモデル内の試算です。実測で校正された予測ではありません。感染症リスク・情報到達率など未提供の指標は表示しません。",
      ],
    ]) {
      const box = el("div", undefined, wide ? "report-finding finding-wide" : "report-finding");
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
      $("report-summary").insertBefore(box, $("report-summary").children[1]);
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
    if (llmEnabled) await loadConversations();
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
  // The minute a question is about (#611): the end of the branch's computed range, so that the
  // answer reads the record of the whole Run, not the minute shown (a Run opens at minute 0); a
  // person chosen is asked about at the minute shown, where the frame has them.
  function questionMinute() {
    const person = state.selected && state.frame?.people.some((p) => p.id === state.selected);
    return person ? state.minute : reference(state.branch).computed_until;
  }
  // What the questions are answered from, under the panel's title.
  function showQuestionScope() {
    const range = reference(state.branch);
    $("question-scope").textContent = state.selected && state.frame?.people.some((p) => p.id === state.selected)
      ? `選んだ人物（${state.selected}）について、表示中の時刻（${time(state.minute)}）の記録で答えます。`
      : `${time(range.fork_minute)}〜${time(range.computed_until)}の記録をもとに答えます。`;
  }
  async function askQuestion() {
    const question = $("question").value.trim();
    if (!question) throw new Error("質問を入力してください。");
    const version = ++state.chatVersion,
      scope = state.scope,
      minute = questionMinute(),
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
      minute !== questionMinute() ||
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
    // The comparison's minute, else the end of the branch's computed range (#611): the report and
    // Opus's analysis are of the whole Run, not of the minute shown.
    const branchIds = state.comparison ? pairIds() : [state.branch],
      minute = state.comparison ? state.comparison.minute : reference(state.branch).computed_until,
      button = $("save-report");
    // Opus's analysis only where it is chosen and the API is ON (#582): else no LLM is asked.
    const analysis = state.llm?.on === true && $("report-analysis").checked;
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
    note(
      analysis
        ? "レポートとOpusの考察を作っています。保存済みのレポートがあれば再利用します…"
        : document.body.dataset.llm === "disabled"
          ? "レポートを作っています…"
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
  Object.assign(app, { resetChat, loadReport, loadHistory, askQuestion, saveReport });
})();
