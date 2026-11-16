// The core's tasks asked through the site's LLM relay (#470): a result question, the reading of an
// intervention's text (and its one repair) and the PDF report's analysis. The core builds the task
// (`llm::Task`'s JSON) from the Run the browser holds; this module sends it as it is to the relay
// (deploy/workers/relay.mjs, `/llm/v1/tasks/{task}`, `{"input": task}`) with the passphrase the
// home keeps, and gives back the relay's answer as the core's reply (`llm::Reply`'s JSON), which
// the core reads against the task's schema and the evidence (the answer is untrusted input).
//
// What the reply records is what Python's router records for the same call
// (src/sheltersim/llm/provider.py): an answer is `recorded` (the model, the tokens, the cost the
// relay settled); a refused call is the task's fallback with the router's outcome for it:
// `budget_exceeded` (the site's day cap), `rate_limited` (the API busy), `input_token_limit` (an
// input the relay finds too large), `api_error` (anything else; counted as a request where the
// call reached the API). A relay that does not let the passphrase in (wrong, revoked, too many
// wrong ones) or has no API key is no link at all: `askRelay` answers null, and the route answers
// as without an API key, as the Python server does without one. The passphrase is never stored.

/// The relay's route of a task.
export const RELAY_TASKS = "/llm/v1/tasks/";

/// The relay's refusals that mean there is no link: the answer without an API key.
const NO_LINK = new Set(["llm_passphrase_required", "llm_passphrase_invalid", "llm_passphrase_locked", "llm_relay_unconfigured"]);
/// The router's outcome of each refusal of the relay (others: `api_error`).
const OUTCOMES = { llm_budget_exceeded: "budget_exceeded", llm_provider_busy: "rate_limited", llm_task_input_too_large: "input_token_limit" };
/// The refusals of a call that reached the API (the relay counted it).
const CALLED = new Set(["llm_provider_busy", "llm_provider_error", "llm_provider_unreachable"]);

/// The site's relay, on the page's origin.
export const sameOriginFetch = (path, init) => fetch(new URL(path, globalThis.location?.href ?? "http://localhost/"), init);

const count = (value) => (Number.isSafeInteger(Number(value)) && Number(value) >= 0 ? Number(value) : 0);

function fallback(task, outcome, requests) {
  return JSON.stringify({
    text: task.fallback_text,
    outcome,
    fallback_used: true,
    cache_hit: false,
    model_id: null,
    estimated_cost_usd: 0,
    api_request_count: requests,
    input_tokens: 0,
    output_tokens: 0,
  });
}

/// The relay's reply to the core's task `taskJson` (its JSON text) as `llm::Reply`'s JSON text, or
/// null where the relay is no link (see above). `relay(path, init)` makes the request (the page's
/// `fetch`; tests answer it).
export async function askRelay(relay, passphrase, taskJson) {
  const task = JSON.parse(taskJson);
  let response;
  let body = null;
  try {
    response = await relay(`${RELAY_TASKS}${task.task}`, {
      method: "POST",
      headers: { Authorization: `Bearer ${passphrase}`, "Content-Type": "application/json" },
      // The task's JSON as the core wrote it: the relay reads its schema's numbers as written.
      body: `{"input":${taskJson}}`,
    });
    body = await response.json().catch(() => null);
  } catch {
    return fallback(task, "api_error", 0);
  }
  if (response.ok && typeof body?.output_text === "string") {
    const usage = body.usage ?? {};
    const cost = Number(usage.cost_usd);
    return JSON.stringify({
      text: body.output_text,
      outcome: "recorded",
      fallback_used: false,
      cache_hit: false,
      model_id: typeof body.model === "string" ? body.model : null,
      estimated_cost_usd: Number.isFinite(cost) && cost >= 0 ? cost : 0,
      api_request_count: 1,
      input_tokens: count(usage.input_tokens),
      output_tokens: count(usage.output_tokens),
    });
  }
  const code = typeof body?.code === "string" ? body.code : `http_${response.status}`;
  if (NO_LINK.has(code)) return null;
  return fallback(task, OUTCOMES[code] ?? "api_error", CALLED.has(code) ? 1 : 0);
}
