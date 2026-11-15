// The LLM relay of the browser version (#287, #419): the Worker `hinanjo-if` (wrangler.jsonc)
// serves the static site as before and answers only `/llm/*` with this script
// (`assets.run_worker_first`). It holds the Anthropic API key (the secret ANTHROPIC_API_KEY),
// lets in only those with a passphrase it gave out (the secret LLM_PASSPHRASES), builds each
// task's prompt itself (relay_tasks.mjs) and keeps the whole site's spending under a daily cap in
// one Durable Object (LlmLedger).
//
// Routes (POST, same origin only, JSON body of at most BODY_LIMIT bytes, the passphrase as
// `Authorization: Bearer <passphrase>`):
//   /llm/v1/session        whether the passphrase is let in: { recipient, day, cap_usd,
//                          remaining_usd, available } (available: the relay has an API key)
//   /llm/v1/tasks/{task}   { input } → the task's prompt is built here from `input`, the Messages
//                          API is called, and { task, prompt_version, model, output_text,
//                          stop_reason, usage: { input_tokens, output_tokens, cost_usd },
//                          remaining_usd } is answered. The output is the model's text as it is:
//                          the page treats it as untrusted input and checks it (AGENTS.md).
//   /llm/v1/admin/recipients   the owner's list (#433): each recipient's label, where it comes
//                          from ("secret": LLM_PASSPHRASES, "issued": issued on /admin), when it
//                          was issued and revoked, and its calls, cost and tokens today and in all
//   /llm/v1/admin/issue    { label } → a new passphrase for the label, answered once and kept
//                          nowhere (only its SHA-256, in the ledger)
//   /llm/v1/admin/revoke   { label } → the issued passphrase of the label is no longer let in
//   The admin routes take the owner's key (the secret LLM_ADMIN_KEY) as `Authorization: Bearer
//   <key>` instead of a passphrase; too many wrong keys from one address are refused for a while,
//   counted apart from wrong passphrases. The page /admin (admin.html) calls them.
// Refusals are { error, code } with an error the screen shows as it is (Japanese).
//
// Passphrases: LLM_PASSPHRASES is a JSON array of { label, sha256 }, the SHA-256 of each
// passphrase (new_passphrase.mjs makes one). One per recipient; removing an entry (and putting the
// secret again) shuts that recipient out. The label is what the ledger and the logs record; the
// passphrase is never stored, logged or answered. Passphrases issued on /admin (#433) are let in
// the same way: the ledger keeps their labels and digests until they are revoked. A presented
// passphrase's digest is compared with every entry in full (no early exit). Too many wrong passphrases from one address
// (FAILURE_LIMIT in FAILURE_WINDOW_MS) are refused for the rest of the window.
//
// Spending (as Python's LLMRouter, src/sheltersim/llm/provider.py): before a call the ledger
// reserves the most it can cost (the UTF-8 bytes of the prompt plus 256, which overestimate its
// tokens, at the 1-hour cache-write price, and max_tokens at the output price); after it the
// reservation is settled at the cost the response's usage gives. A refused HTTP answer settles at
// nothing; a call whose answer never came settles at the reservation. While the reserved and
// spent amounts of the day (Japan time) would pass DAILY_CAP_USD, calls are refused. Amounts are
// whole micro-dollars: tokens × USD per million tokens.

import { DurableObject } from "cloudflare:workers";
import { TASKS } from "./relay_tasks.mjs";

/// The whole site's spending per day (00:00–24:00 Japan time), in USD (decided on #287).
export const DAILY_CAP_USD = 5;
/// USD per million tokens, the same as Python's `_OFFICIAL_MODEL_PRICE_FLOORS`
/// (src/sheltersim/llm/config.py; tests/unit/llm/test_relay.py keeps them equal).
export const PRICES = Object.freeze({
  "claude-opus-5-5": { input: 4.0, output: 20.0 },
  "claude-opus-5": { input: 5.0, output: 25.0 },
  "claude-sonnet-5-5": { input: 2.0, output: 10.0 },
  "claude-sonnet-5": { input: 1.0, output: 5.0 },
  "claude-haiku-5-5": { input: 0.1, output: 0.5 },
  "claude-haiku-4-5-20251001": { input: 0.5, output: 2.5 },
});
/// Room for thinking on top of a task's answer, on models whose thinking cannot be turned off
/// (Python's THINKING_HEADROOM_TOKENS).
export const THINKING_HEADROOM_TOKENS = 4096;
export const BODY_LIMIT = 64 * 1024;
export const FAILURE_LIMIT = 10;
export const FAILURE_WINDOW_MS = 15 * 60 * 1000;
/// Labels of recipients: at most this many characters (the secret's entries and issued ones).
export const LABEL_LIMIT = 40;
/// A passphrase: four groups of five letters and digits not easily mistaken for one another (about
/// 99 bits), as new_passphrase.mjs makes them.
export const PASSPHRASE_ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789";
const ADMIN = "/llm/v1/admin/";
const ADMIN_ROUTES = new Set(["recipients", "issue", "revoke"]);
const ANTHROPIC_URL = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION = "2023-06-01";
const CAP_MICRO = DAILY_CAP_USD * 1_000_000;
const encoder = new TextEncoder();

