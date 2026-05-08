# 12. MCP (Model Context Protocol) integration

이 문서는 xtech fork 의 MCP 통합을 정리한다. MCP 가 codex 안에서 어떻게 양방향으로 동작하는지 — 외부 MCP 서버를 도구로 끌어다 쓰는 클라이언트 경로, 그리고 codex 자체를 MCP 서버로 노출하는 `xtech mcp-server` 경로 — 와, 이 두 경로가 공통으로 의존하는 OAuth/transport/RMCP 어댑터 레이어를 한 번에 잡는 게 목적이다. 폐쇄망 운영에서 “외부 MCP 서버가 어디로 통신하는가” 라는 운영자의 질문도 끝에서 다룬다.

## 1. MCP 가 무엇인지

MCP (Model Context Protocol) 는 LLM 호스트가 **외부 도구 서버** 를 표준 JSON-RPC 메시지로 붙이기 위한 프로토콜이다. 서버는 `tools/list`, `tools/call`, `resources/list`, `resources/read`, `prompts/*`, elicitation 같은 능력을 제공하고, 호스트는 이 능력들을 모델에게 함수 호출 형태로 노출한다. 전송 (transport) 은 두 가지가 정착됐다 — **stdio** (자식 프로세스의 stdin/stdout) 와 **streamable HTTP** (HTTP+SSE 스타일의 양방향 채널). xtech 는 두 transport 모두 지원하며, 내부적으로는 [`rmcp`](https://crates.io/crates/rmcp) crate 의 모델/transport 타입을 쓴다.

## 2. 두 방향: client vs server

xtech 가 MCP 와 만나는 시나리오는 정확히 두 가지다.

| 방향 | 누가 누구를 부르나 | 진입점 | 주 대상 |
| --- | --- | --- | --- |
| **MCP client** | xtech → 외부 MCP 서버 | `codex-rs/codex-mcp/` (`McpConnectionManager`) | 사용자가 `mcp_servers` 에 등록한 서버 |
| **MCP server** | 외부 호스트 (Claude Desktop, IDE 플러그인, 다른 codex …) → xtech | `codex-rs/mcp-server/` (`xtech mcp-server`) | `codex` 라는 단일 메가-tool 노출 |

두 경로는 같은 프로세스에서 동시에 살 수 있다. 예컨대 `xtech mcp-server` 자체가 안에서 또다시 `mcp_servers` 에 정의된 다른 MCP 서버에 클라이언트로 붙는다 — `MessageProcessor::new` 가 만드는 `ThreadManager` 가 일반 codex 세션과 동일한 config 를 그대로 로드하기 때문 (`codex-rs/mcp-server/src/message_processor.rs`).

핵심 약속: 모든 MCP 트래픽 (양방향 모두) 은 `codex-rs/rmcp-client/` 가 노출하는 `RmcpClient` 또는 rmcp 의 server-side 헬퍼를 거친다. 즉 transport/serde/타임아웃은 한 곳으로 모은다.

## 3. 클라이언트 측: `codex-mcp` 와 `McpConnectionManager`

### 3.1 등록

사용자는 `~/.xtech/config.toml` (또는 layer 시스템 어디든 — `arch/06-config.md` 참고) 의 `[mcp_servers.<name>]` 섹션에 서버를 등록한다. 파싱은 `codex-rs/config/src/mcp_types.rs::RawMcpServerConfig` → `McpServerConfig` 변환에서 일어나고 transport 는 `McpServerTransportConfig::Stdio { command, args, env, env_vars, cwd }` 또는 `McpServerTransportConfig::StreamableHttp { url, bearer_token_env_var, http_headers, env_http_headers }` 둘 중 하나가 된다. stdio 와 http 필드를 섞으면 `try_from` 단계에서 거부된다 (`throw_if_set` 헬퍼).

서버별 추가 옵션 — `enabled`, `required`, `startup_timeout_sec`, `tool_timeout_sec`, `enabled_tools` / `disabled_tools`, `default_tools_approval_mode`, `tools.<name>.approval_mode`, `scopes`, `oauth_resource` — 이 모두 `McpServerConfig` 에 정의돼 있다.

전형적인 두 가지 형태:

```toml
# stdio MCP 서버 — 자식 프로세스로 실행
[mcp_servers.fs]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/projects"]
startup_timeout_sec = 15
default_tools_approval_mode = "prompt"
disabled_tools = ["delete_file"]

# streamable HTTP MCP 서버 — 사외 / 사내 게이트웨이
[mcp_servers.acme]
url = "https://mcp.acme.example.com/v1"
bearer_token_env_var = "ACME_MCP_TOKEN"
scopes = ["read:tools", "execute:tools"]
oauth_resource = "https://mcp.acme.example.com"
required = true   # exec 시 startup 실패하면 종료
```

`bearer_token` 은 raw 값을 직접 넣을 수도 있지만 schema 에서 `#[schemars(skip)]` 로 빠져 있고 운영상 권장되지 않는다 — env 참조 (`bearer_token_env_var`) 가 표준 경로.

### 3.2 매니저 라이프사이클

`codex-rs/codex-mcp/src/connection_manager.rs::McpConnectionManager` 가 런타임 owner 다. 핵심 루틴:

- `McpConnectionManager::new(...)` — 등록된 서버 중 `enabled = true` 인 것마다 `AsyncManagedClient` 를 만들고, RMCP 핸드셰이크를 백그라운드 `JoinSet` 에 띄운다. 시작/종료는 `McpStartupUpdateEvent` / `McpStartupCompleteEvent` 로 이벤트 채널에 흘려보낸다.
- `list_all_tools()` — 서버별 `tools/list` 결과를 `mcp__<server>__<tool>` 형태로 qualify 해서 모은다 (`tools.rs::qualify_tools`, prefix 는 `qualified_mcp_tool_name_prefix`).
- `call_tool(server, tool, args, meta)` — 이름→server 라우팅, `ToolFilter` 검사, `RmcpClient::call_tool` 호출, 결과를 `codex_protocol::mcp::CallToolResult` 로 변환.
- `list_all_resources()` / `list_all_resource_templates()` / `read_resource()` — paginated cursor 기반 페치, 중복 cursor 감지 시 abort.
- `begin_shutdown()` / `Drop` — `CancellationToken` 으로 시작 중인 핸드셰이크까지 끊고 stdio 자식 프로세스를 죽인다.

> 컨벤션: **MCP 도구/호출 관련 변경은 모두 `mcp_connection_manager.rs` 를 통과시킬 것** — `CLAUDE.md` 가 명시한 가이드. 새로운 레이어를 그 옆에 끼워넣지 말고 manager 의 메서드를 확장하는 식으로 가야 한다.

### 3.3 도구가 모델에 노출되는 흐름

`McpConnectionManager::list_all_tools()` 의 결과는 `codex-rs/core/src/tools/spec.rs` 가 `ToolRegistryPlanMcpTool` 로 끌어와 `ToolRegistry` 에 등록한다. 모델이 turn 중에 `mcp__server__tool` 을 호출하면 `codex-rs/core/src/tools/registry.rs` 의 dispatch 가 `ToolPayload::Mcp { server, tool, arguments }` 분기로 떨어지고, 거기서 다시 manager 로 콜백한다 (registry.rs:288 부근). approval/permission 게이팅은 `McpPermissionPromptAutoApproveContext` 가 `default_tools_approval_mode` 와 sandbox 정책을 보고 결정한다 (`codex-mcp/src/mcp/mod.rs::mcp_permission_prompt_is_auto_approved`).

이름 sanitize 는 `tools.rs::sanitize_responses_api_tool_name` + `qualify_tools` 가 한다. raw MCP tool 이름은 OpenAI Responses API tool name 제약 (길이/문자 클래스) 을 만족 못 할 수 있어 `mcp__<server>__<tool>` prefix 를 붙인 뒤 재정렬 + 충돌 시 SHA1 해시 suffix 로 deduplicate. 즉 모델이 보는 이름과 manager 가 routing 키로 쓰는 raw `tool.name` 은 다를 수 있고, `ToolInfo` 가 양쪽을 모두 들고 다닌다 (`server_name`, `callable_namespace`, `callable_name`, raw `tool.name`).

### 3.4 elicitation 라우팅

MCP 서버가 사용자에게 입력을 요청 (`elicitation/create`) 하면 `codex-mcp/src/elicitation.rs::ElicitationRequestManager` 가 받아 처리한다. 분기:

- `auto_deny` 가 켜져 있으면 즉시 `ElicitationAction::Decline`.
- `mcp_permission_prompt_is_auto_approved` 가 true 면 자동 accept (no-op response).
- 그 외에는 `EventMsg::ElicitationRequest` 로 protocol event 를 흘려보내고 `oneshot::Sender` 를 `(server_name, request_id)` 키로 보관 → 사용자가 응답하면 `McpConnectionManager::resolve_elicitation` 으로 풀어낸다.

이 메커니즘은 `xtech mcp-server` 의 exec/patch approval 에도 그대로 재사용된다 (5.2 참고).

### 3.5 ChatGPT apps MCP 와 codex_apps

`CODEX_APPS_MCP_SERVER_NAME = "codex_apps"` 는 ChatGPT-hosted app tools 를 위한 내장 MCP 서버다. `McpConfig::apps_enabled` 가 켜지고 ChatGPT 인증이 살아 있을 때만 manager 에 자동 주입되며 (`with_codex_apps_mcp`), tools 는 `${CODEX_HOME}` 캐시에 적힌다 (`codex_apps.rs::write_cached_codex_apps_tools_if_needed`). 폐쇄망 fork 입장에서는 일반적으로 비활성 (`apps_enabled = false`) 이지만 코드 경로는 살아 있어 의도치 않게 켜지지 않도록 운영 체크 대상.

## 4. 서버 측: `codex-rs/mcp-server/`

### 4.1 진입점

`codex-rs/cli/src/main.rs:809` 의 `Subcommand::McpServer` 가 `codex_mcp_server::run_main` 을 부른다. `mcp-server/src/lib.rs` 의 구조는 단순 stdio JSON-RPC 루프:

1. `tokio::io::stdin` 에서 한 줄씩 읽어 `JsonRpcMessage<ClientRequest, …>` 로 디시리얼라이즈 → `incoming_tx` 채널에 push.
2. `MessageProcessor` 가 request 종류별 핸들러로 분기 (`process_request` — InitializeRequest, ListToolsRequest, CallToolRequest, ListResourcesRequest, GetPromptRequest, …).
3. 핸들러는 `OutgoingMessageSender` 로 응답/알림을 enqueue, 별도 task 가 `stdout` 으로 직렬화해 내보낸다.

`xtech mcp-server` 는 **다른 transport (HTTP) 를 노출하지 않는다** — 호스트 (e.g. Claude Desktop) 가 child process 로 띄워 stdio 로 붙는다는 게 전제. 외부에서 HTTP 로 닿게 하고 싶으면 별도 reverse-proxy 또는 wrapper 가 필요하다.

### 4.2 노출되는 능력

`mcp-server` 가 advertise 하는 것은 단 두 개의 “codex” tool 이다 (`codex-rs/mcp-server/src/codex_tool_config.rs`):

- **`codex`** (`CodexToolCallParam`) — 새 thread 를 만들어 prompt 를 돌리고, Codex 가 끝낼 때까지 진행 상황을 notification 으로 흘리고 최종 메시지를 `tools/call` 응답으로 돌려준다. 입력 스키마는 `prompt`, `model`, `profile`, `cwd`, `approval-policy`, `sandbox`, `config` (free-form override map), `base-instructions`, `developer-instructions`, `compact-prompt` 의 kebab-case 필드들.
- **`codex-reply`** (`CodexToolCallReplyParam`) — 기존 `threadId` 에 follow-up 메시지를 보낸다.

응답 포맷은 `codex_tool_runner.rs::create_call_tool_result_with_thread_id` 가 만든다. MCP `tools/call` 응답 규약상 `content` 는 텍스트 블록 리스트지만, 일부 클라이언트는 `structuredContent` 만 본다. 그래서 codex 는 같은 본문을 `content[0]` (text) 와 `structured_content.{threadId, content}` 양쪽에 넣어준다. 이게 “외부 호스트가 thread 를 이어가려면 threadId 가 필요” 한 데 대한 절충.

승인 흐름은 MCP elicitation 으로 매핑된다. exec 실행 / patch 적용 시 `ExecApprovalElicitRequestParams`, `PatchApprovalElicitRequestParams` 를 클라이언트에게 보내고 (`exec_approval.rs`, `patch_approval.rs`), 응답을 받아 codex turn 에 전달. 즉 MCP 호스트가 elicitation 을 지원하면 그 호스트의 UI 가 사용자 승인을 받는 면이 된다. 반대로 elicitation 미지원 호스트가 붙으면 승인 요청이 timeout 으로 떨어진다.

resource/prompt/subscribe/listResourceTemplates 같은 다른 MCP request 들은 대부분 “지원 안 함” 으로 빠지는 stub (`handle_unsupported_request`) 이라는 점을 알아두면 된다. tasks/* (`GetTaskInfoRequest`, `ListTasksRequest`, `GetTaskResultRequest`, `CancelTaskRequest`) 는 의도적으로 미구현 상태로 빠진다 — codex 는 long-running task 모델을 자체 thread 로 표현하지 MCP task 로 노출하지 않는다.

### 4.3 thread 관리

`MessageProcessor::new` 는 `ThreadManager` 를 일반 세션과 동일한 build 로 띄운다 — `init_state_db_from_config`, `thread_store_from_config`, `agent_graph_store_from_state_db`. 즉 **`xtech mcp-server` 로 호출된 thread 도 SQLite/파일 thread store 에 동일하게 기록된다.** `experimental_thread_store_endpoint` 가 설정돼 있으면 (`codex-rs/config/src/config_toml.rs:347`) app-server 와 마찬가지로 원격 thread store 로 라우트되는데, 여기 endpoint 를 가리키게 하면 mcp-server 도 자동으로 따라간다.

## 5. OAuth 흐름

streamable HTTP MCP 서버는 OAuth 2.1 (PKCE + dynamic client registration) 로 인증하는 게 흔하다. fork 의 OAuth 구현은 `codex-rs/rmcp-client/src/oauth.rs` 와 `perform_oauth_login.rs` 에 모여 있다.

저장 위치는 `mcp_oauth_credentials_store` 로 고른다 (`config_toml.rs:210`):

- `keyring` — OS 키체인 (macOS Keychain / Windows Credential Manager / Linux Secret Service or keyutils — `codex_keyring_store::DefaultKeyringStore`).
- `file` — `${CODEX_HOME}/.credentials.json` fallback.
- `auto` (default) — keyring 가능하면 keyring, 안 되면 file.

토큰은 `StoredOAuthTokens { server_name, url, client_id, token_response, expires_at }` 로 직렬화되며 만료 30 초 전부터 refresh 한다 (`REFRESH_SKEW_MILLIS`). 콜백 리다이렉트는 로컬호스트에 임시 HTTP listener 를 띄워 받는다 — port 는 `mcp_oauth_callback_port` 로 고정 가능, redirect URI 자체를 외부 호스트에 두려면 `mcp_oauth_callback_url` 을 쓴다 (단 listener 는 여전히 127.0.0.1).

scope 협상은 `codex-mcp/src/mcp/auth.rs::resolve_oauth_scopes` 가 담당한다. 우선순위: `scopes` 명시 (`Explicit`) → 서버 metadata `discovered_scopes` (`Discovered`) → 비어 있으면 `Empty`. `should_retry_without_scopes` 가 첫 시도 거부 시 scope 빼고 재시도할지 결정한다.

운영 진입점은 `xtech mcp login <name>` / `xtech mcp logout <name>` (`codex-rs/cli/src/mcp_cmd.rs::McpSubcommand::Login` / `Logout`). 시작 실패 메시지에는 “Run `codex mcp login {server}`” 안내가 자동으로 들어간다 (`connection_manager.rs::mcp_init_error_display`). 같은 CLI 가 `list` / `get` / `add` / `remove` 도 들고 있어 config.toml 을 직접 안 건드리고도 `~/.xtech/config.toml` 의 `mcp_servers` 섹션을 편집할 수 있다 (`codex-core::config::edit::ConfigEditsBuilder`).

OAuth 로그인 자체는 `codex-rs/rmcp-client/src/perform_oauth_login.rs::perform_oauth_login` 이 처리한다. 흐름:

1. `discover_streamable_http_oauth` — 서버의 `.well-known/oauth-authorization-server` (또는 `.well-known/openid-configuration`) 로 metadata fetch.
2. dynamic client registration 가능하면 client_id 발급, 아니면 config 의 `client_id` 사용.
3. PKCE pair 생성, 로컬 listener bind (`mcp_oauth_callback_port` 또는 ephemeral), `Authorization Code` 요청을 브라우저로 열기.
4. callback 받아 token exchange → `StoredOAuthTokens` 저장.
5. refresh 는 `AuthorizationManager` 가 백그라운드로 — `expires_at - REFRESH_SKEW_MILLIS` 가 지나면 자동.

`experimental_thread_store_endpoint` 는 MCP OAuth 와 무관한 별개 설정이지만 (실제로는 app-server 의 thread persistence 원격화 옵션), 같은 “experimental” 묶음이라 함께 자주 묻는다 — 4.3 참고.

## 6. `rmcp-client` 와 `codex-mcp` 의 차이

두 crate 는 역할이 분명히 갈린다.

| crate | 책임 |
| --- | --- |
| `codex-rs/rmcp-client/` | **Transport / wire 어댑터.** rmcp 의 `RunningService<RoleClient>` 위에 codex 가 필요로 하는 helper (timeout 래핑, OAuth, stdio launcher 두 종류, streamable HTTP 어댑터, elicitation client service, logging handler) 를 얹는다. 단일 서버 1개를 다루는 단위. codex 비즈니스 로직을 모른다. |
| `codex-rs/codex-mcp/` | **codex 측 통합.** 여러 `RmcpClient` 를 모아 manager 를 만들고, codex 의 tool registry / approval 시스템 / event bus / `codex_apps` 캐시 / config 와 연결한다. |

stdio 자식 프로세스를 띄우는 곳도 `rmcp-client` 안이다. `LocalStdioServerLauncher` 는 codex 가 직접 spawn 하고, `ExecutorStdioServerLauncher` 는 `codex-exec-server` 의 `EnvironmentManager` 를 거쳐 띄운다 (sandbox 환경 일관성). streamable HTTP 쪽은 `StreamableHttpClientTransport` + `AuthClient` (`rmcp_client.rs:47-50`) 조합을 쓰고, 커스텀 CA 가 필요하면 `codex-client::build_reqwest_client_with_custom_ca` 가 들어간다.

따라서 transport 자체에 손을 댈 일이 있으면 `rmcp-client/`, codex 의 의미론 (도구 노출 방식, 캐시, approval) 에 손을 댈 일이면 `codex-mcp/` 가 진입점이다.

또 한 가지 — `MCP_SANDBOX_STATE_META_CAPABILITY` (`codex/sandbox-state-meta`, `rmcp_client.rs` 에서 정의) 는 fork 가 추가한 vendor capability 다. MCP 서버가 initialize 단계에서 이 capability 를 advertise 하면 manager 는 `tools/call` 의 `_meta` 에 현재 sandbox state (`SandboxState`) 를 끼워서 보낸다. 즉 “sandbox-aware MCP 서버” 가 codex 가 어떤 권한 환경에서 부르는지 보고 행동을 바꿀 수 있게 하는 hook. `codex-rs/codex-mcp/src/runtime.rs::SandboxState` 가 그 페이로드 정의.

## 7. 테스트 / 디버깅

- **`codex-rs/mcp-server/tests/`** — `tests/all.rs` 가 `mod suite` 만 들고 있고, 실제 시나리오는 `tests/suite/codex_tool.rs` 에 모인다 (524 LoC). 도우미는 `tests/common/` 의 `mcp_process.rs` (mcp-server 바이너리를 띄우고 stdio JSON-RPC 로 대화하는 harness), `mock_model_server.rs` (responses API 모킹), `responses.rs` (SSE payload 빌더). 헬퍼는 모두 `codex_utils_cargo_bin` 의 `cargo_bin` / `find_resource!` 를 쓰므로 Bazel runfiles 호환.
- **`codex-rs/codex-mcp/src/connection_manager_tests.rs`** — manager 단위 테스트 (940 LoC). 가짜 server 를 띄워 startup 이벤트 (`Starting → Ready` / `Failed { error }` / `Cancelled`), tool filter (allow/deny list), codex_apps 캐시 hit/miss, 시작 timeout/auth 에러 메시지 포맷 (특히 GitHub MCP 분기), elicitation 라우팅 등을 검증.
- **`codex-rs/rmcp-client/tests/`** — `streamable_http_*.rs` 에서 HTTP transport 회복/재시도, `process_group_cleanup.rs` 에서 stdio 자식 프로세스 그룹 정리 (Unix 에서 SIGKILL → process group 까지 잡는지), `resources.rs` 에서 resource paginated 페치 (cursor 중복 검출 포함) 를 다룬다.
- **`codex-rs/config/src/mcp_types_tests.rs`** — TOML 입력 파싱. stdio/http 필드 mix 거부, 잘못된 `env_vars.source`, `startup_timeout_ms` ↔ `startup_timeout_sec` 호환, deprecated `name` 필드 무시 등.
- **`MCP_TOOL` 호출 흐름 디버깅** — `codex-rs/core/src/tools/registry.rs:288` 부근의 `ToolPayload::Mcp` 분기가 진입점. 거기서 `OtelTelemetry` 가 `mcp_server` / `mcp_server_origin` 태그로 metric 을 찍고 (`codex.mcp.tools.list.duration_ms`, `codex.mcp.tools.fetch_uncached.duration_ms`), `RUST_LOG=codex_mcp=debug,codex_rmcp_client=debug` 로 띄우면 매니저가 emit 하는 startup/shutdown trace 와 tools/list 캐시 hit/miss 가 다 보인다.

엔드투엔드 호출 흐름은 다음과 같다 (외부 stdio 서버 `fs` 의 `read_file` 도구를 모델이 호출하는 경우):

```
Model output  ──"mcp__fs__read_file"(args)──▶  ToolRegistry::dispatch
                                                   │
                                                   ▼
                                       ToolPayload::Mcp { server="fs", tool="read_file", … }
                                                   │
                                                   ▼
                                  McpConnectionManager::call_tool("fs", "read_file", args, meta)
                                                   │
                                                   ├─ ToolFilter::allows("read_file")  ── deny → error to model
                                                   │
                                                   ▼
                                           RmcpClient::call_tool
                                                   │
                                                   ▼
                                  rmcp StreamableHttpClientTransport / StdioServerTransport
                                                   │
                                                   ▼
                                  외부 MCP 서버 — JSON-RPC `tools/call`
                                                   │
                                                   ▼
                                  CallToolResult { content, structured_content, is_error, _meta }
                                                   │
                                                   ▼
                                  codex_protocol::mcp::CallToolResult (TS-friendly)
                                                   │
                                                   ▼
                                  ToolRegistry → 모델 turn 의 function_call_output
```

`call_tool` 이 timeout 으로 fail 하면 `tool_timeout_sec` (서버별) 또는 `DEFAULT_TOOL_TIMEOUT = 120s` (`rmcp_client.rs`) 가 적용된다. timeout 은 anyhow error 로 올라와 turn 에 “tool call failed” 로 보고된다 — turn 자체는 죽지 않는다.

## 8. 폐쇄망 운영 관점

xtech 의 핵심 운영 가정은 “LLM 호출은 사내 게이트웨이로만 나간다” 이지만, MCP 는 사용자가 자유롭게 third-party 서버를 등록할 수 있는 면이라 **별도의 운영 검증** 이 필요하다.

운영자가 점검할 항목:

1. **`mcp_servers` 화이트리스팅.** 사용자별 `~/.xtech/config.toml` 의 `[mcp_servers.*]` 섹션은 임의 URL (`url = "https://..."`) 또는 임의 바이너리 (`command = "..."`) 를 쓸 수 있다. 폐쇄망 정책으로 묶고 싶으면 system / managed layer (precedence 가 더 강한 layer — `arch/06-config.md` 의 layer 표 참고) 에서 server set 을 강제하거나, 기업용 MDM/managed config 로 `mcp_servers` 자체를 빈 테이블로 잠그는 게 합리적.
2. **Streamable HTTP 서버의 egress 도메인.** `McpServerTransportConfig::StreamableHttp { url, … }` 의 origin (`connection_manager.rs::transport_origin`) 이 그대로 outbound HTTP 호출처가 된다. 사내 프록시 / 방화벽에서 이 origin 들을 모두 통과 정책에 등록해야 connect 자체가 된다 — 반대로, **승인되지 않은 origin 은 방화벽에서 막혀 그냥 startup failure 로 떨어진다.**
3. **Stdio 서버의 부수 통신.** stdio MCP 서버는 사용자 머신에서 별개의 프로세스로 돌고, 거기서 또 외부 API 를 부르는 경우가 많다 (예: GitHub MCP → `api.github.com`). codex 입장에서는 자식 프로세스의 outbound 트래픽을 가시화할 방법이 없다 — sandbox 정책 (`McpRuntimeEnvironment`, `codex-linux-sandbox`) 을 통해 자식 프로세스가 어떤 권한으로 도는지를 운영자가 의식해야 한다. `experimental_environment` 를 셋해서 executor 로 우회 spawn 하면 sandbox env 가 더 일관되게 적용된다.
4. **OAuth callback.** OAuth 로그인은 로컬 127.0.0.1 listener 를 띄우므로 outbound 만 정책에 잡으면 충분하지만, `mcp_oauth_callback_url` 을 외부 도메인으로 셋한 경우 redirect 가 외부로 나간다는 점을 기억해야 한다. 보통 그대로 두는 게 맞다.
5. **`codex_apps` 자동 활성.** ChatGPT 인증이 fork 환경에서 켜져 있다면 `apps_enabled` 가 무심코 `true` 가 됐을 때 `https://chatgpt.com/...` 으로 outbound 가 발생할 수 있다. fork 가 사내 gateway 만 쓰는 시나리오에서는 system-layer config 에서 `apps_enabled = false` 로 못박는 게 안전.
6. **`xtech mcp-server` 노출 범위.** 자체적으로는 stdio 만 쓰니 네트워크 노출은 없지만, 이 프로세스가 띄우는 codex 세션이 fork 의 LLM gateway 를 호출한다. 즉 외부 호스트가 mcp-server 에 stdio 로 붙으면 그쪽 호스트가 사실상 사내 LLM 게이트웨이 사용 권한을 위임받는 셈이라 — 호스트 머신 권한 = LLM 사용 권한이라는 단순한 등식이 성립함을 인지하고 워크스테이션 정책을 잡아야 한다.

요약하면, MCP 는 fork 의 “LLM egress 만 통제하면 됨” 가정을 깨는 유일한 경로다. egress 통제 정책에 `mcp_servers.*.url` (streamable HTTP) 과 stdio 서버의 자식 프로세스 두 줄을 반드시 추가해야 폐쇄망 가정이 유지된다.

### 8.1 운영자 점검 체크리스트 (요약)

| 점검 항목 | 위치 | 강제 방법 |
| --- | --- | --- |
| 외부 origin 화이트리스팅 | `[mcp_servers.*]` `url` | system layer 에서 `mcp_servers` 강제 / 방화벽 allow-list |
| stdio 서버 outbound | 자식 프로세스 자체 | OS 방화벽 + `experimental_environment` 로 sandbox 강제 |
| OAuth 토큰 위치 | `mcp_oauth_credentials_store` | system layer 에서 `keyring` 강제 (file fallback 비활성) |
| ChatGPT apps | `apps_enabled` | system layer 에서 `false` 못박기 |
| `xtech mcp-server` 호스트 노출 | stdio 만 사용 | 워크스테이션 권한 정책 = LLM 사용 권한 |
| thread 원격 저장 | `experimental_thread_store_endpoint` | unset (로컬 SQLite) 또는 사내 endpoint 만 |

### 8.2 transport 별 차이점 한 줄 요약

- **stdio**: 코드/바이너리 신뢰가 핵심. 실행 권한 = 신뢰. fork 입장에서 outbound 는 자식 프로세스 책임.
- **streamable HTTP**: URL/credential 신뢰가 핵심. fork 가 직접 reqwest 로 호출하므로 사내 프록시/CA 적용 가능 (`build_reqwest_client_with_custom_ca`). bearer token 은 환경변수만, OAuth 는 keyring 기반.

두 transport 모두 startup 실패는 **fail-soft** 가 기본 (`required = false` 일 때) — codex 세션 자체는 살고, 해당 서버의 도구만 사라진다. `required = true` 로 잡힌 서버가 실패하면 `codex exec` 가 종료된다는 점은 자동화 파이프라인 신뢰성 측면에서 의식해둘 가치가 있다.
