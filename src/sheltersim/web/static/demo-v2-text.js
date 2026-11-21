/* The screen's words and number formats, and the helpers that make its elements. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
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
  const metricDefs = [
    {
      key: "stress_mean",
      displayLabel: "避難者の負担",
      displayIcon: "people",
      label: "平均ストレス",
      unit: "/100",
      factor: 100,
      icon: "♟",
      note: "避難者全員のストレスの平均（0〜100、高いほどつらい）",
    },
    {
      key: "toilet_wait_mean_24h_minutes",
      displayLabel: "トイレ待ち時間",
      displayIcon: "clock",
      label: "平均トイレ待ち時間（24時間）",
      unit: "分",
      factor: 1,
      icon: "◷",
      note: "直近24時間にトイレを使い始めた人の、待った時間の平均",
    },
    {
      key: "staff_load_ratio",
      displayLabel: "スタッフの忙しさ",
      displayIcon: "staff",
      label: "スタッフ負荷",
      unit: "%",
      factor: 100,
      icon: "♟",
      note: "スタッフのうち、いま対応中の人の割合",
    },
    {
      key: "unresolved_support_request_count",
      displayLabel: "対応待ちの相談",
      displayIcon: "alert",
      label: "未解決の支援要請",
      unit: "件",
      factor: 1,
      icon: "!",
      note: "助けを求めて、まだ対応されていない件数",
    },
    {
      key: "population",
      displayLabel: "避難者数",
      displayIcon: "people",
      label: "現在の避難者数",
      unit: "人",
      factor: 1,
      icon: "♟",
      note: "この時刻に避難所にいる人数",
    },
    {
      key: "toilet_queue_length",
      displayLabel: "トイレ待ち",
      displayIcon: "queue",
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
  // A length of time in minutes, as days, hours and minutes (#612): "計 2日5時間", "なし", "—".
  const lasting = (minutes) => {
    if (typeof minutes !== "number") return "—";
    if (minutes === 0) return "なし";
    const parts = [[Math.floor(minutes / 1440), "日"], [Math.floor((minutes % 1440) / 60), "時間"], [minutes % 60, "分"]];
    return `計 ${parts.filter(([count]) => count > 0).map(([count, unit]) => `${count}${unit}`).join("")}`;
  };
  Object.assign(app, {
    $, enc, json, llmModeLabel, metricDefs, zoneNames, eventNames, eventTone, activityNames,
    disasterName, supportModeName, time, shortBranch, runStatusLabel, num, el, uiIcon, lasting,
  });
})();