/// The day (Japan time, UTC+9 without summer time) of a moment, as YYYY-MM-DD.
export function jstDay(ms) {
  return new Date(ms + 9 * 3600 * 1000).toISOString().slice(0, 10);
}

const usd = (micro) => Math.max(0, micro) / 1_000_000;

async function sha256(text) {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", encoder.encode(text)));
}

function hex(bytes) {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/// A new passphrase (crypto.getRandomValues, each character uniform over PASSPHRASE_ALPHABET).
export function newPassphrase() {
  const size = PASSPHRASE_ALPHABET.length;
  const limit = 256 - (256 % size);
  const chars = [];
  while (chars.length < 20) {
    for (const byte of crypto.getRandomValues(new Uint8Array(32))) {
      if (byte < limit && chars.length < 20) chars.push(PASSPHRASE_ALPHABET[byte % size]);
    }
  }
  return [0, 5, 10, 15].map((at) => chars.slice(at, at + 5).join("")).join("-");
}

/// A label given on /admin, trimmed; null when it is empty, longer than LABEL_LIMIT or holds
/// control characters.
export function checkLabel(value) {
  if (typeof value !== "string") return null;
  const label = value.trim();
  if (!label || [...label].length > LABEL_LIMIT || /\p{Cc}/u.test(label)) return null;
  return label;
}

/// Digests (hex) as the recipients recipientFor compares: [{ label, digest }].
function fromHex(entries) {
  return entries.map(({ label, sha256: digest }) => ({ label, digest: Uint8Array.from(digest.match(/../g), (pair) => parseInt(pair, 16)) }));
}

/// The recipients of LLM_PASSPHRASES: [{ label, digest }]; entries not of the form are left out
/// (and said so in the log, without their content).
export function parsePassphrases(secret) {
  let list;
  try {
    list = JSON.parse(secret ?? "[]");
  } catch {
    console.error(JSON.stringify({ event: "llm_relay_passphrases_unreadable" }));
    return [];
  }
  if (!Array.isArray(list)) return [];
  const out = [];
  for (const entry of list) {
    const label = entry?.label;
    const digest = entry?.sha256;
    if (typeof label !== "string" || !label || label.length > LABEL_LIMIT || typeof digest !== "string" || !/^[0-9a-f]{64}$/.test(digest)) {
      console.error(JSON.stringify({ event: "llm_relay_passphrase_entry_skipped" }));
      continue;
    }
    out.push(...fromHex([{ label, sha256: digest }]));
  }
  return out;
}

/// Whether two digests are the same, looking at every byte.
function sameDigest(a, b) {
  let diff = a.length ^ b.length;
  for (let i = 0; i < a.length; i++) diff |= a[i] ^ (b[i] ?? 0);
  return diff === 0;
}

/// The recipient whose digest is the passphrase's, or null; every entry is compared in full.
export async function recipientFor(passphrase, recipients) {
  const digest = await sha256(passphrase);
  let found = null;
  for (const recipient of recipients) {
    if (sameDigest(digest, recipient.digest) && found === null) found = recipient;
  }
  return found;
}

/// The most a call can cost, in micro-dollars (see the head of this file).
export function reservationMicro(price, promptBytes, maxTokens) {
  return Math.ceil((promptBytes + 256) * price.input * 2 + maxTokens * price.output);
}

/// What a response's usage costs, in micro-dollars (Python's `_estimate_cost` without web search).
export function usageMicro(price, usage) {
  const created = usage?.cache_creation ?? {};
  const five = Number(created.ephemeral_5m_input_tokens ?? 0);
  const hour = Number(created.ephemeral_1h_input_tokens ?? 0);
  // Without the split, cache writes are counted at the dearer 1-hour price.
  const unsplit = Math.max(0, Number(usage?.cache_creation_input_tokens ?? 0) - five - hour);
  const micro =
    Number(usage?.input_tokens ?? 0) * price.input +
    five * price.input * 1.25 +
    (hour + unsplit) * price.input * 2 +
    Number(usage?.cache_read_input_tokens ?? 0) * price.input * 0.05 +
    Number(usage?.output_tokens ?? 0) * price.output;
  return Math.ceil(micro);
}

/// A task's input the task cannot build a prompt from (an answer 400 with the message).
export class TaskInputError extends Error {}

function answer(status, body) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
  });
}

