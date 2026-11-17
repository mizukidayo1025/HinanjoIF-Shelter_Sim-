/* The evacuees and staff of the shown frame, and one person's dialog. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const { state, $, json, zoneNames, activityNames, time, num, el, uiIcon, drawAll } = app;
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
    if (state.selectedGroup?.length) {
      const all = el("button", "全員の一覧に戻る", "person-chip");
      all.addEventListener("click", () => {
        state.selectedGroup = [];
        state.selected = null;
        renderPeople();
        drawAll();
      });
      $("people-list").append(all);
    }
    for (const p of group) {
      const button = el("button", p.id, "person-chip");
      button.classList.toggle("selected", p.id === state.selected);
      button.addEventListener("click", () =>
        selectPerson(p.id, state.selectedGroup, anchorOf(button)),
      );
      $("people-list").append(button);
    }
    if (!state.selectedGroup?.length && f.staff.length) {
      $("people-list").append(el("span", "スタッフ", "group-caption"));
      for (const person of f.staff) {
        const button = el("button", person.id, "person-chip");
        button.classList.toggle("selected", person.id === state.selected);
        button.addEventListener("click", () => selectPerson(person.id, [], anchorOf(button)));
        $("people-list").append(button);
      }
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
  Object.assign(app, { personSummary, renderPeople, selectPerson });
})();
