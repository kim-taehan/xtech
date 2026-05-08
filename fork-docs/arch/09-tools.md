# 09 - Tool 시스템

이 문서는 Codex 가 모델에 노출하는 "도구(tool)" 시스템 — 빌트인 도구 정의, 등록 흐름, dispatch, 그리고 변형(variant) — 을 정리한다. 외부 MCP 도구는 [`12-mcp.md`](./12-mcp.md) 에서 별도로 다루며, 이 문서에서는 인터페이스 지점만 짚는다.

## 1. 빌트인 도구 목록

빌트인 도구의 *스펙* 은 `codex-rs/tools/` 크레이트에서 정의되고, *실행 핸들러* 는 `codex-rs/core/src/tools/handlers/` 에 산다.

| 도구 이름           | 책임                                                                       | 스펙 모듈                            | 핸들러                          |
| ------------------ | -------------------------------------------------------------------------- | ----------------------------------- | ----------------------------- |
| `shell`             | 배열형 argv (`["bash","-lc",...]`) 로 단일 명령 실행                       | `tools/local_tool.rs::create_shell_tool` | `handlers/shell.rs::ShellHandler` |
| `shell_command`     | 사용자 기본 셸에 단일 문자열 스크립트 전달                                  | `tools/local_tool.rs::create_shell_command_tool` | `handlers/shell.rs::ShellCommandHandler` |
| `local_shell`       | OpenAI Responses API 의 first-party `local_shell` 변형                     | `tools/tool_spec.rs::create_local_shell_tool` | `handlers/shell.rs::LocalShellHandler` |
| `exec_command` / `write_stdin` | unified-exec — PTY 세션을 만들고 stdin 으로 추가 입력 전송           | `tools/local_tool.rs` (`create_exec_command_tool`, `create_write_stdin_tool`) | `handlers/unified_exec.rs::ExecCommandHandler`, `WriteStdinHandler` |
| `apply_patch`       | 텍스트 패치 (Begin Patch / End Patch 그래마) 를 파일시스템에 적용          | `tools/apply_patch_tool.rs`         | `handlers/apply_patch.rs::ApplyPatchHandler` |
| `view_image`        | 사용자가 첨부한 이미지를 다음 turn 의 multimodal payload 로 끌어올림        | `tools/view_image.rs`               | `handlers/view_image.rs::ViewImageHandler` |
| `tool_search`       | 클라이언트 사이드 도구 검색 (지연 로드된 MCP/dynamic 도구 탐색용)           | `tools/tool_discovery.rs::create_tool_search_tool` | `handlers/tool_search.rs::ToolSearchHandler` |
| `web_search`        | OpenAI Responses API 의 `web_search` (서버 사이드, cached/live 모드)        | `tools/tool_spec.rs::create_web_search_tool` | (서버 측 처리 — 클라 핸들러 없음) |
| `image_generation`  | OpenAI Responses API 의 `image_generation` (이미지 입력 모달리티 필요)      | `tools/tool_spec.rs::create_image_generation_tool` | (서버 측 처리)                |
| `update_plan`       | Codex 의 plan/checklist 도구                                              | `tools/plan_tool.rs`                 | `handlers/plan.rs`            |
| `request_user_input`, `request_permissions`, `request_plugin_install` | 사용자 상호작용/권한 elicitation     | `tools/request_*` 모듈              | 동명의 `handlers/`             |
| `list_mcp_resources`, `list_mcp_resource_templates`, `read_mcp_resource` | MCP 리소스 헬퍼                      | `tools/mcp_resource_tool.rs`        | `handlers/mcp_resource.rs`    |
| `spawn_agent`, `wait_agent`, `close_agent`, `send_message`, `followup_task`, `list_agents`, `resume_agent`, `send_input` | 멀티 에이전트(서브-스레드) 도구 v1/v2 | `tools/agent_tool.rs`            | `handlers/multi_agents*.rs`   |
| `spawn_agents_on_csv`, `report_agent_job_result` | 에이전트 잡 러너                                            | `tools/agent_job_tool.rs`           | `handlers/agent_jobs.rs`      |
| `get_goal`, `create_goal`, `update_goal` | 목표(goal) 관리                                                   | `tools/goal_tool.rs`                | `handlers/goal.rs`            |
| `code_mode` / `wait` | "코드 모드" — 다른 도구를 코드 안에서 합성 호출                           | `tools/code_mode.rs`                | `tools/code_mode/*.rs`        |

