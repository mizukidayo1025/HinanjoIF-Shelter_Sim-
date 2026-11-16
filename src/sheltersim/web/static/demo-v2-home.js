/* The home: the saved Scenarios, the examples to fetch and the AI's API (its ON/OFF, and the
   browser version's passphrase). */
(() => {
  "use strict";
  const app = window.HinanjoApp;
  const {
    site, exampleProject, state, remember, $, enc, disasterName, time, num, el, api, action, show,
  } = app;
  const { openRun, clearRunView, refreshRuns, invalidateIntervention, resetChat } = app.later;
  // A Run's name on the home and in the header: where, what disaster, how many people, how long.
  function runTitle(run, scenarios) {
    const scenario = scenarios.find(
        (s) =>
          s.scenario_id === run.scenario_id &&
          s.scenario_version === run.scenario_version,
      )?.scenario,
      request =
        scenario?.provenance?.natural_language_input ||
        scenario?.provenance?.initial_scenario_request,
      people = scenario?.profiles?.length,
      days = scenario?.duration_minutes / 1440;
    return {
      name: request
        ? `${request.municipality} · ${disasterName(request.disaster)}`
        : run.scenario_id,
      size: [
        typeof people === "number" ? `${people}人` : null,
        Number.isFinite(days) ? `${num(days)}日間` : null,
      ]
        .filter(Boolean)
        .join(" · "),
      total: scenario?.duration_minutes,
    };
  }
  // The screen calls every Run a シナリオ (#373), named by its conditions. One's own Runs of the
  // same conditions are told apart by when they were made (#447): 「11/15 10:30 作成」 in local
  // time, from the record's `created_at` (a Run made before #447 has none). An example's Runs are
  // named by their conditions alone: its rows carry the 「例」 mark instead.
  function runNames(runs, scenarios, { example = false } = {}) {
    const titled = runs.map((r) => {
      const { name, size } = runTitle(r, scenarios);
      return { run: r, name: [name, size].filter(Boolean).join(" · ") };
    });
    const count = new Map();
    for (const t of titled) count.set(t.name, (count.get(t.name) || 0) + 1);
    return new Map(
      titled.map((t) => {
        const made = example || count.get(t.name) < 2 ? null : createdLabel(t.run.created_at);
        return [t.run.run_id, made ? `${t.name} · ${made}` : t.name];
      }),
    );
  }
  // When a Run was made, in local time: 「11/15 10:30 作成」; null without a creation time.
  function createdLabel(createdAt) {
    const date = createdAt ? new Date(createdAt) : null;
    if (!date || Number.isNaN(date.getTime())) return null;
    const two = (n) => String(n).padStart(2, "0");
    return `${date.getMonth() + 1}/${date.getDate()} ${two(date.getHours())}:${two(date.getMinutes())} 作成`;
  }
  // How far a Run is computed, in words.
  function runProgress(run, total) {
    const main = run.branches?.find((b) => b.branch_id === "main"),
      until = main?.computed_until ?? 0;
    if (run.status === "completed" || (total && until >= total))
      return "最後まで計算済み";
    if (run.status === "running") return `計算中 · ${time(until)}まで`;
    if (!until) return "まだ計算していません";
    return `${time(until)}まで計算済み`;
  }
  function runLink(projectId, run, scenarios, label, named) {
    const { name: title, size, total } = runTitle(run, scenarios),
      name = named || title,
      params = new URLSearchParams({
        project: projectId,
        run: run.run_id,
        branch: "main",
      }),
      link = el("a", undefined, "home-run");
    link.href = `/?${params}#simulation`;
    link.append(
      el("strong", label || name),
      el("span", [label ? name : null, named ? null : size].filter(Boolean).join(" · ")),
      el("small", runProgress(run, total)),
    );
    link.addEventListener("click", (event) => {
      if (event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey)
        return;
      event.preventDefault();
      void openRun(projectId, run.run_id);
    });
    return link;
  }
  // The home: the examples' Runs and one's own Scenarios. It reads the Projects and changes
  // nothing. The browser version holds an example once it is fetched (#411): the home lists the
  // examples it holds and offers the others to fetch.
  async function loadHome() {
    const scope = ++state.homeVersion,
      browser = window.HinanjoBrowserApi,
      offered = browser ? await browser.examples() : [],
      exampleIds = browser
        ? offered.filter((e) => e.installed).map((e) => e.project_id)
        : [exampleProject],
      listed = new Set(
        (await api("/api/v1/projects")).projects.map((p) => p.project_id),
      ),
      read = async (id) => (listed.has(id) ? api(`/api/v1/projects/${enc(id)}`) : null),
      [examples, own] = await Promise.all([
        Promise.all(exampleIds.map(async (id) => [id, await read(id)])),
        !app.ownId || exampleIds.includes(app.ownId) ? null : read(app.ownId),
      ]);
    if (scope !== state.homeVersion) return;
    // One list of the saved Scenarios (#440): the examples', marked 「例」, then one's own.
    const rows = [
      ...examples.flatMap(([id, project]) =>
        homeRows(id, project, { example: true, deletable: Boolean(browser) }),
      ),
      ...(own ? homeRows(app.ownId, own, { example: false, deletable: true }) : []),
    ];
    $("home-list").replaceChildren(
      ...(rows.length ? rows : [el("p", "まだシナリオはありません。", "quiet-message")]),
    );
    $("home-fetch").hidden = !offered.some((e) => !e.installed);
    void refreshApi();
    $("home-browser-notes").hidden = !browser;
    $("home-new").disabled = site.scenarios === "unavailable";
    $("home-new-note").hidden = site.scenarios !== "unavailable";
  }
  // A Project's Runs as rows of the home's list: each opens its Run, and has 「削除」 when it may
  // be deleted. The server's example cannot be fetched again, so it has none there (#440).
  function homeRows(projectId, project, { example, deletable }) {
    const runs = project?.runs || [],
      names = runNames(runs, project?.scenarios || [], { example });
    return runs.map((run) => {
      const name = names.get(run.run_id),
        row = el("div", undefined, "home-item"),
        link = runLink(projectId, run, project.scenarios, undefined, name);
      if (example) link.querySelector("strong").append(" ", el("span", "例", "home-tag"));
      row.append(link);
      if (deletable) {
        const remove = el("button", "削除", "danger compact home-delete");
        remove.type = "button";
        remove.setAttribute("aria-label", `「${name}」を削除`);
        remove.addEventListener("click", () =>
          action("シナリオを削除しています…", () =>
            deleteRun(projectId, run.run_id, name),
          ),
        );
        row.append(remove);
      }
      return row;
    });
  }
  // 「計算済みの例を取得する」 (#411): the site's examples, what each holds and its size; the one
  // chosen is fetched and saved in this browser, then opened.
  const mb = (bytes) => `約${num(Math.max(1, Math.round(bytes / 1e6)))} MB`;
  function exampleTitle(example) {
    const days = example.duration_minutes / 1440;
    return {
      name: [
        example.municipality,
        example.disaster ? disasterName(example.disaster) : null,
      ]
        .filter(Boolean)
        .join(" · ") || example.project_id,
      size: [
        Number.isFinite(example.people) ? `${example.people}人` : null,
        Number.isFinite(days) ? `${num(days)}日間` : null,
        example.branches > 1 ? `分岐 ${example.branches}本` : null,
      ]
        .filter(Boolean)
        .join(" · "),
    };
  }
  // 「API を有効にする」 (#420): the passphrase handed out for the browser site's LLM relay
  // (deploy/workers/relay.mjs, #419), kept in this browser only. The relay checks it and says
  // whose it is and what is left of the site's daily cap; the screen never sees an API key.
  const LLM_KEY = "hinanjo-if-llm-passphrase";
  const storedPassphrase = () => {
    try {
      return localStorage.getItem(LLM_KEY) || "";
    } catch {
      return "";
    }
  };
  function keepPassphrase(value) {
    try {
      if (value) localStorage.setItem(LLM_KEY, value);
      else localStorage.removeItem(LLM_KEY);
    } catch {
      /* Not kept: it is asked again on the next visit. */
    }
  }
  async function llmSession(passphrase) {
    let response;
    try {
      response = await fetch("/llm/v1/session", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${passphrase}`,
          "Content-Type": "application/json",
        },
        body: "{}",
      });
    } catch {
      throw Object.assign(new Error("中継に接続できませんでした。"), { code: "llm_unreachable" });
    }
    let body = null;
    try {
      body = await response.json();
    } catch {
      /* A site without the relay answers its 404 page. */
    }
    if (!response.ok)
      throw Object.assign(
        new Error(body?.error || "このサイトではAPIを有効にできません。"),
        { code: body?.code || `http_${response.status}` },
      );
    return body;
  }
  // Whether the AI's (LLM's) features are ON (#581): one state for the whole screen
  // (`state.llm`), shown at the top and on the home. The browser version's comes from its relay
  // (the passphrase), the server's from its LLM settings (GET /api/v1/llm).
  // The features that need the AI (#580): each is a `<fieldset data-needs-api>` around its
  // controls. While the API is OFF (or not yet known) the fieldset is disabled, which disables
  // every control in it without touching the controls' own `disabled`, and a note at its top
  // says how to turn it ON.
  const NEEDS_API_NOTE =
    "AIを使う機能です。API を ON にすると使えます（ホームの「AIの機能（API）」）。";
  function applyLlm() {
    const on = state.llm?.on === true;
    document.querySelectorAll("[data-needs-api]").forEach((box) => {
      box.disabled = !on;
      let note = box.querySelector(":scope > .needs-api-note");
      if (!note) {
        note = el("p", NEEDS_API_NOTE, "needs-api-note");
        box.prepend(note);
      }
      note.hidden = on;
    });
  }
  function setLlm(on, label) {
    state.llm = { on };
    applyLlm();
    const badge = $("header-api");
    badge.hidden = false;
    badge.textContent = on ? "API ON" : "API OFF";
    badge.classList.toggle("on", on);
    badge.title = `AIの機能（API）：${label}`;
    $("home-api-status").textContent = label;
    $("home-api-status").classList.toggle("on", on);
  }
  function showApi(session, message) {
    setLlm(
      Boolean(session?.available),
      session
        ? session.available
          ? `有効です（${session.recipient}）。今日のサイト全体の残り：$${Number(session.remaining_usd).toFixed(2)}`
          : `合言葉は確認できました（${session.recipient}）が、中継にAPIキーがまだ設定されていません。`
        : message || "まだ有効にしていません。",
    );
    $("home-api-enable").hidden = Boolean(session);
    $("home-api-disable").hidden = !session;
  }
  function showServerApi(status) {
    setLlm(
      status.available,
      status.available
        ? status.mode === "replay"
          ? "ON です（記録したAIの応答で答えます）。"
          : "ON です（サーバーにAPIキーが設定されています）。"
        : `OFF です。${status.reason}`,
    );
  }
  // Asked at the start and on each visit to the home; a check under way is shared.
  let checking = null;
  function refreshApi() {
    return (checking ??= checkApi().finally(() => (checking = null)));
  }
  async function checkApi() {
    const browser = Boolean(window.HinanjoBrowserApi);
    $("home-api-how").textContent = browser
      ? "配布した合言葉を入れると ON になります。合言葉はこのブラウザーにだけ保存します。"
      : "サーバーをAPIキーつきで起動すると ON になります。";
    if (!browser) {
      $("home-api-enable").hidden = true;
      $("home-api-disable").hidden = true;
      try {
        showServerApi(await api("/api/v1/llm"));
      } catch (error) {
        setLlm(false, `確認できませんでした：${error.message}`);
      }
      return;
    }
    const passphrase = storedPassphrase();
    if (!passphrase) return showApi(null);
    try {
      showApi(await llmSession(passphrase));
    } catch (error) {
      // A passphrase no longer let in is forgotten; a relay out of reach keeps it.
      if (["llm_passphrase_invalid", "llm_passphrase_required"].includes(error.code))
        keepPassphrase("");
      showApi(null, error.message);
    }
  }
  async function enableApi(event) {
    event.preventDefault();
    const passphrase = $("api-passphrase").value.trim();
    if (!passphrase) return;
    $("api-submit").disabled = true;
    $("api-dialog-note").textContent = "確認しています…";
    try {
      const session = await llmSession(passphrase);
      keepPassphrase(passphrase);
      $("api-passphrase").value = "";
      $("api-dialog-note").textContent = "";
      $("api-dialog").close();
      showApi(session);
    } catch (error) {
      $("api-dialog-note").textContent = error.message;
    } finally {
      $("api-submit").disabled = false;
    }
  }
  let fetching = false;
  async function openExamples() {
    const dialog = $("examples-dialog");
    $("examples-progress").hidden = true;
    $("examples-list").replaceChildren(el("p", "例の一覧を読み込んでいます…", "quiet-message"));
    if (!dialog.open) dialog.showModal();
    let examples;
    try {
      examples = await window.HinanjoBrowserApi.examples();
    } catch (error) {
      $("examples-list").replaceChildren(
        el("p", `例の一覧を読めませんでした：${error.message}`, "quiet-message"),
      );
      return;
    }
    $("examples-list").replaceChildren(
      ...(examples.length
        ? examples.map((example) => {
            const { name, size } = exampleTitle(example),
              row = el("div", undefined, "home-run examples-item"),
              button = el(
                "button",
                example.installed ? "開く" : "取得する",
                example.installed ? undefined : "primary",
              );
            button.type = "button";
            button.addEventListener("click", () => void chooseExample(example));
            row.append(
              el("strong", name),
              el("span", size),
              el("small", example.installed ? "取得済み" : `${mb(example.bytes)}を読み込みます`),
              button,
            );
            return row;
          })
        : [el("p", "このサイトには計算済みの例がありません。", "quiet-message")]),
    );
  }
  async function chooseExample(example) {
    if (fetching) return;
    fetching = true;
    const buttons = [...$("examples-list").querySelectorAll("button")],
      bar = $("examples-progress-bar"),
      text = $("examples-progress-text");
    buttons.forEach((b) => (b.disabled = true));
    try {
      if (!example.installed) {
        $("examples-progress").hidden = false;
        bar.removeAttribute("value");
        text.textContent = "読み込みを始めています…";
        await window.HinanjoBrowserApi.install(example.project_id, ({ phase, bytes, total }) => {
          if (phase === "fetch") {
            bar.max = Math.max(1, total);
            bar.value = bytes;
            text.textContent = `読み込んでいます… ${mb(bytes)} / ${mb(total)}`;
          } else if (phase === "import") {
            bar.removeAttribute("value");
            text.textContent = "このブラウザーに保存しています…";
          }
        });
      }
      const project = await api(`/api/v1/projects/${enc(example.project_id)}`),
        run = (project.runs || []).find((r) => r.status === "completed") || project.runs?.[0];
      $("examples-dialog").close();
      if (run) await openRun(example.project_id, run.run_id);
      else show("home");
    } catch (error) {
      text.textContent = error.message;
      $("examples-progress").hidden = false;
      bar.value = 0;
    } finally {
      fetching = false;
      buttons.forEach((b) => (b.disabled = false));
    }
  }
  // 「削除」 on the home (#440): after the person confirms with the Scenario's name
  // (POST runs/{run}/delete). Deleting the open Run closes it.
  async function deleteRun(projectId, runId, name) {
    if (
      !confirm(
        `「${name}」を削除します。計算の記録、分岐、質問の記録も消え、元に戻せません。よろしいですか？`,
      )
    )
      return;
    await api(`/api/v1/projects/${enc(projectId)}/runs/${enc(runId)}/delete`, {
      confirmation: runId,
    });
    if (state.project === projectId && state.run === runId) {
      clearTimeout(state.poll);
      state.scope++;
      state.run = null;
      state.data = null;
      state.frame = null;
      state.loadedRun = null;
      state.comparison = null;
      invalidateIntervention();
      resetChat();
      clearRunView();
      await refreshRuns();
      remember();
    }
    await loadHome();
  }
  Object.assign(app, {
    runNames, loadHome, keepPassphrase, showApi, refreshApi, enableApi, openExamples,
    applyLlm,
  });
  Object.defineProperty(app, "fetching", { get: () => fetching });
})();
