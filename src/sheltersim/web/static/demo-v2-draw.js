/* The canvases: the map of a frame, the facility of a preview and the indicators over time. */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const { state, $, metricDefs, time, num } = app;
  const { motionFraction, comparing } = app.later;
  // Canvas rendering uses geometry, physical points, and recorded staff positions only.
  function surface(canvas) {
    const r = canvas.getBoundingClientRect();
    if (!r.width || !r.height) return null;
    const dpr = Math.min(devicePixelRatio || 1, 2);
    const width = Math.round(r.width * dpr),
      height = Math.round(r.height * dpr);
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    const ctx = canvas.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return { ctx, w: r.width, h: r.height };
  }
  const hits = new Map();
  function drawMap(canvas, frame, layer = "normal", zoom = state.zoom, extra = {}) {
    const sf = surface(canvas);
    if (!sf) return;
    const interactive = [
      "simulation-map",
      "compare-map-a",
      "compare-map-b",
    ].includes(canvas.id);
    hits.set(
      canvas,
      window.HinanjoMap.draw(sf, frame, {
        viewportBounds:
          state.page === "comparison" &&
          state.sceneMode === "physical" &&
          state.comparison
            ? {
                width: Math.max(
                  ...state.comparison.frames.map(
                    (f) => f.geometry?.width_m || 1,
                  ),
                ),
                height: Math.max(
                  ...state.comparison.frames.map(
                    (f) => f.geometry?.height_m || 1,
                  ),
                ),
              }
            : undefined,
        mode: state.sceneMode,
        animate:
          state.playing &&
          !matchMedia("(prefers-reduced-motion: reduce)").matches,
        time: state.visualMs,
        reducedMotion: matchMedia("(prefers-reduced-motion: reduce)").matches,
        gaitDistances: state.replay?.gaits.get(frame?.branch_id),
        layer,
        zoom,
        selected: state.selected,
        pan: interactive ? state.mapPan : { x: 0, y: 0 },
        ...extra,
      }),
    );
  }
  function drawPopulation(scenario) {
    const sf = surface($("population-chart"));
    if (!sf) return;
    const { ctx, w, h } = sf,
      people = Object.values(scenario.model_configs?.population?.people || {}),
      groups = [
        ["子ども", 0, "#ffb449"],
        ["成人", 0, "#3c98ff"],
        ["高齢者", 0, "#f26b8c"],
        ["未記録", 0, "#bdcbd7"],
      ];
    for (const p of people) {
      const a = p.age_years;
      groups[typeof a !== "number" ? 3 : a < 18 ? 0 : a >= 65 ? 2 : 1][1]++;
    }
    const total = people.length;
    ctx.font = "10px sans-serif";
    if (!total) {
      ctx.fillText("年齢構成の記録なし", 8, h / 2);
      return;
    }
    let angle = -Math.PI / 2;
    const cx = w / 2,
      cy = h * 0.36,
      r = Math.min(w * 0.28, h * 0.23);
    for (const [label, count, color] of groups) {
      ctx.beginPath();
      ctx.arc(cx, cy, r, angle, angle + (count / total) * Math.PI * 2);
      ctx.arc(
        cx,
        cy,
        r * 0.6,
        angle + (count / total) * Math.PI * 2,
        angle,
        true,
      );
      ctx.closePath();
      ctx.fillStyle = color;
      ctx.fill();
      angle += (count / total) * Math.PI * 2;
      const index = groups.findIndex((g) => g[0] === label);
      ctx.fillRect(10, h * 0.68 + index * 13, 7, 7);
      ctx.fillStyle = "#36577c";
      ctx.fillText(`${label} ${count}人`, 22, h * 0.68 + index * 13 + 7);
    }
  }
  function drawHistory() {
    const sf = surface($("history-chart"));
    if (!sf) return;
    const { ctx, w, h } = sf;
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, w, h);
    const key = $("chart-metric").value,
      overview = key === "overview";
    const definitions = overview
      ? [
          {
            key: "stress_mean",
            label: "平均ストレス /100",
            factor: 100,
            color: "#ff365e",
            axis: "left",
          },
          {
            key: "staff_load_ratio",
            label: "スタッフ負荷 %",
            factor: 100,
            color: "#efa023",
            axis: "left",
          },
        ]
      : [
          {
            ...metricDefs.find((d) => d.key === key),
            color:
              key === "stress_mean"
                ? "#ff365e"
                : key === "staff_load_ratio"
                  ? "#efa023"
                  : "#0878ff",
            axis: "left",
          },
        ];
    const raw = state.history
      .map((row) => row.data || row.snapshot || row)
      .filter((r) => typeof r.minute_index === "number");
    const series = definitions
      .map((d) => ({
        ...d,
        points: raw.filter((r) => typeof r[d.key] === "number"),
      }))
      .filter((d) => d.points.length);
    ctx.font = "13px sans-serif";
    ctx.fillStyle = "#6783a4";
    if (!series.length) {
      ctx.fillText("この指標の時系列記録はありません", 20, h / 2);
      return;
    }
    const left = 43,
      right = 18,
      top = 40,
      bottom = 32,
      pw = w - left - right,
      ph = h - top - bottom;
    const minX = Math.min(...raw.map((p) => p.minute_index)),
      maxX = Math.max(minX + 1, ...raw.map((p) => p.minute_index));
    const leftValues = series
      .filter((d) => d.axis === "left")
      .flatMap((d) => d.points.map((p) => p[d.key] * d.factor));
    const high = Math.max(1, ...leftValues),
      maxY = overview
        ? 100
        : high >= 10
          ? Math.ceil((high * 1.05) / 10) * 10
          : high * 1.1;
    let legendX = left;
    for (const d of series) {
      ctx.strokeStyle = d.color;
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.moveTo(legendX, 13);
      ctx.lineTo(legendX + 16, 13);
      ctx.stroke();
      ctx.fillStyle = "#47658f";
      ctx.textAlign = "left";
      ctx.font = "13px sans-serif";
      ctx.fillText(d.label, legendX + 21, 17);
      legendX += ctx.measureText(d.label).width + 38;
    }
    ctx.lineWidth = 1;
    for (let i = 0; i <= 4; i++) {
      const y = top + (ph * i) / 4;
      ctx.strokeStyle = "#e5eef7";
      ctx.beginPath();
      ctx.moveTo(left, y);
      ctx.lineTo(w - right, y);
      ctx.stroke();
      ctx.textAlign = "right";
      ctx.fillStyle = "#7186a8";
      ctx.font = "13px sans-serif";
      ctx.fillText(num(maxY * (1 - i / 4)), left - 7, y + 4);
    }
    const xOf = (m) => left + ((m - minX) / (maxX - minX)) * pw;
    for (const d of series) {
      const pos = (p) => [
        xOf(p.minute_index),
        top +
          ph -
          ((p[d.key] * d.factor) / (d.axis === "right" ? 100 : maxY)) * ph,
      ];
      ctx.beginPath();
      d.points.forEach((p, i) => {
        const [x, y] = pos(p);
        i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
      });
      ctx.strokeStyle = d.color;
      ctx.lineWidth = 2.3;
      ctx.stroke();
      if (d.key === "population") {
        const last = pos(d.points.at(-1)),
          first = pos(d.points[0]);
        ctx.lineTo(last[0], top + ph);
        ctx.lineTo(first[0], top + ph);
        ctx.closePath();
        ctx.fillStyle = "#0878ff0b";
        ctx.fill();
      }
      for (
        let i = 0;
        i < d.points.length;
        i += Math.max(1, Math.ceil(d.points.length / 14))
      ) {
        const [x, y] = pos(d.points[i]);
        ctx.fillStyle = d.color;
        ctx.beginPath();
        ctx.arc(x, y, 2.1, 0, Math.PI * 2);
        ctx.fill();
      }
    }
    ctx.textAlign = "center";
    ctx.font = "13px sans-serif";
    ctx.fillStyle = "#647fa3";
    for (let i = 0; i <= 3; i++) {
      const m = Math.round(minX + ((maxX - minX) * i) / 3);
      ctx.textAlign = i === 0 ? "left" : i === 3 ? "right" : "center";
      ctx.fillText(time(m), left + (pw * i) / 3, h - 10);
    }
  }
  function drawAll() {
    const visible = (frame, index = 0) =>
      state.motion
        ? window.HinanjoScene.sample(
            state.motion.from[index],
            state.motion.to[index],
            motionFraction(),
            state.sceneMode,
          )
        : frame;
    if (state.page === "scenario" && state.preview) {
      const f = state.preview.facility_design;
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
      drawPopulation(state.preview.scenario);
    }
    if (state.page === "simulation")
      drawMap($("simulation-map"), visible(state.frame), $("layer").value);
    if (state.page === "comparison" && comparing()) {
      drawMap(
        $("compare-map-a"),
        visible(state.comparison.frames[0], 0),
        $("compare-layer").value,
      );
      drawMap(
        $("compare-map-b"),
        visible(state.comparison.frames[1], 1),
        $("compare-layer").value,
      );
    } else if (state.page === "comparison") {
      // The open branch alone, and B's place for the branch to be made.
      drawMap($("compare-map-a"), visible(state.frame), $("compare-layer").value);
      drawMap($("compare-map-b"), null, $("compare-layer").value, state.zoom, {
        emptyText: state.run
          ? "この時刻から分岐を作ると、ここに並びます"
          : "シナリオを開くと施設が表示されます",
      });
    }
    if (state.page === "report") drawHistory();
  }
  Object.assign(app, { hits, drawMap, drawPopulation, drawHistory, drawAll });
})();
