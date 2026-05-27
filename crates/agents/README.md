# medical-agents

Agentic orchestrator with tool use for FerriScribe's clinical AI chat.

This crate implements a multi-step reasoning loop: an AI provider receives a
user message plus tool definitions, decides which tools to invoke, gets back
structured results, and synthesises a final response. Eight specialised agents
share five built-in tools (ICD lookup, drug interactions, vitals extraction,
RAG knowledge-base search, and clinical checklists).

> **Audience:** future-you returning to this crate after months away.

---

## How It Fits in the Workspace

```
medical-core  ──(traits, types)──▶  medical-agents  ◀──(RAG backends)──  medical-rag
                                        │
                                        ▼
                                   src-tauri
                              (Tauri commands for chat)
```

| Relationship | Crate | What it provides / consumes |
|---|---|---|
| **Depends on** | `medical-core` | `Agent` and `Tool` traits, `AgentContext`, `AgentResponse`, `ToolDef`, `ToolOutput`, `CompletionRequest`, `AiProvider` trait, error types |
| **Depends on** | `medical-rag` | `EmbeddingGenerator`, `VectorStore`, `Bm25Search` — wired into `RagSearchTool` for knowledge-base search |
| **Used by** | `src-tauri` | Tauri commands that run chat sessions; creates an `AgentOrchestrator`, picks an agent, and calls `execute()` |

---

## Module Map

| Module | Purpose |
|---|---|
| `orchestrator` | `AgentOrchestrator` — the central loop that drives an `Agent` through iterative tool-use rounds via an `AiProvider`. |
| `agents` | Eight agent implementations (`ChatAgent`, `MedicationAgent`, `DiagnosticAgent`, `ComplianceAgent`, `DataExtractionAgent`, `WorkflowAgent`, `ReferralAgent`, `SynopsisAgent`) plus `all_agents()` factory. |
| `tools` | `ToolRegistry` and five tool implementations: `IcdLookupTool`, `DrugInteractionTool`, `VitalsExtractorTool`, `RagSearchTool`, `ChecklistTool`. |

---

## Key Types

### `AgentOrchestrator`

The heart of the crate. Holds a `ToolRegistry` and exposes a single async
method — `execute(agent, context, provider, model, temperature, cancel)` — that:

1. Builds a message list from conversation history, patient context, RAG
   context, and the current user message.
2. Filters the agent's declared tools against those actually registered.
3. Loops (up to `MAX_ITERATIONS = 10`):
   - Sends a `CompletionRequest` with tool definitions to the `AiProvider`.
   - If the provider returns tool calls, executes each via the `ToolRegistry`
     and appends results as `Role::Tool` messages.
   - If no tool calls are returned, the text content is the final answer.
4. Checks `CancellationToken` at the top of each iteration and after tool
   execution so the frontend can abort mid-run.

Returns an `AgentResponse` containing the final text, all tool-call records
(with durations), cumulative token usage, and iteration count.

### `Agent` trait (from `medical-core`)

Every agent implements:

- `name()` — short identifier (`"chat"`, `"medication"`, etc.)
- `description()` — one-line human summary
- `system_prompt()` — the system prompt prepended to every conversation
- `available_tools()` — the `ToolDef` list this agent may invoke
- `execute()` — **not called directly**; agents return an error here directing
  callers to `AgentOrchestrator::execute`

### `Tool` trait (from `medical-core`)

Each tool provides:

- `definition()` — a `ToolDef` (name, description, JSON Schema parameters)
- `execute(arguments)` — takes the model's JSON arguments, returns
  `ToolOutput` (content string + `is_error` flag)

### `ToolRegistry`

A `HashMap<String, Arc<dyn Tool>>` keyed by tool name. Provides:

- `new()` — empty registry
- `with_defaults()` / `Default` — pre-loads all five built-in tools
- `register(tool)` — add a custom tool
- `get(name)` — look up a tool by name
- `list_definitions()` — collect all `ToolDef`s (for forwarding to the model)

---

## The Eight Agents

| Agent | Name | Tools | Purpose |
|---|---|---|---|
| `ChatAgent` | `chat` | All 5 tools | General-purpose conversational medical assistant |
| `MedicationAgent` | `medication` | drug interactions, ICD lookup | Drug safety, dosage validation, Beers Criteria |
| `DiagnosticAgent` | `diagnostic` | ICD lookup, vitals extraction | Differential diagnosis and ICD-10 assignment |
| `ComplianceAgent` | `compliance` | checklist | SOAP note auditing and documentation compliance |
| `DataExtractionAgent` | `data_extraction` | vitals extraction | Structured data from unstructured clinical text |
| `WorkflowAgent` | `workflow` | checklist | Step-by-step clinical workflow guidance |
| `ReferralAgent` | `referral` | ICD lookup | Medical referral letter generation |
| `SynopsisAgent` | `synopsis` | _(none)_ | Concise SOAP note summaries under 200 words |

Use `all_agents()` to get all eight as `Vec<Box<dyn Agent>>`.

---

## The Five Built-in Tools

