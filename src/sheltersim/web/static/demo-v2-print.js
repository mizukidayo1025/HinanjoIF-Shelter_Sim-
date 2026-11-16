/* The report's print page (#470): the answer of `POST runs/{run}/pdf-report` (the core's data,
   facts and the checked Opus analysis, `PdfAnalysis.report`) laid out as the shared document the
   PDF used to be (src/sheltersim/web/pdf_render.py before #470), shown over the page and printed
   with the browser's print (「PDFに保存」 there). Every string of the answer is set as text, never as
   markup. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const { el } = app;

  // key, label, displayed unit, scale (the core's `pdf::METRICS`).
  const METRICS = [
    ["stress_mean", "平均ストレス", "/100", 100],
    ["toilet_wait_max_minutes", "最大トイレ待ち時間", "分", 1],
    ["staff_load_ratio", "スタッフ稼働率", "%", 100],
    ["unresolved_support_request_count", "未解決の支援要請", "件", 1],
    ["toilet_queue_length", "トイレ待ち人数", "人", 1],
    ["stress_high_count", "高ストレス者数", "人", 1],
  ];
  const EVENT_LABELS = {
    facility_outage: "施設停止",
    facility_recovery: "施設復旧",
    water_outage: "断水",
    water_restored: "給水復旧",
    resource_delivery: "物資到着",
    delivery_delayed: "配送遅延",
    staff_unavailable: "スタッフ稼働停止",
    staff_shift_change: "スタッフ交代",
    power_outage: "停電",
    power_restored: "電力復旧",
    equipment_failure: "設備故障",
    equipment_repaired: "設備修理",
    extreme_heat: "高温",
    extreme_cold: "低温",
    "branch-add-toilet": "トイレの追加",
    "branch-add-staff": "スタッフの追加",
    effect_program_apply: "介入プログラムの適用",
  };
  const OPERATORS = {
    add_toilet: "トイレの追加",
    add_staff: "スタッフの追加",
    add_facility: "施設の追加",
    add_resource: "物資の追加",
    set_facility_availability: "施設の稼働条件変更",
    facility_outage: "施設の停止",
    set_staff_availability: "スタッフの稼働条件変更",
    set_ambient_noise: "騒音条件の変更",
    set_privacy: "プライバシー条件の変更",
    set_temperature: "温度条件の変更",
    set_lighting: "照明条件の変更",
    set_ventilation: "換気条件の変更",
    modify_demand_rate: "需要発生率の変更",
    move_facility: "施設の移動",
    set_route_access: "経路の通行条件変更",
    transfer_equipment: "設備の移設",
    add_evacuees: "避難者の追加",
    depart_evacuees: "避難者の退出",
  };
  const STATUS = { applied: "適用済み", failed: "失敗", suppressed: "抑制", scheduled: "予定（未実行）" };
  const PARAMETERS = {
    capacity: "容量",
    working_units: "稼働数",
    count: "数",
    quantity: "数量",
    water_ml: "水量（ml）",
    food_meal_units: "食数",
    available: "稼働可能",
    multiplier: "倍率",
    ratio: "割合",
    facility_id: "施設ID",
    zone_id: "ゾーンID",
    target_id: "対象ID",
    end_minute: "終了予定分",
    delta: "変化量",
    resource_id: "物資",
    household_size: "世帯人数",
    include_initial: "初期の避難者を含む",
    to_minute: "変更後の到着予定分",
    staff_id: "スタッフID",
    until_minute: "終了予定分",
  };
  const LIMITS = [
    "本資料は避難所運営条件の検討用です。実際の避難者の健康・心理や運営成果を予測・診断するものではありません。",
    "数値係数の実測校正状況はこのレポートでは未取得です。背景資料の存在は係数の校正根拠を意味しません。仮定IDと出典台帳を照合してください。",
    "単一seedの観測と比較です。複数seed、異なる需要・設備故障・スタッフ条件で追加検証し、適用可能な集計実測データで校正を検討してください。",
    "未計算期間、未取得指標、欠測区間、未選択の比較は評価対象外です。実建物の形状を推測しないため施設マップは掲載していません。",
    "AI文の根拠参照・数値混入・断定表現を機械検証していますが、自由記述の意味の正確性を完全に証明するものではありません。共有前に担当者が記録と照合してください。",
  ];
  const label = (table, key) => table[key] ?? key;
  const isNumber = (value) => typeof value === "number" && Number.isFinite(value);

  /// A recorded value scaled, with thousands separators and at most 4 decimals ("未取得" if none).
  function fmt(value, factor = 1) {
    if (!isNumber(value)) return "未取得";
    const scaled = value * factor;
    if (scaled === 0) return "0";
    const text = scaled.toLocaleString("en-US", { minimumFractionDigits: 4, maximumFractionDigits: 4 });
    return text.replace(/0+$/, "").replace(/\.$/, "");
  }
  /// A minute as "N日目 HH:MM (m分)".
  function clock(minute) {
    if (minute === null || minute === undefined) return "未観測";
    const pad = (n) => String(n).padStart(2, "0");
    return `${Math.floor(minute / 1440) + 1}日目 ${pad(Math.floor(minute / 60) % 24)}:${pad(minute % 60)} (${minute}分)`;
  }
  function parameters(values) {
    const parts = [];
    for (const [key, value] of Object.entries(values ?? {})) {
      if (key === "parameters" && value && typeof value === "object") parts.push(parameters(value));
      else if (key !== "op" && key !== "kind") parts.push(`${label(PARAMETERS, key)}: ${typeof value === "object" ? JSON.stringify(value) : value}`);
    }
    return parts.join(" / ") || "追加の数値条件は未取得";
  }
  function interventionText(item) {
    const name = OPERATORS[item.operator] ?? label(EVENT_LABELS, item.type);
    return `${name}\n${parameters(item.details)}\n${label(STATUS, item.status)} / event_id: ${item.event_id}`;
  }
  function factText(value) {
    if ("calibration" in value)
      return `${value.calibration} 対象範囲 ${value.range[0]}〜${value.range[1]}分。${value.partial ? "途中結果。" : "計算期間の終了時点。"}`;
    if ("population" in value) return `登録避難者 ${value.population}人 / スタッフ ${value.staff_count}人 / 予定期間 ${value.duration_minutes}分`;
    if ("delta" in value)
      return `${value.label}: A ${fmt(value.baseline)} / B ${fmt(value.candidate)} / 差 ${fmt(value.delta)} ${value.unit}（${value.assessment}）`;
    if ("metric" in value) return `${value.label}: ${fmt(value.value)} ${value.unit} / ${clock(value.minute)}`;
    if ("criterion" in value) {
      const metric = METRICS.find(([key]) => value.criterion.startsWith(key));
      return `${metric ? metric[1] : "指標"}が正：初回 ${clock(value.first_observed_minute)} / 観測継続 ${value.positive_interval_minutes}分 / 欠測 ${value.missing_minutes}分`;
    }
    if ("recorded_commands" in value) return `介入コマンド ${value.recorded_commands}件 / 対象範囲の介入実行記録 ${value.recorded_intervention_events}件`;
    if ("operator" in value)
      return `${label(OPERATORS, value.operator)} / ${parameters(value.parameters)} / 予定 ${clock(value.scheduled_minute)} / ${label(STATUS, value.status)}`;
    return `${label(EVENT_LABELS, value.type)} / ${clock(value.minute)} / ${label(STATUS, value.status)}`;
  }

  const SVG = "http://www.w3.org/2000/svg";
  function svg(tag, attributes = {}, text) {
    const node = document.createElementNS(SVG, tag);
    for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, String(value));
    if (text !== undefined) node.textContent = text;
    return node;
  }
  /// A metric over the report's range, a line per branch (A blue, B green): only consecutive
  /// minutes are joined, missing ones are not interpolated.
  function chart(data, [key, name, unit, factor]) {
    const figure = el("figure", null, "pr-chart");
    figure.append(el("figcaption", `${name} (${unit})`));
    const width = 320,
      height = 150,
      left = 40,
      bottom = 26,
      plotW = width - 50,
      plotH = height - 44;
    const [start, end] = data.range;
    const series = data.branches.map((branch) => branch.series);
    const values = series.flatMap((rows) => rows.map((row) => row[key]).filter(isNumber).map((value) => value * factor));
    if (values.length === 0) {
      figure.append(el("p", "時系列は未取得", "pr-small"));
      return figure;
    }
    let low = Math.min(0, ...values),
      high = Math.max(...values);
    if (high <= low) high = low + 1;
    const x = (minute) => left + ((minute - start) / Math.max(1, end - start)) * plotW;
    const y = (value) => height - bottom - ((value - low) / (high - low)) * plotH;
    const drawing = svg("svg", { viewBox: `0 0 ${width} ${height}`, role: "img", "aria-label": `${name}の推移` });
    for (let index = 0; index < 3; index += 1) {
      const level = low + (index * (high - low)) / 2;
      drawing.append(svg("line", { x1: left, x2: left + plotW, y1: y(level), y2: y(level), class: "pr-grid" }));
      drawing.append(svg("text", { x: left - 4, y: y(level) + 3, "text-anchor": "end" }, fmt(level)));
    }
    for (const minute of [...new Set([start, Math.floor((start + end) / 2), end])].sort((a, b) => a - b)) {
      drawing.append(svg("text", { x: x(minute), y: height - bottom + 13, "text-anchor": "middle" }, String(minute)));
    }
    drawing.append(svg("text", { x: width - 4, y: height - 2, "text-anchor": "end" }, "経過分"));
    series.forEach((rows, index) => {
      const cls = index === 0 ? "pr-a" : "pr-b";
      let points = [];
      let previous = null;
      const flush = () => {
        if (points.length > 1) drawing.append(svg("polyline", { points: points.join(" "), class: cls }));
        else if (points.length === 1) {
          const [px, py] = points[0].split(",").map(Number);
          drawing.append(svg("line", { x1: px - 1, x2: px + 1, y1: py, y2: py, class: `${cls} pr-dot` }));
        }
        points = [];
      };
      for (const row of rows) {
        const value = row[key];
        if (!isNumber(value)) {
          flush();
          previous = null;
          continue;
        }
        if (previous !== null && row.minute_index !== previous + 1) flush();
        points.push(`${x(row.minute_index).toFixed(2)},${y(value * factor).toFixed(2)}`);
        previous = row.minute_index;
      }
      flush();
    });
    figure.append(drawing);
    return figure;
  }

  /// The report as an <article>: its sections in the order of the former PDF.
  function compose(report) {
    const { data, analysis, facts } = report;
    const branches = data.branches;
    const alias = Object.fromEntries(branches.map((branch, index) => [branch.branch_id, String.fromCharCode(65 + index)]));
    const c = data.conditions;
    const article = el("article", null, "pr-document");
    let section = null;
    const add = (node) => (section ?? article).append(node);
    const p = (text, cls) => add(el("p", text, cls));
    const sub = (text) => add(el("h3", text));
    const heading = (text) => {
      section = el("section", null, "pr-section");
      section.append(el("h2", text));
      article.append(section);
    };
    // `widths`: each column's share of the line (%), as the former PDF's (equal shares without).
    const table = (rows, { compact = false, widths = null } = {}) => {
      const node = el("table", null, `pr-table${compact ? " pr-compact" : ""}${widths ? " pr-fixed" : ""}`);
      if (widths) {
        const columns = el("colgroup");
        for (const width of widths) {
          const column = el("col");
          column.style.width = `${width}%`;
          columns.append(column);
        }
        node.append(columns);
      }
      const head = el("thead");
      const headRow = el("tr");
      for (const cell of rows[0]) headRow.append(el("th", cell));
      head.append(headRow);
      const body = el("tbody");
      for (const row of rows.slice(1)) {
        const tr = el("tr");
        for (const cell of row) tr.append(el("td", cell));
        body.append(tr);
      }
      node.append(head, body);
      add(node);
    };
    const prose = (key, title) => {
      sub(title);
      (analysis.sections[key] ?? []).forEach((paragraph, index) => {
        const prefix = key === "proposals" ? (index === 0 ? "優先案：" : index === 1 ? "代替案：" : "追加案：") : "";
        const block = el("div", null, "pr-prose");
        block.append(el("p", prefix + paragraph.text), el("p", `根拠: ${paragraph.refs.join(", ")}`, "pr-small"));
        add(block);
      });
    };

    article.append(el("p", "避難所IF！", "pr-title"));
    heading("01  エグゼクティブサマリー");
    sub("避難所運営シミュレーション 分析レポート");
    p("目的：シミュレーションの条件・観測結果と検討課題を共有し、設備や人員配置、運営方針の比較検討に活用するための資料です。");
    p(
      `対象：${branches.length > 1 ? "A/B比較" : "単一シナリオ"} | 評価時刻：${clock(data.range[1])} | ${
        data.partial ? "途中結果（以降は未計算・評価対象外）" : "計算期間の終了時点"
      }`,
    );
    table([
      ["観測KPI", ...branches.map((b) => alias[b.branch_id]), "単位"],
      ...METRICS.map(([key, name, unit, factor]) => [name, ...branches.map((b) => fmt(b.final?.[key], factor)), unit]),
    ]);
    for (const b of branches) p(`${alias[b.branch_id]} = ${b.branch_id} / 評価時点の避難者数 ${b.population}人`, "pr-small");
    prose("executive", "AIの要旨・着眼点（仮説）");
    sub("重要な読み方");
    p("表とグラフは記録に基づく観測値です。AIの解釈・原因候補は仮説、施策は今後の検討提案として掲載します。指標の低下だけで実運用の有効性を判断することはできません。");

    heading("02  シミュレーション条件");
    const stock = c.initial_stock;
    table([
      ["設定項目", "記録値"],
      ["登録避難者数 / 初期スタッフ数", `${c.population}人 / ${c.staff_count}人`],
      ["予定期間 / 評価対象範囲", `${c.duration_minutes}分 / ${data.range[0]}〜${data.range[1]}分`],
      ["スタッフ技能", c.staff_skills.join(", ") || "未取得"],
      ["初期物資", stock ? `水 ${stock.water_ml} ml / 食料 ${stock.food_meal_units} 食` : "初期物資量は未取得"],
      ["採用モデル・版", Object.entries(data.model_versions ?? {}).map(([key, value]) => `${key}: ${value}`).join(" / ")],
      ["Scenario / seed", `${data.scenario_id} v${data.scenario_version} / ${data.seed}`],
    ], { widths: [28, 72] });
    sub("施設構成・設備（開始時の設定）");
    if (c.facilities.length > 0) {
      table([
        ["施設ID・種別", "所属ゾーン", "容量 / 稼働数"],
        ...c.facilities.slice(0, 12).map((f) => [`${f.id} / ${f.kind}`, f.zone_id, `${f.capacity} / ${f.working_units ?? f.capacity}`]),
      ]);
    } else p("施設設定は未取得。");
    p(`ゾーン数 ${c.zones.length} / 施設数 ${c.facilities.length}。${c.facilities.length > 12 ? "施設表は先頭12件を掲載。" : ""}`, "pr-small");
    p(`ゾーン：${c.zones.slice(0, 12).map((z) => `${z.name} (${z.id}, 容量 ${z.capacity ?? "未取得"})`).join(" / ")}`, "pr-small");
    sub("主な仮定・出典");
    p("施設数、初期状態、行動・心理の係数などにはScenario固有の仮定を含みます。実建物の図面・形状と現地の医療結果は評価対象外です。");
    p(`仮定ID：${c.assumption_ids.join(", ")}`, "pr-small");
    p(`出典ID：${c.source_ids.join(", ") || "未取得"}`, "pr-small");

    heading("03  結果分析：時系列と問題の継続");
    p("凡例：青=A / 緑=B（比較時）。ストレスは0〜100に換算。稼働率は割合を百分率に換算。線は連続した分記録だけを結び、欠測区間は補間しません。", "pr-small");
    const charts = el("div", null, "pr-charts");
    for (const metric of METRICS.slice(0, 4)) charts.append(chart(data, metric));
    add(charts);
    sub("問題の観測時刻・継続時間");
    const problems = [["分岐 / 判定条件", "初回観測", "観測継続時間 / 欠測"]];
    for (const b of branches) {
      for (const [key, name] of [
        ["toilet_queue_length", "トイレ待ち人数 > 0"],
        ["unresolved_support_request_count", "未解決支援要請 > 0"],
      ]) {
        const window = b.problems[key];
        problems.push([`${alias[b.branch_id]} / ${name}`, clock(window.first_observed_minute), `${window.positive_interval_minutes}分 / ${window.missing_minutes}分`]);
      }
    }
    table(problems);
    p(
      "継続時間は対象範囲の各1分区間で開始値が正かつ両端が記録済みの区間を合算。閾値は問題の診断基準ではなく表示用です。初回観測は範囲内の初回であり、真の発生時刻とは限りません。",
      "pr-small",
    );

    heading("04  介入と比較・重要イベント");
    if (data.differences.length > 0) {
      p(
        `Aを基準にB−Aを計算。同じRun・seed・モデル・評価時刻を使用。共通の起点：${data.paired ? "確認済み" : "未確認"}。系譜の一致だけで現実の因果や効果は確定しません。`,
      );
      table([
        ["KPI", "A", "B", "差分 B−A", "方向の整理"],
        ...METRICS.map(([key, name, unit, factor]) => {
          const item = data.differences[0].metrics[key];
          return [`${name} (${unit === "%" || unit === "/100" ? "pt" : unit})`, fmt(item.baseline, factor), fmt(item.candidate, factor), fmt(item.delta, factor), item.assessment];
        }),
      ], { widths: [31, 14, 14, 17, 24] });
      p("改善・悪化は当該指標の低下・上昇の方向のみ。スタッフ稼働率の差には良否を付けません。", "pr-small");
    } else p("比較対象が選択されていないため、比較と差分は評価対象外です。");
    sub("記録済み介入");
    const commands = branches.flatMap((b) => b.interventions.map((command) => [alias[b.branch_id], command]));
    if (commands.length > 0) {
      table([
        ["分岐 / 実行・予定時刻", "介入内容・結果（型付き作用・参照ID）"],
        ...commands.slice(0, 6).map(([name, command]) => [`${name} / ${clock(command.realized_minute)}\n予定 ${clock(command.scheduled_minute)}`, interventionText(command)]),
      ], { widths: [27, 73] });
      if (commands.length > 6) p(`全${commands.length}件中先頭6件を掲載。詳細は内部JSONに保存。`, "pr-small");
    } else p("記録済み介入はありません。");
    sub("重要イベント（予定ではなく実行結果）");
    const events = branches.flatMap((b) => b.events.map((event) => [alias[b.branch_id], event]));
    if (events.length > 0) {
      table([
        ["分岐・時刻", "記録イベント", "結果・参照ID"],
        ...events.slice(0, 10).map(([name, event]) => [`${name} | ${clock(event.minute)}`, label(EVENT_LABELS, event.type), `${label(STATUS, event.status)} | ${event.event_id}`]),
      ], { widths: [28, 32, 40] });
      if (events.length > 10) p(`重要イベントは全${events.length}件中、記録順の先頭10件を掲載。全件は内部JSONに保存。`, "pr-small");
    } else p("対象範囲の重要イベント記録はありません。");
    prose("interpretation", "AIによる結果の解釈（仮説）");

    heading("05  AI分析と優先的に検討すべき施策");
    p(`執筆モデル：${analysis.model_id} / 根拠付き構造化出力を検証して掲載。`, "pr-small");
    prose("hypotheses", "原因候補・運営上の課題（AIの仮説）");
    prose("proposals", "優先案・代替案（今後の検討提案）");
    sub("参照した観測根拠");
    const used = [...new Set(Object.values(analysis.sections).flatMap((paragraphs) => paragraphs.flatMap((paragraph) => paragraph.refs)))];
    table(
      [
        ["根拠ID", "検証済みの記録値・条件"],
        ...used.map((ref) => {
          let local = "";
          if (ref.includes(".E") || ref.includes(".I")) {
            const [branchPart, recordPart] = ref.split(".");
            const branch = branches[Number(branchPart.slice(1))];
            const records = recordPart.startsWith("E") ? branch.events : branch.interventions;
            local = `\nevent_id: ${records[Number(recordPart.slice(1))].event_id}`;
          }
          return [ref, factText(facts[ref]) + local];
        }),
      ],
      { compact: true, widths: [31, 69] },
    );

    heading("06  限界と今後の検討・再現情報");
    p("根拠IDの読み方：B0=A、B1=B、D1=B−A。KPIは評価時点、peakは記録内の最大値と時刻、windowは観測継続時間です。EとIは各Branchのイベント・介入の記録順の先頭12件に対応します。", "pr-small");
    prose("limitations", "追加検証が必要な事項（AIの整理）");
    sub("解釈上の限界");
    for (const text of LIMITS) p(text);
    sub("再現用ID・版");
    table([
      ["項目", "記録値"],
      ["Project / Run", `${data.project_id} / ${data.run_id}`],
      ["Scenario hash", data.scenario_hash],
      ["seed / 対象分", `${data.seed} / [${data.range.join(", ")}]`],
      ["レポート / プロンプト版", `${data.report_version} / ${analysis.prompt_version}`],
      ["AI入力hash", analysis.input_hash],
      ...branches.flatMap((b, index) => [
        [`${alias[b.branch_id]} Branch / state`, `${b.branch_id} / ${b.state_hash}`],
        ["Checkpoint hash", data.heads[index].checkpoint_hash],
      ]),
    ], { widths: [28, 72] });
    p("再現には保存済みScenario、Checkpoint、乱数状態、モデル版、記録済みProvider応答を保持してください。元データ・比較・AI分析のJSONは内部保存を継続します。", "pr-small");
    return article;
  }

  let opener = null;
  function onKey(event) {
    if (event.key === "Escape") closeReport();
  }
  function closeReport() {
    const root = document.getElementById("print-report");
    if (!root || root.hidden) return;
    root.hidden = true;
    root.replaceChildren();
    document.documentElement.classList.remove("print-report-open");
    document.removeEventListener("keydown", onKey);
    opener?.focus();
  }
  /// Shows the report over the page, with buttons to print it (or save it as a PDF) and to close.
  function printReport(report) {
    let root = document.getElementById("print-report");
    if (!root) {
      root = el("div", null, "print-report");
      root.id = "print-report";
      root.setAttribute("role", "dialog");
      root.setAttribute("aria-modal", "true");
      root.setAttribute("aria-label", "分析レポート（印刷用）");
      document.body.append(root);
    }
    opener = document.activeElement;
    const bar = el("div", null, "print-report-bar");
    const print = el("button", "印刷・PDFに保存", "primary");
    print.type = "button";
    print.addEventListener("click", () => window.print());
    const close = el("button", "閉じる");
    close.type = "button";
    close.addEventListener("click", closeReport);
    bar.append(el("span", "印刷画面で「PDFに保存」を選ぶと、PDFとして保存できます。"), print, close);
    root.replaceChildren(bar, compose(report));
    root.hidden = false;
    document.documentElement.classList.add("print-report-open");
    document.addEventListener("keydown", onKey);
    print.focus();
  }
  Object.assign(app, { printReport });
})();