빌트인 외에 두 가지 *동적* 카테고리가 있다.

- **MCP 도구** — 연결된 MCP 서버에서 발견되는 도구. `handlers/mcp.rs::McpHandler` 가 namespaced tool name 을 받아 `mcp_connection_manager` 로 전달.
- **Dynamic / Discoverable 도구** — `DynamicToolSpec` 으로 런타임 주입되는 도구. `handlers/dynamic.rs::DynamicToolHandler` 가 처리.

## 2. 등록 흐름

도구 시스템의 데이터 흐름은 다음과 같다.

```
ToolsConfig (codex-tools)
    │
    ▼
build_tool_registry_plan()  ──► ToolRegistryPlan { specs[], handlers[] }
    │                                (codex-rs/tools/src/tool_registry_plan.rs)
    ▼
build_specs_with_discoverable_tools()       (core/src/tools/spec.rs)
    │  for each ToolHandlerKind in plan.handlers → register_handler(Arc<...>)
    ▼
ToolRegistry  (HashMap<ToolName, Arc<dyn AnyToolHandler>>)
    │
    ▼
ToolRouter::from_config(...)  → 모델에게 노출할 spec 리스트 + dispatch 가능한 registry
```

### 2.1 `ToolSpec` 모양

`codex-rs/tools/src/tool_spec.rs` 의 `ToolSpec` 이 모든 도구 스펙의 sum type 이다.

```rust
pub enum ToolSpec {
    Function(ResponsesApiTool),     // 일반 함수 호출 도구
    Namespace(ResponsesApiNamespace),
    ToolSearch { execution, description, parameters },
    LocalShell {},
    ImageGeneration { output_format },
    WebSearch { external_web_access, filters, ... },
    Freeform(FreeformTool),         // grammar 기반 (apply_patch freeform)
}
```

`ResponsesApiTool` 은 OpenAI Responses API 모양 그대로 — `name`, `description`, `strict`, `parameters: JsonSchema`. JSON Schema 는 `tools/json_schema.rs` 의 자체 타입이며, `JsonSchema::object(properties, required, additional_properties)` 같은 빌더로 조립한다 (`apply_patch_tool.rs`, `local_tool.rs` 참조).

### 2.2 등록 결정 트리

`build_tool_registry_plan()` (in `tools/src/tool_registry_plan.rs`) 이 **실제 어떤 도구가 주입되는지** 를 결정한다. 플로우 요약:

1. `code_mode_enabled` → `code_mode` + `wait` 도구 추가, 다른 빌트인은 `for_code_mode_nested_tools()` 로 wrap.
2. `environment_mode.has_environment()` 일 때 `shell_type` 분기로 셸 도구 push.
3. `mcp_tools.is_some()` → list/read MCP resource 도구 push.
4. `update_plan` 은 항상 push.
5. feature 게이트(`goal_tools`, `request_permissions_tool_enabled`, `tool_suggest`, …)별로 추가.
6. `apply_patch_tool_type` (Freeform vs Function) 분기.
7. `web_search_mode` 가 `Some` 이면 `web_search` push.
8. `image_gen_tool` → `image_generation`.
9. `view_image` 추가.
10. `collab_tools` (멀티 에이전트) v1/v2 분기.

`ToolsConfig` 자체는 `tools/src/tool_config.rs::ToolsConfig::new(params)` 에서 model preset (`ModelInfo`) + `Features` 조합으로 결정된다.

## 3. 호출 dispatch

### 3.1 흐름

모델이 tool call (`function_call`) 을 SSE 로 보내면:

1. `core/src/tools/router.rs::ToolRouter::build_tool_call(session, item)` 가 `ResponseItem` variant 별로 분기해 `ToolCall { tool_name: ToolName, call_id, payload: ToolPayload }` 를 만든다.
   - `FunctionCall` → 일반 함수 호출. `session.resolve_mcp_tool_info()` 로 MCP 캐시를 먼저 조회해서, 매칭되면 `ToolPayload::Mcp { server, tool, raw_arguments }`, 아니면 `ToolPayload::Function { arguments }` 로 라우팅.
   - `LocalShellCall` → `ShellToolCallParams` 를 합성해 `ToolPayload::LocalShell { params }`.
   - `CustomToolCall` → freeform 도구. `ToolPayload::Custom { input }`.
   - `ToolSearchCall` (execution=`client`) → `ToolPayload::ToolSearch { arguments }`.