| Tool | Registered name | What it does |
|---|---|---|
| `IcdLookupTool` | `search_icd_codes` | Substring search over a hardcoded list of common ICD-10 codes. |
| `DrugInteractionTool` | `lookup_drug_interactions` | Pairwise check against a hardcoded interaction table (8 known pairs). Returns severity + clinical guidance. |
| `VitalsExtractorTool` | `extract_vitals` | Regex extraction of BP, HR, temperature, RR, and SpO2 from free-form clinical text. Range-validates each value. |
| `RagSearchTool` | `search_knowledge_base` | Hybrid search: embed query → vector similarity + BM25 → reciprocal rank fusion → top-k. Two constructors: `new()` (stub, returns "not connected") and `with_rag(...)` (wired to real backends). |
| `ChecklistTool` | `generate_checklist` | Returns numbered step-by-step checklists for new-patient visits, follow-ups, or a general fallback. |

---

## How It Works — Chat Message Flow

```
User message + conversation history
    │
    ▼
AgentOrchestrator::execute(agent, context, provider, ...)
    │
    ├─ build_messages(context)
    │     ├─ conversation_history
    │     ├─ patient_context → System message (medications, conditions, allergies)
    │     ├─ rag_context → System message (scored excerpts)
    │     └─ user_message → User message
    │
    ├─ Filter agent.available_tools() against tool_registry
    │
    ▼
  ┌─ Loop (max 10 iterations, cancellation-checked each round) ──┐
  │                                                                │
  │  provider.complete_with_tools(request, tool_defs)              │
  │       │                                                        │
  │       ├── No tool calls → return AgentResponse(final text)     │
  │       │                                                        │
  │       └── Tool calls requested:                                │
  │             │                                                  │
  │             ├─ Append assistant message (with tool_calls)      │
  │             ├─ For each tool call:                             │
  │             │    tool_registry.get(name).execute(arguments)    │
  │             │    → append ToolOutput as Role::Tool message     │
  │             │    → record AgentToolCallRecord                  │
  │             │                                                  │
  │             └─ Continue loop ─────────────────────────────────┘
```

---

## Examples

### Running a chat session

```rust
use medical_agents::orchestrator::AgentOrchestrator;
use medical_agents::agents::ChatAgent;
use medical_agents::tools::ToolRegistry;
use medical_core::types::AgentContext;
use tokio_util::sync::CancellationToken;

let registry = ToolRegistry::with_defaults();
let orchestrator = AgentOrchestrator::new(registry);

let context = AgentContext {
    user_message: "What are the ICD-10 codes for hypertension?".into(),
    conversation_history: vec![],
    patient_context: None,
    rag_context: vec![],
    recording: None,
};

let response = orchestrator
    .execute(
        &ChatAgent,
        context,
        &my_ai_provider,
        "local-model-name",
        0.7,
        CancellationToken::new(),
    )
    .await?;

println!("{} tool calls, {} iterations", response.tool_calls_made.len(), response.iterations);
```

### Registering a custom tool

```rust
use std::sync::Arc;
use medical_agents::tools::ToolRegistry;

let mut registry = ToolRegistry::with_defaults();
registry.register(Arc::new(MyCustomTool));
let orchestrator = AgentOrchestrator::new(registry);
```

---

## Gotchas

### Tool selection is model-driven, not code-driven

The orchestrator sends all available tool definitions to the AI provider on
every iteration. The **model** decides which tools to call and with what
arguments. There is no code-side routing or intent classification — tool
selection quality depends entirely on the model's function-calling ability and
the clarity of each tool's `description` and `parameters` schema.

### Agent tool lists are advisory filters

An agent's `available_tools()` declares which tools it *may* use, but the
orchestrator further intersects this with what is actually registered. If a
tool name appears in `available_tools()` but is missing from the registry, it
is silently dropped — the model never sees it. This is intentional: it lets
the app ship with all agents even when optional backends (like RAG) are not
yet configured.

### Recursion limit

`MAX_ITERATIONS` is hardcoded to 10. If the model keeps requesting tool calls
without producing a final text response, the orchestrator aborts with
`AppError::Agent("max iterations ...")`. This prevents runaway loops but
means complex multi-tool queries can fail if the model is indecisive. If you
see this in practice, consider whether the tool descriptions are too vague or
overlapping.

### Error handling in tool execution

Tool errors are **not** fatal. If a tool's `execute()` returns `Err(...)`,
the orchestrator catches it and converts it to a `ToolOutput` with
`is_error = true`, which the model sees as a tool-result message. The model
can then acknowledge the failure and adjust its response. Only provider-level
errors (network failures, bad API responses) abort the loop.

### `RagSearchTool` has two constructors

`RagSearchTool::new()` creates a stub that returns an informational "not yet
connected" message — useful during development or when RAG is not configured.
`RagSearchTool::with_rag(embeddings, vector_store, bm25)` wires it to real
backends. `ToolRegistry::with_defaults()` uses the stub constructor; the
Tauri layer is responsible for replacing it with a wired instance once the
RAG system is initialised.

### Agents don't self-execute

Every agent's `execute()` method returns an error directing callers to
`AgentOrchestrator::execute`. The `Agent` trait's `execute` method exists for
interface completeness, but the orchestrator owns the reasoning loop. Never
call `agent.execute()` directly.

### Cancellation is checked twice per iteration

`CancellationToken` is checked at the top of each loop iteration and again
after tool execution. Long-running tools (especially RAG search) will not be
interrupted mid-execution — cancellation only takes effect between steps.

### Model and temperature are caller-supplied

The orchestrator does **not** hardcode a model or temperature. Both are
forwarded from the caller on every `CompletionRequest`, so the Tauri layer
must source them from user settings for the active provider.
