// Runs the LLM relay (deploy/workers/relay.mjs, #419) in node for tests/unit/llm/test_relay.py:
// its Durable Object on node:sqlite (the SQL of `ctx.storage.sql`), and Anthropic replaced by
// the answers the script gives, so no request leaves the machine.
//
// stdin: { env: { ANTHROPIC_API_KEY?, LLM_PASSPHRASES?, LLM_ADMIN_KEY? }, tasks: { name: { model, promptVersion,
//          maxOutputTokens, maxPromptBytes, effort?, rejectInput? } } or real_tasks: true (the
//          relay's own, relay_tasks.mjs), steps: [{ now, request:
//          { method, path, headers, body }, provider?: { status, body } | "unreachable" }] }
// stdout: { answers: [{ status, body, provider_request? }], ledger: { spend, reservations, usage,
//           failures, recipients }, logs: [lines] }
// A header's "{issued}" is replaced by the passphrase the last admin issue answered (#433).
// A task's `build` puts the input into one user message (rejectInput: the input "bad" is
// refused); the provider request a step caused is answered with its headers but the API key's
// value replaced by whether it was the env's.

import { register } from "node:module";
import { DatabaseSync } from "node:sqlite";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const STUB = "export class DurableObject { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }";
register(
  `data:text/javascript,${encodeURIComponent(`export async function resolve(specifier, context, next) {
    if (specifier === "cloudflare:workers") return { url: "data:text/javascript,${encodeURIComponent(STUB)}", shortCircuit: true };
    return next(specifier, context);
  }`)}`,
);

const here = dirname(fileURLToPath(import.meta.url));
const relay = await import(pathToFileURL(join(here, "..", "..", "deploy", "workers", "relay.mjs")).href);

const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const script = JSON.parse(Buffer.concat(chunks).toString("utf8"));

const logs = [];
console.log = (line) => logs.push(String(line));
console.error = (line) => logs.push(String(line));

// ctx.storage of a SQLite-backed Durable Object, on node:sqlite.
const db = new DatabaseSync(":memory:");
const sql = {
  exec(query, ...bindings) {
    const statement = db.prepare(query);
    const rows = /^\s*SELECT/i.test(query) ? statement.all(...bindings) : (statement.run(...bindings), []);
    return {
      toArray: () => rows,
      one() {
        if (rows.length !== 1) throw new Error(`one() of ${rows.length} rows`);
        return rows[0];
      },
    };
  },
};
const storage = {
  sql,
  transactionSync(callback) {
    db.exec("BEGIN");
    try {
      const value = callback();
      db.exec("COMMIT");
      return value;
    } catch (error) {
      db.exec("ROLLBACK");
      throw error;
    }
  },
};
const ledger = new relay.LlmLedger({ storage }, {});
const env = {
  ...script.env,
  ASSETS: { fetch: async () => new Response("asset", { status: 200 }) },
  LEDGER: { idFromName: (name) => name, get: () => ledger },
};

const tasks = {};
for (const [name, spec] of Object.entries(script.tasks ?? {})) {
  tasks[name] = {
    ...spec,
    build(input) {
      if (spec.rejectInput && input === "bad") throw new relay.TaskInputError("入力を読めません。");
      return { system: "test system", messages: [{ role: "user", content: String(input) }] };
    },
  };
}

let clock = 0;
let step = null;
let providerRequest = null;
const worker = relay.createWorker({
  now: () => clock,
  tasks: script.real_tasks ? undefined : tasks,
  async fetchProvider(url, init) {
    const headers = { ...init.headers, "x-api-key": init.headers["x-api-key"] === script.env.ANTHROPIC_API_KEY ? "<the env's key>" : "<another>" };
    providerRequest = { url, headers, body: JSON.parse(init.body) };
    if (step.provider === "unreachable") throw new TypeError("fetch failed");
    return new Response(JSON.stringify(step.provider.body), { status: step.provider.status, headers: { "content-type": "application/json" } });
  },
});

const answers = [];
for (step of script.steps) {
  clock = step.now ?? clock;
  providerRequest = null;
  const { method = "POST", path, body } = step.request;
  // "{issued}" in a header is the passphrase the last /llm/v1/admin/issue answered (#433).
  const issued = answers.findLast((item) => typeof item.body?.passphrase === "string")?.body.passphrase;
  const headers = Object.fromEntries(Object.entries(step.request.headers ?? {}).map(([name, value]) => [name, String(value).replace("{issued}", issued ?? "")]));
  const request = new Request(`https://hinanjo-if.example${path}`, {
    method,
    headers,
    body: method === "GET" || method === "HEAD" ? undefined : typeof body === "string" ? body : JSON.stringify(body ?? {}),
  });
  const response = await worker.fetch(request, env);
  const text = await response.text();
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    parsed = text;
  }
  answers.push({ status: response.status, type: response.headers.get("content-type"), body: parsed, ...(providerRequest ? { provider_request: providerRequest } : {}) });
}

const table = (name) => db.prepare(`SELECT * FROM ${name}`).all();
process.stdout.write(
  JSON.stringify({
    answers,
    ledger: { spend: table("spend"), reservations: table("reservations"), usage: table("usage"), failures: table("failures"), recipients: table("recipients") },
    logs,
  }),
);