2. `ToolRouter::dispatch_tool_call_with_code_mode_result(...)` 가 `ToolInvocation { session, turn, cancellation_token, tracker, call_id, tool_name, source, payload }` 를 만들어 `ToolRegistry::dispatch_any(invocation)` 으로 넘긴다 (`core/src/tools/registry.rs:261`).
3. `dispatch_any` 의 시퀀스:
   - active turn 의 `tool_calls` 카운터 증가 (turn budget tracking).
   - `ToolDispatchTrace::start` 로 OTEL/로깅 trace 시작.
   - `HashMap<ToolName, Arc<dyn AnyToolHandler>>` 에서 핸들러 lookup. 없으면 `unsupported_tool_call_message` 로 모델에 에러 응답.
   - `handler.matches_kind(payload)` 로 `Function` vs `Mcp` 페이로드 호환성 검사.
   - `pre_tool_use_payload(invocation)` → `run_pre_tool_use_hooks(...)` (사용자 hook 이 거부하면 short-circuit).
   - `handler.handle(invocation).await` 본 실행.
   - `post_tool_use_payload(...)` → `run_post_tool_use_hooks(...)`.
   - 결과를 `AnyToolResult` 로 wrap → `ResponseInputItem` 으로 변환되어 다음 turn 입력에 들어간다.

### 3.2 `ToolHandler` trait

`registry.rs:44` 에 정의된 native RPITIT trait — `#[async_trait]` 이 아니라 `impl Future<Output=...> + Send` 반환.

```rust
pub trait ToolHandler: Send + Sync {
    type Output: ToolOutput + 'static;
    fn tool_name(&self) -> ToolName;
    fn kind(&self) -> ToolKind;       // Function | Mcp

    fn matches_kind(&self, payload: &ToolPayload) -> bool { /* default */ }
    fn is_mutating(&self, _: &ToolInvocation)
        -> impl Future<Output = bool> + Send { async { false } }
    fn pre_tool_use_payload(&self, _: &ToolInvocation) -> Option<PreToolUsePayload> { None }
    fn post_tool_use_payload(&self, _: &ToolInvocation, _: &Self::Output)
        -> Option<PostToolUsePayload> { None }
    fn create_diff_consumer(&self) -> Option<Box<dyn ToolArgumentDiffConsumer>> { None }

    fn handle(&self, invocation: ToolInvocation)
        -> impl Future<Output = Result<Self::Output, FunctionCallError>> + Send;
}
```

`AnyToolHandler` 는 trait-object 화된 erased 버전 (private trait + blanket impl) 으로 레지스트리에 들어간다 — 각 핸들러의 `type Output` 가 다르므로 erase 가 필요하다.

`is_mutating` 은 hook 시스템과 audit log 가 사용한다. `ShellHandler` 는 `is_known_safe_command(command)` 로 read-only 추정이 가능하면 false 를 돌려 사용자 승인 prompt 를 건너뛴다.

`create_diff_consumer` 는 streaming 중 부분 인자(JSON delta) 를 `EventMsg` 로 변환하는 hook — apply_patch 가 패치 헤더 파싱을 부분적으로 발화시켜 UI 에 바로 반영하기 위해 쓴다.

### 3.3 ToolName 과 namespace

`codex_protocol::ToolName { namespace: Option<String>, name: String }`. namespace 없는 경우 `ToolName::plain("shell")`, MCP 도구는 `ToolName::namespaced("server-A", "search")`. registry 의 lookup key 도 `ToolName` 이므로 같은 단순 이름이 다른 server 에 있어도 충돌하지 않는다.

## 4. Shell tool 변형

`ConfigShellToolType` (`codex-rs/protocol/src/openai_models.rs`) 다섯 변형은 다음과 같이 다르다.