const refuse = (status, code, error, extra = {}) => answer(status, { error, code, ...extra });

/// Checks the task registry's form; the registry otherwise an error.
export function checkTasks(tasks) {
  for (const [name, task] of Object.entries(tasks)) {
    if (!/^[a-z][a-z0-9_]{0,40}$/.test(name)) throw new Error(`task name ${name}`);
    if (!PRICES[task.model]) throw new Error(`${name}: no price for ${task.model}`);
    if (typeof task.promptVersion !== "string" || !task.promptVersion) throw new Error(`${name}: no prompt version`);
    for (const key of ["maxOutputTokens", "maxPromptBytes"]) {
      if (!Number.isSafeInteger(task[key]) || task[key] <= 0) throw new Error(`${name}: ${key}`);
    }
    if (typeof task.build !== "function") throw new Error(`${name}: no build`);
  }
  return tasks;
}

/// The Worker: `now()` the time in ms, `fetchProvider(url, init)` the call to Anthropic, `tasks` the
/// registry (relay_tasks.mjs). Tests make one with fakes.
export function createWorker({ now = () => Date.now(), fetchProvider = (url, init) => fetch(url, init), tasks = TASKS } = {}) {
  checkTasks(tasks);

  async function handle(request, env, url) {
    if (request.method !== "POST") return refuse(405, "llm_method_not_allowed", "この道はPOSTだけを受け付けます。");
    // Same origin only: the relay sends no CORS headers, and refuses what another site sends.
    const origin = request.headers.get("Origin");
    const site = request.headers.get("Sec-Fetch-Site");
    if ((origin && origin !== url.origin) || (site && site !== "same-origin" && site !== "none")) {
      return refuse(403, "llm_cross_origin", "ほかのサイトからは使えません。");
    }
    const declared = Number(request.headers.get("Content-Length") ?? 0);
    if (declared > BODY_LIMIT) return refuse(413, "llm_body_too_large", "送る内容が大きすぎます。");
    const text = await request.text();
    if (encoder.encode(text).length > BODY_LIMIT) return refuse(413, "llm_body_too_large", "送る内容が大きすぎます。");
    let body;
    try {
      body = text === "" ? {} : JSON.parse(text);
    } catch {
      body = null;
    }
    if (body === null || typeof body !== "object" || Array.isArray(body)) return refuse(400, "llm_body_invalid", "送る内容がJSONのobjectではありません。");

    const session = url.pathname === "/llm/v1/session";
    const taskName = /^\/llm\/v1\/tasks\/([a-z][a-z0-9_]{0,40})$/.exec(url.pathname)?.[1];
    const adminRoute = url.pathname.startsWith(ADMIN) ? url.pathname.slice(ADMIN.length) : null;
    if (!session && !taskName && !ADMIN_ROUTES.has(adminRoute)) return refuse(404, "llm_route_unknown", "中継にこの道はありません。");

    const ledger = env.LEDGER.get(env.LEDGER.idFromName("global"));
    const at = now();
    const day = jstDay(at);
    if (adminRoute) return admin(adminRoute, request, env, body, ledger, at, day);
    const auth = request.headers.get("Authorization") ?? "";
    const passphrase = auth.startsWith("Bearer ") ? auth.slice(7) : "";
    if (!passphrase) return refuse(401, "llm_passphrase_required", "合言葉を入れてください。");
    const address = hex(await sha256(`address:${request.headers.get("CF-Connecting-IP") ?? "unknown"}`)).slice(0, 32);
    if (await ledger.locked(address, at)) {
      return refuse(429, "llm_passphrase_locked", "合言葉の誤りが続いたため、しばらく受け付けません。15分ほど後に、もう一度試してください。");
    }
    const recipient = await recipientFor(passphrase, [...parsePassphrases(env.LLM_PASSPHRASES), ...fromHex(await ledger.activeRecipients())]);
    if (!recipient) {
      await ledger.failed(address, at);
      return refuse(401, "llm_passphrase_invalid", "合言葉が違います。");
    }

    if (session) {
      const used = await ledger.used(day);
      return answer(200, {
        recipient: recipient.label,
        day,
        cap_usd: DAILY_CAP_USD,
        remaining_usd: usd(CAP_MICRO - used),
        available: Boolean(env.ANTHROPIC_API_KEY),
      });
    }

    if (!env.ANTHROPIC_API_KEY) return refuse(503, "llm_relay_unconfigured", "中継にAPIキーが設定されていません。APIキーなしの動きで続けてください。");
    if (!Object.hasOwn(tasks, taskName)) return refuse(404, "llm_task_unknown", "中継にこの作業はありません。");
    const task = tasks[taskName];
    let prompt;
    try {
      prompt = task.build(body.input);
    } catch (error) {
      if (error instanceof TaskInputError) return refuse(400, "llm_task_input_invalid", error.message);
      throw error;
    }
    const promptBytes = encoder.encode(JSON.stringify({ system: prompt.system, messages: prompt.messages })).length;
    if (promptBytes > task.maxPromptBytes) return refuse(413, "llm_task_input_too_large", "入力が長すぎます。短くしてください。");
    const price = PRICES[task.model];
    const thinking = task.model.startsWith("claude-opus") ? THINKING_HEADROOM_TOKENS : 0;
    const maxTokens = task.maxOutputTokens + thinking;
    const reserved = reservationMicro(price, promptBytes, maxTokens);
    const held = await ledger.reserve({ day, recipient: recipient.label, task: taskName, micro: reserved, at });
    if (!held.ok) {
      return refuse(429, "llm_budget_exceeded", `今日のAPIの上限（$${DAILY_CAP_USD}）に達しました。日本時間の0時に戻ります。それまでは、APIキーなしの動きで続けてください。`, {
        remaining_usd: usd(held.remaining_micro),
      });
    }

    const request_body = { model: task.model, max_tokens: maxTokens, system: prompt.system, messages: prompt.messages };
    if (task.effort) request_body.output_config = { effort: task.effort };
    if (task.disableThinking) request_body.thinking = { type: "disabled" };
    let response;
    try {
      response = await fetchProvider(ANTHROPIC_URL, {
        method: "POST",
        headers: { "x-api-key": env.ANTHROPIC_API_KEY, "anthropic-version": ANTHROPIC_VERSION, "content-type": "application/json" },
        body: JSON.stringify(request_body),
        signal: AbortSignal.timeout(task.timeoutMs ?? 120_000),
      });
    } catch {
      // The request may have reached the API: it is counted at its reservation.
      await ledger.settle({ id: held.id, micro: reserved, outcome: "unreachable" });
      log({ recipient: recipient.label, task: taskName, outcome: "unreachable", micro: reserved });
      return refuse(502, "llm_provider_unreachable", "APIにつながりませんでした。APIキーなしの動きで続けてください。");
    }
    if (!response.ok) {
      const remaining = await ledger.settle({ id: held.id, micro: 0, outcome: `http_${response.status}` });
      log({ recipient: recipient.label, task: taskName, outcome: `http_${response.status}`, micro: 0 });
      const busy = response.status === 429 || response.status === 529;
      return refuse(busy ? 503 : 502, busy ? "llm_provider_busy" : "llm_provider_error", busy ? "APIが混み合っています。少し待ってから試すか、APIキーなしの動きで続けてください。" : "APIが誤りを返しました。APIキーなしの動きで続けてください。", { remaining_usd: usd(remaining) });
    }
    let data;
    try {
      data = await response.json();
    } catch {
      data = null;
    }
    const micro = data ? usageMicro(price, data.usage) : reserved;
    const remaining = await ledger.settle({
      id: held.id,
      micro,
      inputTokens: Number(data?.usage?.input_tokens ?? 0),
      outputTokens: Number(data?.usage?.output_tokens ?? 0),
      outcome: data ? "answered" : "unreadable",
    });
    log({ recipient: recipient.label, task: taskName, outcome: data ? "answered" : "unreadable", micro });
    if (!data) return refuse(502, "llm_provider_error", "APIの答えを読めませんでした。APIキーなしの動きで続けてください。");
    const output = (Array.isArray(data.content) ? data.content : [])
      .filter((block) => block?.type === "text" && typeof block.text === "string")
      .map((block) => block.text)
      .join("");
    return answer(200, {
      task: taskName,
      prompt_version: task.promptVersion,
      model: typeof data.model === "string" ? data.model : task.model,
      output_text: output,
      stop_reason: typeof data.stop_reason === "string" ? data.stop_reason : null,
      usage: {
        input_tokens: Number(data.usage?.input_tokens ?? 0),
        output_tokens: Number(data.usage?.output_tokens ?? 0),
        cost_usd: usd(micro),
      },
      remaining_usd: usd(remaining),
    });
  }

  // The owner's routes (#433): the key, then the list, an issue or a revoke.
  async function admin(route, request, env, body, ledger, at, day) {
    if (!env.LLM_ADMIN_KEY) {
      return refuse(503, "llm_admin_unconfigured", "中継に管理用の鍵が設定されていません。最初に一度だけ、コマンドで secret LLM_ADMIN_KEY を入れてください（docs/development.md の「LLMの中継」）。");
    }
    const auth = request.headers.get("Authorization") ?? "";
    const key = auth.startsWith("Bearer ") ? auth.slice(7) : "";
    if (!key) return refuse(401, "llm_admin_key_required", "管理用の鍵を入れてください。");
    const address = `admin:${hex(await sha256(`address:${request.headers.get("CF-Connecting-IP") ?? "unknown"}`)).slice(0, 32)}`;
    if (await ledger.locked(address, at)) {
      return refuse(429, "llm_admin_locked", "管理用の鍵の誤りが続いたため、しばらく受け付けません。15分ほど後に、もう一度試してください。");
    }
    if (!sameDigest(await sha256(key), await sha256(env.LLM_ADMIN_KEY))) {
      await ledger.failed(address, at);
      return refuse(401, "llm_admin_key_invalid", "管理用の鍵が違います。");
    }
    const secretLabels = new Set(parsePassphrases(env.LLM_PASSPHRASES).map((entry) => entry.label));
    if (route === "recipients") {
      const listed = await ledger.recipientList(day);
      const sums = (rows, label) => {
        const row = rows.find((item) => item.recipient === label);
        return {
          calls: Number(row?.calls ?? 0),
          spent_usd: usd(Number(row?.spent_micro ?? 0)),
          input_tokens: Number(row?.input_tokens ?? 0),
          output_tokens: Number(row?.output_tokens ?? 0),
        };
      };
      const time = (ms) => (ms === null || ms === undefined ? null : new Date(Number(ms)).toISOString());
      const recipients = [
        ...[...secretLabels].map((label) => ({ label, source: "secret", issued_at: null, revoked_at: null, active: true })),
        ...listed.issued.map((row) => ({ label: row.label, source: "issued", issued_at: time(row.issued_at), revoked_at: time(row.revoked_at), active: row.revoked_at === null })),
      ].map((item) => ({ ...item, today: sums(listed.today, item.label), total: sums(listed.total, item.label) }));
      return answer(200, { day, cap_usd: DAILY_CAP_USD, remaining_usd: usd(CAP_MICRO - (await ledger.used(day))), recipients });
    }
    const label = checkLabel(body.label);
    if (!label) return refuse(400, "llm_admin_label_invalid", `相手の名前は、1〜${LABEL_LIMIT}文字で入れてください。`);
    if (route === "issue") {
      if (secretLabels.has(label)) return refuse(409, "llm_admin_label_taken", "この名前は secret LLM_PASSPHRASES の一覧にあります。別の名前にしてください。");
      const passphrase = newPassphrase();
      const issued = await ledger.issue({ label, sha256: hex(await sha256(passphrase)), at });
      if (!issued) return refuse(409, "llm_admin_label_taken", "この名前の合言葉はすでに有効です。先に止めるか、別の名前にしてください。");
      console.log(JSON.stringify({ event: "llm_relay_admin", action: "issue", recipient: label }));
      return answer(200, { label, passphrase, issued_at: new Date(at).toISOString() });
    }
    // revoke
    if (await ledger.revoke({ label, at })) {
      console.log(JSON.stringify({ event: "llm_relay_admin", action: "revoke", recipient: label }));
      return answer(200, { label, revoked_at: new Date(at).toISOString() });
    }
    if (secretLabels.has(label)) {
      return refuse(409, "llm_admin_secret_entry", "この相手は secret LLM_PASSPHRASES の一覧にあるため、ここでは止められません。一覧から消して、secret を入れ直してください。");
    }
    return refuse(404, "llm_admin_label_unknown", "この名前の有効な合言葉はありません。");
  }

  return {
    async fetch(request, env) {
      const url = new URL(request.url);
      if (!url.pathname.startsWith("/llm/")) return env.ASSETS.fetch(request);
      try {
        return await handle(request, env, url);
      } catch (error) {
        console.error(JSON.stringify({ event: "llm_relay_error", name: error?.name ?? "Error" }));
        return refuse(500, "llm_relay_error", "中継で誤りが起きました。APIキーなしの動きで続けてください。");
      }
    },
  };
}

