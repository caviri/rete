# WebMCP: the parts of the spec a page uses

Source: <https://webmachinelearning.github.io/webmcp/>, Draft Community Group
Report, 24 November 2026 (W3C Web Machine Learning CG, not a W3C Standard).
WebMCP is still changing; re-read the spec before relying on anything new.

## Where it lives

```webidl
partial interface Document {
  [SecureContext, SameObject] readonly attribute ModelContext modelContext;
};

[Exposed=Window, SecureContext]
interface ModelContext : EventTarget {
  Promise<undefined> registerTool(ModelContextTool tool, optional ModelContextRegisterToolOptions options = {});
  undefined waitForToolsUntil(Promise<any> promise);
  Promise<sequence<Tool>> getTools(optional ModelContextGetToolOptions options = {});
  Promise<DOMString> executeTool(Tool tool, optional object inputObject, optional ModelContextExecuteToolOptions options = {});
  attribute EventHandler ontoolchange;
  attribute EventHandler ontoolactivated;   // ToolActivatedEvent { toolName }
  attribute EventHandler ontoolcancel;      // ToolCancelEvent { toolName }
};

dictionary ModelContextRegisterToolOptions {
  sequence<USVString> exposedTo;   // origins that may see the tool
  AbortSignal signal;              // abort to unregister
};
```

`document.modelContext` is the only location in the spec. Earlier Chrome previews
used `navigator.modelContext`, `provideContext({ tools })`, `clearContext()` and
`unregisterTool(name)`; none of those are in the current draft. The kit tries the
spec first and falls back to them.

## A tool

```webidl
dictionary ModelContextTool {
  required DOMString name;           // 1–128 chars: ASCII letters, digits, "_", "-", "."
  USVString title;
  required DOMString description;
  object inputSchema;                // JSON Schema
  object outputSchema;
  required ToolExecuteCallback execute;
  ToolAnnotations annotations;
};

dictionary ToolAnnotations {
  boolean readOnlyHint = false;
  boolean untrustedContentHint = false;
  boolean consequentialHint = false;
  boolean debugging = false;
};

callback ToolExecuteCallback = Promise<any> (object inputObject, ToolExecuteCallbackOptions options);
dictionary ToolExecuteCallbackOptions { required AbortSignal signal; };
```

- `execute` receives the parsed input and `{ signal }` (aborted when the agent cancels). There is no `client` argument in the current draft.
- Whatever `execute` resolves with is serialised to JSON for the agent. A rejected promise, or a value that cannot be serialised, reaches the agent as an `OperationError`.
- Aborting the registration signal removes the tool; calls already running are not cancelled.
- `registerTool` rejects with `NotAllowedError` when the `tools` permissions policy is off, e.g. in an iframe without `allow="tools"`.

## Not in the spec (yet)

A declarative API (form attributes that turn a `<form>` into a tool) is being
explored in a separate explainer and is not part of the draft. Do not rely on it.

## Trying it

Chrome with `chrome://flags/#enable-webmcp-testing` exposes `document.modelContext`;
the Model Context Tool Inspector extension lists and runs a page's tools.