| 변형            | tool 이름               | 입력 schema            | 백엔드                                | 사용처                                                           |
| --------------- | ---------------------- | ---------------------- | ------------------------------------- | ---------------------------------------------------------------- |
| `Default`       | `shell`                | `command: string[]`    | execvp 직접 호출                      | argv 로 안전하게 분리 가능한 모델 (대부분의 GPT-5 family)         |
| `Local`         | `local_shell`          | (서버 정의)            | OpenAI Responses API 가 native 처리   | OpenAI 가 first-party local-shell 도구를 노출하는 모델 preset    |
| `UnifiedExec`   | `exec_command` + `write_stdin` | PTY 세션 ID 기반   | `codex_utils_pty` ConPTY/pty fork      | TTY 가 필요한 인터랙티브 명령 (REPL, top, ssh) — Windows 에서는 ConPTY 미지원 시 자동 fallback |
| `Disabled`      | (없음)                 | -                      | -                                     | 셸을 모델에 노출하지 않는 모드 (read-only/checker 에이전트)       |
| `ShellCommand`  | `shell_command`        | `command: string`      | 사용자 기본 셸 (`zsh`/`bash`/`pwsh`)   | 단일 스크립트 문자열을 그대로 user shell 에 넘기는 fork 친화 모드 |

선택 로직은 `ToolsConfig::new` (`tool_config.rs:184-209`) 에 있다. 우선순위:

1. `Feature::ShellTool` 비활성 → `Disabled`.
2. `Feature::ShellZshFork` → 무조건 `ShellCommand`.
3. `Feature::UnifiedExec` 활성 + ConPTY 지원 → `UnifiedExec`, 미지원이면 `ShellCommand`.
4. 그 외엔 `model_info.shell_type` 그대로 (대부분 `Default`).

`ShellCommandBackendConfig::ZshFork` 는 zsh 재실행 wrapper 와 결합되어 사용자 dotfile/PATH/alias 까지 완전히 재현한다 (UnifiedExecShellMode::ZshFork).

## 5. apply_patch tool

두 형태로 노출된다 (`tools/apply_patch_tool.rs`).

- **Freeform** (`create_apply_patch_freeform_tool`) — `ToolSpec::Freeform`. Lark 그래마(`tool_apply_patch.lark`) 를 그대로 임베드해 모델이 raw `*** Begin Patch ... *** End Patch` 텍스트를 출력한다. **GPT-5 계열 권장.**
- **Function** (`create_apply_patch_json_tool`) — `ToolSpec::Function`. `{"input": "<patch text>"}` 모양의 JSON. 모델이 freeform 출력을 잘 못 하는 경우(특히 gpt-oss 계열)에 사용.

선택은 `ToolsConfig::apply_patch_tool_type` 으로 결정 — model preset 의 `ApplyPatchToolType` 을 그대로 따르고, 없으면 `Feature::ApplyPatchFreeform` 활성 시 freeform 으로 default.

핸들러는 변형과 무관하게 `core/src/tools/handlers/apply_patch.rs::ApplyPatchHandler` 하나. 실제 실행은 `core/src/tools/runtimes/apply_patch.rs::ApplyPatchRuntime` 가 sandbox 정책 + 사용자 승인을 거쳐 처리한다. 스트리밍 중에는 `ApplyPatchArgumentDiffConsumer` 가 부분 패치를 파싱해 `PatchApplyUpdated` 이벤트를 흘려보낸다.

## 6. search tool / web_search

- **`tool_search`** — `Feature::ToolSearch` (default on, stable) + `model_info.supports_search_tool` 둘 다 만족할 때만 등록. 지연 로드(`defer_loading: true`) 인 MCP/dynamic 도구 카탈로그를 모델이 키워드로 조회할 수 있게 한다. 핸들러는 클라이언트 사이드 (`ToolSearchHandler`).
- **`web_search`** — 모델 서버 사이드 도구. `web_search_mode` 가 `Cached` / `Live` 중 하나여야 등록되고, `Disabled` 또는 `None` 이면 spec 자체가 빠진다.

### 폐쇄망에서 끄는 방법

`config.toml`:

```toml
web_search = "disabled"            # 또는 [tools] web_search 항목 제거

[features]
tool_search = false                # 도구 검색까지 끔
```

또는 model preset 자체에서 `supports_search_tool = false`, `web_search_tool_type = "text"` 등으로 모델이 신호하지 않도록 조정. 본 fork 의 Qwen 게이트웨이 preset 은 web_search/tool_search 모두 끈 채로 시작한다 — 06-config.md 의 "remote 기본값" 참조.

## 7. 외부 도구 (MCP)

