// The tasks the LLM relay (relay.mjs, #419) answers at `/llm/v1/tasks/{name}`: each one builds its
// prompt here, in the relay, from the page's `input`, so the page can send data only, never a
// prompt of its own. None is offered yet; reading a Scenario from text comes with #421.
//
// A task: {
//   model,            a model of relay.mjs's PRICES (Python's model ids)
//   promptVersion,    recorded with the answer, as Python's tasks record theirs
//   maxOutputTokens,  the answer's size (thinking's room is added on Opus)
//   maxPromptBytes,   the UTF-8 bytes of { system, messages } it accepts
//   effort?,          output_config.effort (Python asks "low" for preparation)
//   disableThinking?, thinking: { type: "disabled" } (Haiku's runtime tasks)
//   timeoutMs?,       how long the call may take (120 s by default)
//   build(input),     → { system, messages }, or throws relay.mjs's TaskInputError
// }

export const TASKS = Object.freeze({});