/// One line per call for `wrangler tail` and the Workers logs: who (the label), what, how it
/// ended and its cost; never the passphrase, the key or the text.
function log({ recipient, task, outcome, micro }) {
  console.log(JSON.stringify({ event: "llm_relay_call", recipient, task, outcome, cost_usd: usd(micro) }));
}

/// The ledger: one SQLite-backed Durable Object (`idFromName("global")`) for the whole site, so
/// every reservation is checked against the same day's sums. Each method runs in one
/// transaction; the Worker calls them by RPC.
///   spend(day, spent_micro)                         settled costs of a day
///   reservations(id, day, recipient, task, micro, at)   calls not settled yet (counted in full)
///   usage(day, recipient, calls, spent_micro, input_tokens, output_tokens)   per recipient
///   failures(address, window_start, count)          wrong passphrases (and, under "admin:", wrong
///                                                   owner's keys) per hashed address
///   recipients(id, label, sha256, issued_at, revoked_at)   passphrases issued on /admin (#433)
export class LlmLedger extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS spend (day TEXT PRIMARY KEY, spent_micro INTEGER NOT NULL)");
    this.sql.exec("CREATE TABLE IF NOT EXISTS reservations (id TEXT PRIMARY KEY, day TEXT NOT NULL, recipient TEXT NOT NULL, task TEXT NOT NULL, micro INTEGER NOT NULL, at INTEGER NOT NULL)");
    this.sql.exec("CREATE TABLE IF NOT EXISTS usage (day TEXT NOT NULL, recipient TEXT NOT NULL, calls INTEGER NOT NULL, spent_micro INTEGER NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, PRIMARY KEY (day, recipient))");
    this.sql.exec("CREATE TABLE IF NOT EXISTS failures (address TEXT PRIMARY KEY, window_start INTEGER NOT NULL, count INTEGER NOT NULL)");
    this.sql.exec("CREATE TABLE IF NOT EXISTS recipients (id INTEGER PRIMARY KEY AUTOINCREMENT, label TEXT NOT NULL, sha256 TEXT NOT NULL, issued_at INTEGER NOT NULL, revoked_at INTEGER)");
  }

  /// The issued passphrases not revoked: [{ label, sha256 }].
  activeRecipients() {
    return this.sql.exec("SELECT label, sha256 FROM recipients WHERE revoked_at IS NULL ORDER BY id").toArray();
  }

  /// Keeps a new passphrase's digest for `label`; false when the label has one not revoked.
  issue({ label, sha256, at }) {
    return this.ctx.storage.transactionSync(() => {
      const taken = this.sql.exec("SELECT id FROM recipients WHERE label = ? AND revoked_at IS NULL", label).toArray();
      if (taken.length > 0) return false;
      this.sql.exec("INSERT INTO recipients (label, sha256, issued_at, revoked_at) VALUES (?, ?, ?, NULL)", label, sha256, at);
      return true;
    });
  }

  /// Revokes the issued passphrase of `label`; false when it has none not revoked.
  revoke({ label, at }) {
    return this.ctx.storage.transactionSync(() => {
      const rows = this.sql.exec("SELECT id FROM recipients WHERE label = ? AND revoked_at IS NULL", label).toArray();
      if (rows.length === 0) return false;
      this.sql.exec("UPDATE recipients SET revoked_at = ? WHERE id = ?", at, rows[0].id);
      return true;
    });
  }

  /// The owner's list: the issued passphrases (without their digests) and each recipient's sums
  /// today and in all.
  recipientList(day) {
    return {
      issued: this.sql.exec("SELECT label, issued_at, revoked_at FROM recipients ORDER BY id").toArray(),
      today: this.sql.exec("SELECT recipient, calls, spent_micro, input_tokens, output_tokens FROM usage WHERE day = ?", day).toArray(),
      total: this.sql
        .exec("SELECT recipient, SUM(calls) AS calls, SUM(spent_micro) AS spent_micro, SUM(input_tokens) AS input_tokens, SUM(output_tokens) AS output_tokens FROM usage GROUP BY recipient")
        .toArray(),
    };
  }

  usedSync(day) {
    const spent = this.sql.exec("SELECT COALESCE(SUM(spent_micro), 0) AS micro FROM spend WHERE day = ?", day).one().micro;
    const held = this.sql.exec("SELECT COALESCE(SUM(micro), 0) AS micro FROM reservations WHERE day = ?", day).one().micro;
    return Number(spent) + Number(held);
  }

  /// The day's spent and reserved micro-dollars.
  used(day) {
    return this.usedSync(day);
  }

  /// Reserves `micro` for a call when the day's sums stay within the cap: { ok, id,
  /// remaining_micro } or { ok: false, remaining_micro }.
  reserve({ day, recipient, task, micro, at }) {
    return this.ctx.storage.transactionSync(() => {
      const used = this.usedSync(day);
      if (used + micro > CAP_MICRO) return { ok: false, remaining_micro: CAP_MICRO - used };
      const id = crypto.randomUUID();
      this.sql.exec("INSERT INTO reservations (id, day, recipient, task, micro, at) VALUES (?, ?, ?, ?, ?, ?)", id, day, recipient, task, micro, at);
      return { ok: true, id, remaining_micro: CAP_MICRO - used - micro };
    });
  }

  /// Settles a reservation at `micro`, adds it to the day's and the recipient's sums; the day's
  /// remaining micro-dollars.
  settle({ id, micro, inputTokens = 0, outputTokens = 0 }) {
    return this.ctx.storage.transactionSync(() => {
      const rows = this.sql.exec("SELECT day, recipient FROM reservations WHERE id = ?", id).toArray();
      if (rows.length === 0) return 0;
      const { day, recipient } = rows[0];
      this.sql.exec("DELETE FROM reservations WHERE id = ?", id);
      this.sql.exec("INSERT INTO spend (day, spent_micro) VALUES (?, ?) ON CONFLICT (day) DO UPDATE SET spent_micro = spent_micro + excluded.spent_micro", day, micro);
      this.sql.exec(
        "INSERT INTO usage (day, recipient, calls, spent_micro, input_tokens, output_tokens) VALUES (?, ?, 1, ?, ?, ?) ON CONFLICT (day, recipient) DO UPDATE SET calls = calls + 1, spent_micro = spent_micro + excluded.spent_micro, input_tokens = input_tokens + excluded.input_tokens, output_tokens = output_tokens + excluded.output_tokens",
        day,
        recipient,
        micro,
        inputTokens,
        outputTokens,
      );
      return CAP_MICRO - this.usedSync(day);
    });
  }

  /// Whether the address gave FAILURE_LIMIT wrong passphrases in the current window.
  locked(address, at) {
    const rows = this.sql.exec("SELECT window_start, count FROM failures WHERE address = ?", address).toArray();
    return rows.length > 0 && at - Number(rows[0].window_start) < FAILURE_WINDOW_MS && Number(rows[0].count) >= FAILURE_LIMIT;
  }

  /// Counts a wrong passphrase from the address (a new window after FAILURE_WINDOW_MS).
  failed(address, at) {
    this.ctx.storage.transactionSync(() => {
      this.sql.exec("DELETE FROM failures WHERE window_start < ?", at - FAILURE_WINDOW_MS);
      this.sql.exec("INSERT INTO failures (address, window_start, count) VALUES (?, ?, 1) ON CONFLICT (address) DO UPDATE SET count = count + 1", address, at);
    });
  }

  /// The per-recipient sums of a day, for the owner's checks (not reachable from the routes).
  usage(day) {
    return this.sql.exec("SELECT recipient, calls, spent_micro, input_tokens, output_tokens FROM usage WHERE day = ? ORDER BY recipient", day).toArray();
  }
}

export default createWorker();