MCP 도구는 등록 흐름에 따로 합류한다 (`spec.rs:117-128`, `build_tool_registry_plan` 의 `mcp_tools` 분기). 핸들러는 단일 `McpHandler` 하나가 namespaced tool name 으로 인스턴스화되어 `mcp_connection_manager` 를 통해 호출을 forward 한다. 자세한 connection lifecycle, namespace 규칙, deferred loading 정책은 [`12-mcp.md`](./12-mcp.md) 참조.

## 8. Chat completions 변환

OpenAI Chat Completions wire path 에서는 도구 spec 모양이 다르다 (`{"type":"function","function":{name,description,parameters}}`). 변환은 `tools/src/tool_spec.rs::create_tools_json_for_chat_completions_api()` 가 담당 — 내부적으로 `create_tools_json_for_responses_api()` 를 먼저 호출해 Responses 모양 JSON 을 얻은 뒤, `type == "function"` 이 아닌 항목 (`web_search`, `image_generation`, `namespace`, `local_shell`, custom freeform 등) 은 **드롭** 하고 나머지는 `function` 키 아래로 재포장한다.

```rust
// 단순화된 변환
for mut tool in responses_api_tools_json {
    if tool.get("type") != Some(&"function") { continue; }
    let name = map.get("name").to_string();
    map.remove("type");
    out.push(json!({ "type": "function", "name": name, "function": map }));
}
```

