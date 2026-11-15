/* Branches: the intervention that makes one, and the comparison page. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const {
    state, reference, $, enc, json, metricDefs, zoneNames, supportModeName, time, shortBranch, num,
    el, runBase, branchBase, api, note, loading, action, show, controls, drawAll, renderKpis,
    refreshRun, seek, computeToEnd,
  } = app;
  const { resetChat } = app.later;
  function invalidateIntervention() {
    state.intervention = null;
    $("intervention-check").checked = false;
    $("intervention-check").disabled = true;
    $("intervention-result").replaceChildren();
    $("intervention-apply").disabled = true;
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
  Object.assign(app, {
    invalidateIntervention, comparing, renderForkPicker, pickForkTime, previewIntervention,
    applyIntervention, pairIds, loadComparison, openComparison, renderMultiChoices, multiChosen,
    loadMultiCompare,
  });
})();