호출 지점은 `core/src/client.rs:1587` 의 chat completions 분기. Responses 분기는 같은 파일 :679 에서 `create_tools_json_for_responses_api` 를 직접 사용한다. 자세한 wire payload 는 [`03-wire-protocol.md`](./03-wire-protocol.md#tools) 참조.

**이 fork 의 의의**: 본 fork 는 chat-completions 게이트웨이 (Qwen) 를 기본 사용하므로, **freeform / namespace / web_search / image_generation / local_shell 도구는 모델에 도달하기 전에 사라진다**. 따라서:

- `apply_patch` 는 `ApplyPatchToolType::Function` 으로 강제해야 한다 — model preset 또는 ConfigToml 의 apply_patch 설정으로. 그렇지 않으면 패치 도구가 무성공 dropped 된다.
- `web_search` / `image_generation` 은 사용 불가. `web_search_mode = "disabled"` 로 명시해 spec 자체를 빌드 단계에서 제거하는 게 깔끔하다.
- `local_shell` 모델 preset 도 안 통한다 — `Default` (`shell`) 또는 `ShellCommand` 로 강제.

## 9. 사용자가 새 tool 추가하려면

대략의 체크리스트 — "X 라는 새 함수형 도구" 를 추가한다고 가정.

1. **스펙 모듈 추가**: `codex-rs/tools/src/x_tool.rs` 에 `pub fn create_x_tool() -> ToolSpec` 작성. `ToolSpec::Function(ResponsesApiTool { name: "x", parameters: JsonSchema::object(...), ... })` 모양.
2. **`tools/src/lib.rs`** 에서 `mod x_tool;` + `pub use x_tool::create_x_tool;` 노출.
3. **핸들러 작성**: `codex-rs/core/src/tools/handlers/x.rs` 에 `pub struct XHandler;` + `impl ToolHandler for XHandler { ... }`. `ToolName::plain("x")`, `ToolKind::Function` 반환, `handle()` 에서 `parse_arguments<XArgs>()` 로 인자 deserialize 후 작업.
4. **핸들러 export**: `core/src/tools/handlers/mod.rs` 에서 `pub use x::XHandler;`.
5. **`ToolHandlerKind` 추가**: `codex-rs/tools/src/tool_registry_plan_types.rs` 의 enum 에 variant 추가.
6. **plan 분기**: `tools/src/tool_registry_plan.rs::build_tool_registry_plan` 안에 적절한 feature/config 조건 아래로 `plan.push_spec(create_x_tool(), parallel, code_mode)` + `plan.register_handler("x", ToolHandlerKind::X)` 호출.
7. **dispatch 매핑**: `core/src/tools/spec.rs::build_specs_with_discoverable_tools` 의 `match handler.kind` 에 `ToolHandlerKind::X => builder.register_handler(Arc::new(XHandler))` 추가.
8. **feature flag (옵션)**: 기본 비활성으로 출시할 거면 `codex-rs/features/src/lib.rs` 에 `Feature::X` + `FeatureSpec` 추가하고 `ToolsConfig::new` 에서 게이트.
9. **테스트**: `tool_spec` snapshot 테스트, handler 단위 테스트, 그리고 가능하면 `core_test_support::responses` 로 end-to-end tool-call → response 파이프라인 검증.

도구가 외부 API/MCP 라면 step 1-2 는 dynamic tool spec 으로 대체할 수 있다 (`DynamicToolSpec` + thread config) — 그 경우 코드 수정 없이 런타임 주입만으로 추가된다.

### 9.1 새 도구 추가 시 자주 빠뜨리는 것

- **`is_mutating` 미구현** — read-only 도구라도 default `false` 로 두면 hook 우회로 audit 누락이 생긴다. 의미가 분명하면 `async { false }` / `async { true }` 명시적으로 작성.
- **`matches_kind` 오버라이드 누락** — payload kind 가 default 와 다르면 dispatch 가 silently 실패한다 (e.g., custom freeform 도구는 `ToolPayload::Custom` 만 받아야 함).
- **`output_schema` 추가** — Responses API 에는 영향 없으나 chat completions wire 로 넘어가면 `#[serde(skip)]` 라 실제 wire 에는 빠진다. 그 점을 인지한 채로 모델 description 에 출력 모양을 적어둘 것.
- **테스트 fixture** — `tool_spec_tests.rs` 에 새 도구 spec snapshot 을 추가, `spec_tests.rs` 에 등록 분기 검증, 핸들러 단위 테스트는 `core_test_support::responses` 로 작성하자 (CLAUDE.md 의 테스트 가이드 참조).
- **fork 호환성** — 새 도구가 chat completions 로 살아 남으려면 반드시 `ToolSpec::Function` 변형으로 만들 것. `Freeform` / `Namespace` / `WebSearch` 등은 wire 단계에서 drop 된다 (§8 참조).

## 10. 참고 — 모듈 구조 한눈에

```
codex-rs/
├── tools/                       # 스펙 (모델/wire 에 노출되는 모양)
│   ├── tool_spec.rs            # ToolSpec sum type, chat/responses 변환
│   ├── tool_config.rs          # ToolsConfig, ConfigShellToolType 분기 입력
│   ├── tool_registry_plan.rs   # build_tool_registry_plan — *어떤* 도구를 등록할지 결정
│   ├── tool_registry_plan_types.rs  # ToolHandlerKind enum (스펙↔핸들러 매핑 키)
│   ├── local_tool.rs           # shell / shell_command / exec_command / write_stdin
│   ├── apply_patch_tool.rs     # freeform + json
│   ├── view_image.rs / plan_tool.rs / goal_tool.rs / agent_tool.rs / agent_job_tool.rs
│   ├── tool_discovery.rs       # tool_search, request_plugin_install
│   ├── mcp_tool.rs / mcp_resource_tool.rs
│   └── code_mode.rs            # code_mode 도구 + 다른 spec 들의 nested wrap
└── core/src/tools/             # 핸들러 (실제 실행 로직)
    ├── registry.rs             # ToolRegistry, ToolHandler trait, dispatch_any
    ├── router.rs               # ResponseItem → ToolCall, dispatch entrypoint
    ├── spec.rs                 # build_specs_with_discoverable_tools (핸들러 instantiate)
    ├── orchestrator.rs / sandboxing.rs / parallel.rs
    ├── handlers/
    │   ├── shell.rs            # ShellHandler, ShellCommandHandler, ContainerExecHandler, LocalShellHandler
    │   ├── unified_exec.rs     # ExecCommandHandler, WriteStdinHandler
    │   ├── apply_patch.rs      # ApplyPatchHandler + 스트리밍 diff consumer
    │   ├── view_image.rs / plan.rs / goal.rs / mcp.rs / mcp_resource.rs
    │   ├── tool_search.rs / request_user_input.rs / request_permissions.rs
    │   ├── multi_agents.rs (v1) / multi_agents_v2.rs (v2)
    │   └── agent_jobs.rs / dynamic.rs / unavailable_tool.rs / test_sync.rs
    ├── runtimes/               # 실제 OS/PTY/패치 실행 (sandbox 통합)
    │   ├── shell.rs / unified_exec.rs / apply_patch.rs
    └── code_mode/              # code_mode 도구의 합성 실행기
```

문서를 따라 읽을 때는 보통 *spec 정의 → registry plan 분기 → spec.rs 의 핸들러 매핑 → handler 구현 → runtime* 순서로 trace 하면 된다.
