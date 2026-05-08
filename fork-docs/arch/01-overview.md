# 01 — 아키텍처 개요

이 문서는 `xtech` (구 `openai/codex` fork) 코드베이스를 처음 들여다보는 엔지니어를 위한 오리엔테이션이다. 각 영역의 자세한 동작은 같은 디렉토리의 후속 문서들 (`02-turn-lifecycle.md`, `03-wire-protocol.md`, `06-config.md`, `09-tools.md`, `13-tui-structure.md`) 을 참조한다.

## 1. 모노레포 구성

저장소 루트는 polyglot 이지만 사실상 모든 비즈니스 로직은 Rust 다.

- `codex-rs/` — Cargo 워크스페이스. 워크스페이스 정의는 `codex-rs/Cargo.toml:1-111` 의 `[workspace] members` 와 `codex-rs/Cargo.toml:123-231` 의 `[workspace.dependencies]` (path 기반 internal crate 등록).
- `codex-cli/` — `dist/` 에 들어가는 Rust 바이너리를 감싸는 npm 래퍼. `codex-cli/scripts/` 가 패키징을 담당한다.
- `sdk/typescript/`, `sdk/python/` — 외부 노출 SDK. 본 fork 의 작업 스코프와는 거의 무관.
- `docs/` — upstream 사용자 문서 (config / sandbox / exec / agents). fork 한정 결정은 본 `fork-docs/` 에 둔다 (`fork-docs/README.md:1`).
- `tools/argument-comment-lint/` — 위치 인자 literal 호출에 `/*param*/` 코멘트를 강제하는 Bazel 기반 Dylint.
- `justfile` — 루트에 있지만 `set working-directory := "codex-rs"` (`justfile:1`) 라 모든 레시피는 codex-rs 안에서 실행된다.

워크스페이스에 등록된 멤버는 100여 개 (정확히 `codex-rs/Cargo.toml:1-111` 기준 약 110개 path entry). 큰 그림으로는 다음 그룹으로 나뉜다.

### 1.1 Core 비즈니스 로직 그룹
- `core` (`codex-core`, `codex-rs/core/`) — 에이전트 / 턴 루프 / 도구 디스패치 / SSE 파싱이 모두 여기에 있다. 가장 큰 크레이트이고 “손대지 말 것” 1순위 (CLAUDE.md / AGENTS.md 모두 명시). `codex-rs/core/src/client.rs:1` 가 LLM HTTP 클라이언트 진입점, `codex-rs/core/src/codex_thread.rs` 가 thread/turn 상태기. `codex-rs/core/src/` 에는 128 개 파일 (apply_patch / compact / context_manager / event_mapping / exec / guardian / personality_migration / hook_runtime 등) 이 깔려 있어 “core 가 곧 product” 라 봐도 무방하다.
- `core-api`, `core-plugins`, `core-skills` — core 가 외부에 노출하는 trait / 플러그인 / 스킬 등록 layer. core 본체에서 떼어낸 가장 최근의 분리 산물.
- `protocol` (`codex-protocol`) — `ResponseItem` / `ResponseEvent` / role enum 등 wire-agnostic 타입 정의. 여러 크레이트가 공유. fork 가 손댄 `WireApi::Chat` 도 wire enum 차원에서는 인접 크레이트인 `codex-model-provider-info` 에 산다.

### 1.2 진입점 / 실행 그룹
- `cli` (`codex-cli`, 크레이트명 `codex-cli`, 바이너리명 `codex`) — 멀티툴. `codex exec` / `codex tui` / `codex mcp-server` 등 모든 서브커맨드 디스패치. `codex-rs/cli/src/main.rs`, deps 는 `codex-rs/cli/Cargo.toml:23-53`.
- `exec` (`codex-exec`) — headless 비대화형 드라이버. `codex exec` 가 곧 이 크레이트.
- `tui` (`codex-tui`) — Ratatui 기반 풀스크린 대화형 UI. `chatwidget.rs` 가 11k LoC 이상으로 hot 영역.

### 1.3 API 클라이언트 / wire 그룹
- `codex-api` (`codex-rs/codex-api/`) — Responses API + (이 fork 에서 부활시킨) Chat Completions API 클라이언트. `endpoint/`, `requests/`, `sse/` 서브모듈로 wire 별 분기.
- `model-provider-info`, `model-provider`, `models-manager` — provider 메타데이터, 빌트인 OSS provider (ollama / lmstudio) 정의 (`codex-rs/model-provider-info/src/lib.rs:415-440`).
- `ollama`, `lmstudio` — OSS 게이트웨이 readiness probe.
- `chatgpt`, `backend-client`, `responses-api-proxy` — 다양한 OpenAI 측 통신 보조.

### 1.4 프로토콜 / IPC 그룹
- `app-server` + `app-server-protocol` + `app-server-client` + `app-server-transport` — IDE / 데스크탑 클라이언트와 codex 사이의 JSON-RPC. **신규 RPC 는 v2 (`app-server-protocol/src/protocol/v2.rs`) 에만 추가** 한다 (CLAUDE.md 규약). camelCase wire format, `*Params` / `*Response` / `*Notification` 네이밍, cursor 페이지네이션 — 자세한 룰은 CLAUDE.md “App-server v2 API conventions” 절에 정리되어 있다. 자세한 wire 동작은 `03-wire-protocol.md`.
- `codex-mcp`, `mcp-server` — MCP 클라이언트 매니저 (`codex-rs/codex-mcp/src/connection_manager.rs`) 및 실험적 MCP 서버. tool mutation 은 반드시 connection_manager 를 통해야 한다.
- `exec-server`, `stdio-to-uds`, `uds` — 외부 프로세스 / IPC 트랜스포트.

### 1.5 도구 그룹
- `tools` (`codex-tools`) — 모델에 노출되는 tool spec / dispatcher. `codex-rs/tools/src/tool_spec.rs:174` 의 `create_tools_json_for_chat_completions_api` 가 이 fork 가 다시 살린 변환기. 개별 도구 모듈은 `codex-rs/tools/src/` 아래 `apply_patch_tool.rs`, `agent_tool.rs`, `code_mode.rs`, `dynamic_tool.rs` 등으로 한 도구 = 한 파일 패턴.
- `apply-patch`, `shell-command`, `shell-escalation`, `code-mode`, `file-search`, `file-system`, `skills`, `connectors`, `hooks` — 개별 도구 / 부수 기능. 자세한 등록 흐름과 도구별 책임은 `09-tools.md`.

### 1.6 샌드박스 그룹
- `linux-sandbox` — Landlock + seccomp 기반. `seccompiler` 의존이 있고 (`codex-rs/Cargo.toml:344`) `landlock` 도 직접 의존 (`Cargo.toml:298`).
- `sandboxing` — macOS Seatbelt (`/usr/bin/sandbox-exec`) 래퍼.
- `windows-sandbox-rs` (`codex-windows-sandbox`) — Windows job-object 기반.
- `bwrap` — Linux 배포용 bubblewrap 번들링 (`22326e263c` 커밋이 DotSlash artifact 에 bwrap 을 묶었다, recent commits 참고).
- `process-hardening`, `network-proxy`, `secrets`, `keyring-store` — 인접 보안 컴포넌트.

샌드박스 코드를 만질 때는 `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` / `CODEX_SANDBOX_ENV_VAR` 를 신규 코드에서 참조하지 말 것 (CLAUDE.md). 기존 참조는 모두 “코덱스가 자기 자신을 샌드박스 안에서 실행시킬 때의 회피 가드” 라 의도가 깨지면 CI 가 침묵한다.

### 1.7 통합 / 인프라 그룹
- `analytics`, `otel`, `feedback`, `rollout`, `rollout-trace`, `state` — 텔레메트리 / 세션 영속화. `state` 는 SQLite 기반이라 Bazel 빌드를 위해서는 `sqlx::migrate!` 가 가리키는 SQL 파일을 BUILD 의 `compile_data` 로 노출해야 한다 (CLAUDE.md Bazel notes).
- `cloud-tasks`, `cloud-tasks-client`, `cloud-requirements`, `external-agent-sessions`, `external-agent-migration` — 원격/클라우드 에이전트 인프라.
- `login`, `aws-auth`, `device-key`, `agent-identity` — 인증. fork 환경에서는 `OLLAMA_API_KEY` 만 쓰므로 이 그룹의 코드는 거의 dead path 다.

### 1.8 유틸 그룹
`utils/*` 아래 20여 개 마이크로 크레이트 (`absolute-path`, `cargo-bin`, `cache`, `home-dir`, `json-to-toml`, `pty`, `string`, `template`, `oss`, …). 단일 책임을 가진 `pub fn` 한 줌짜리가 대부분이라 신규 코드를 `codex-core` 에 넣고 싶을 때 먼저 봐야 할 곳이 여기다 (`codex-utils-oss` 가 fork 의 `apply_fork_config_to_env` 를 re-export 하는 호스트).

## 2. 의존 위계

큰 흐름은 다음과 같다 (`->` 는 “import 한다”).

```
codex (cli bin) -> codex-cli -> codex-exec, codex-tui, codex-mcp-server, codex-app-server, codex-core, ...
codex-exec      -> codex-core, codex-app-server-client, codex-app-server-protocol, codex-protocol, codex-otel
codex-tui       -> codex-app-server-client, codex-app-server-protocol, codex-protocol  (※ codex-core 직접 의존 없음)
codex-mcp-server -> codex-core
codex-app-server -> codex-core, codex-core-plugins, codex-protocol, codex-tools
codex-core      -> codex-api, codex-tools, codex-protocol, codex-config, codex-mcp, codex-rollout, codex-hooks,
                   codex-model-provider-info, codex-sandboxing, codex-state, ...
codex-api       -> codex-protocol, codex-model-provider-info, codex-login (HTTP layer only)
codex-protocol  -> (low-level types; few internal deps)
```

검증 근거:
- `codex-rs/cli/Cargo.toml:23-53` — cli 가 exec / tui / mcp-server / app-server / core / protocol 등을 모두 가진 멀티툴 (단일 `codex` 바이너리에 전부 link 된다, `codex-rs/cli/Cargo.toml:8-10`).
- `codex-rs/exec/Cargo.toml:23-40` — exec 는 `codex-core` 와 `codex-app-server-client` 를 둘 다 의존. 직접 core 를 호출하는 경로와 app-server 를 통해 우회하는 경로가 둘 다 살아있다.
- `codex-rs/tui/Cargo.toml:23-66` — tui 는 `codex-app-server-client` / `codex-app-server-protocol` / `codex-protocol` 만 들고 `codex-core` 를 직접 import 하지 않는다. TUI ↔ core 통신은 in-process app-server 를 통해 이뤄진다 (`scripts/run_tui_with_exec_server.sh`, `justfile:20-22`). 자세한 RPC 매핑은 `03-wire-protocol.md`.
- `codex-rs/core/Cargo.toml:30-65` — core 는 codex-api / codex-tools / codex-protocol / codex-config / codex-mcp / codex-rollout / codex-hooks / codex-model-provider-info / codex-sandboxing / codex-state 를 import. 의존이 가장 두꺼운 노드 (약 35 개의 internal crate).
- `codex-rs/app-server/Cargo.toml` — app-server 는 core 를 import 하므로 protocol/v2 에 새 RPC 를 추가하면 그 핸들러를 보통 app-server 안에 두고 거기서 core API 를 호출하는 모양이 된다.

따라서 **TUI 는 core 의 wire 변경을 직접 보지 않고 app-server v2 를 통해서만 본다**. fork 의 wire 변경 (`WireApi::Chat` 부활) 은 core / codex-api / model-provider-info / tools 에 국한되었고 TUI 는 무관하다 (`fork-docs/work-log-2026-05-08.md:14-23`). 반대로, app-server v2 RPC 모양을 바꾸면 TUI 와 외부 IDE 클라이언트가 함께 깨진다 — 그래서 v2 만 확장하라는 규약이 강제된다.

## 3. 손대면 위험한 곳 vs 비교적 안전한 곳

### 3.1 위험 (변경 시 리뷰 강도 상승, 충돌 위험)

- `codex-rs/core/` 전체 — “Resist adding to this crate” (CLAUDE.md). 새 기능은 신규 크레이트 또는 기존 작은 크레이트로 빼야 한다.
- `codex-rs/core/src/client.rs` (2269 LoC) — wire 분기 / 인증 / SSE 파이프. 이 fork 도 여기서 `WireApi::Chat` 분기를 추가했다 (`codex-rs/core/src/client.rs:1507-1556`).
- `codex-rs/core/src/codex_thread.rs` — 턴 상태기. 자세한 흐름은 `02-turn-lifecycle.md` 참조.
- `codex-rs/core/src/config/mod.rs` — `ConfigToml` / 디폴트. 변경 시 `just write-config-schema` 로 `core/config.schema.json` 재생성 필수. fork 의 디폴트 provider 변경도 여기서 (`config/mod.rs:2634`).
- `codex-rs/tui/src/chatwidget.rs` (11163 LoC), `codex-rs/tui/src/bottom_pane/chat_composer.rs` (10456 LoC), `codex-rs/tui/src/bottom_pane/mod.rs` (2806 LoC), `codex-rs/tui/src/bottom_pane/footer.rs` (2017 LoC), `codex-rs/tui/src/app.rs` (1171 LoC) — 모두 AGENTS.md 가 명시적으로 “신규 메서드를 여기에 더하지 말라” 고 못박은 hot 파일. 새 모듈을 만들고 거기에 추가하는 게 원칙. 모듈 사이즈 가이드: <500 LoC 목표, ~800 LoC 가 분리 압력선.
- `codex-rs/codex-mcp/src/connection_manager.rs` — MCP tool mutation 은 반드시 이 매니저를 통해야 한다.
- `codex-rs/app-server-protocol/src/protocol/v1.rs` — **수정 금지 / 추가 금지**. 신규 RPC 는 모두 `protocol/v2.rs`.
- `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR`, `CODEX_SANDBOX_ENV_VAR` — 어떤 형태로든 신규 코드에서 참조 금지. 기존 참조는 샌드박스 내 테스트 회피용 가드라 의도가 깨지면 CI 가 침묵으로 잘못된다.
- 이 fork 추가분: `codex-rs/codex-api/src/{endpoint,requests,sse}/chat.rs`, `codex-rs/tools/src/tool_spec.rs:174-200` — upstream 이 의도적으로 제거한 코드 경로를 되살린 부분이라 rebase 충돌이 큰 영역 (`fork-docs/work-log-2026-05-08.md:121-126`).

### 3.2 비교적 안전 (개입 비용 낮음)

- `codex-rs/utils/*` — 마이크로 유틸. 신규 utility 는 여기에 새 크레이트로 떨구면 core 부풀리기를 피할 수 있다. 새 크레이트를 워크스페이스에 추가할 때는 `codex-rs/Cargo.toml:1-111` 의 `members` 와 `[workspace.dependencies]` (`codex-rs/Cargo.toml:123-231`) 양쪽에 등록한다.
- `codex-rs/ollama/`, `codex-rs/lmstudio/` — readiness probe 정도라 표면적이 좁다. 이 fork 의 변경도 대부분 여기 (`ollama/src/lib.rs`, `ollama/src/client.rs`, `ollama/src/fork_config.rs`).
- `codex-rs/model-provider-info/` — 데이터 구조 + 빌트인 카탈로그. 변경 영향이 명확. `WireApi` enum (`codex-rs/model-provider-info/src/lib.rs:47-82`) 도 여기 산다.
- `codex-rs/exec-server/`, `codex-rs/stdio-to-uds/` — 트랜스포트 어댑터.
- `codex-rs/docs/` 안 사용자 문서 — 동작 변경에 동반 갱신.
- 단발성 도구 추가 (`apply-patch`, `file-search`, `connectors`, `shell-command`) — tool registry 에 등록만 하면 core 본체 변경 없이 끼워넣을 수 있다. 자세한 패턴은 `09-tools.md`.

## 4. 빌드 시스템

Cargo 와 Bazel 이 공존한다. 일상 개발은 Cargo, CI / RBE / 릴리즈는 Bazel. 둘은 같은 `*.rs` 소스를 보지만 의존 그래프 / 데이터 파일은 별도로 선언한다.

`justfile` 의 핵심 레시피 (`justfile:11-119`):

| 목적 | 명령 |
| --- | --- |
| 소스로 codex 실행 | `just codex [args]` (`justfile:12-13`, alias `just c`) |
| headless 실행 | `just exec [args]` (`justfile:16-17`) |
| TUI + exec-server | `just tui-with-exec-server` (`justfile:20-22`) |
| 포맷 | `just fmt` (`justfile:34-35`) |
| Clippy autofix | `just fix [-p <crate>]` (`justfile:37-38`) |
| 워크스페이스 테스트 | `just test` (cargo nextest, `justfile:53-54`) |
| MCP 서버 | `just mcp-server-run` (`justfile:89-90`) |
| config schema 재생성 | `just write-config-schema` (`justfile:93-94`) |
| Bazel 빌드 | `just bazel-codex` (`justfile:60-61`) |
| Bazel 락 갱신 | `just bazel-lock-update` (`justfile:64-65`) |
| 인자 코멘트 lint | `just argument-comment-lint` (`justfile:106-111`) |

빌드 산출물 경로:
- 디버그 / 릴리즈 바이너리: `codex-rs/target/{debug,release}/codex` (`codex-rs/target/`). 디버그 빌드는 `[profile.dev]` (`codex-rs/Cargo.toml:475-479`) 가 `debug = 1` 로 잡혀 있어 백트레이스는 살아있되 풀 디버그 정보보다는 가볍다.
- 릴리즈는 `[profile.release]` (`codex-rs/Cargo.toml:486-494`) 에서 `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"` — npm 번들 사이즈 최소화 목적.
- 크로스 / 유니버설: `codex-rs/target/aarch64-apple-darwin/...`, `dist/xtech-*-arm64.tar.gz`, `dist/xtech-*-universal.pkg` (`dist/` 디렉토리 listing 참고). pkg 빌드 스크립트는 `fork-docs/scripts/build-macos-pkg.sh`.
- 패치 채널: `codex-rs/Cargo.toml:501-513` 의 `[patch.crates-io]` 가 `crossterm` / `ratatui` / `tokio-tungstenite` / `tungstenite` 를 fork 된 git revision 으로 고정. 의존성 변경 시 `MODULE.bazel.lock` 를 같이 커밋해야 Bazel 이 깨지지 않는다 (AGENTS.md `bazel-lock-update` 항목).

Bazel 만의 함정: `include_str!` / `include_bytes!` / `sqlx::migrate!` 추가 시 해당 크레이트의 `BUILD.bazel` 에 `compile_data` / `build_script_data` 를 명시해야 한다. Cargo 만으로는 잡히지 않는다 (CLAUDE.md “Bazel notes”). 또한 워크스페이스 clippy 룰셋 (`codex-rs/Cargo.toml:425-460`) 이 `unwrap_used` / `expect_used` / `redundant_clone` / `await_holding_lock` / 다수 `manual_*` lint 를 deny 로 박아두므로, fork 패치를 만들 때도 이 룰을 회피하지 말 것 — `just fix -p <crate>` 로 보통 자동 수정된다.

## 5. fork 가 건드리는 영역

이 fork 는 LLM 호출을 사내 nginx 게이트웨이 (Ollama 호환, `qwen3.5-122b`) 로 디폴트 라우팅하기 위해 다음 영역에만 손을 댔다. 표면적이 좁아 후속 upstream rebase 비용을 통제할 수 있도록 설계되었다.

- Provider 라우팅 / 디폴트: `codex-rs/core/src/config/mod.rs:2634` (디폴트 provider id), `codex-rs/model-provider-info/src/lib.rs:415-425` (빌트인 ollama provider 의 wire / env_key).
- `WireApi::Chat` 부활: `codex-rs/model-provider-info/src/lib.rs:47-82` (enum), `codex-rs/core/src/client.rs:1507-1556` (분기 + 디스패치), `codex-rs/codex-api/src/{endpoint,requests,sse}/chat.rs` (HTTP 클라이언트), `codex-rs/tools/src/tool_spec.rs:174-200` (Responses → Chat tool JSON 변환).
- Readiness gating (—oss 없이도 동작): `codex-rs/exec/src/lib.rs:577-585`, `codex-rs/tui/src/lib.rs:1010-1015`, `codex-rs/ollama/src/{lib,client}.rs`.
- Role 매핑 (`developer → user`, 게이트웨이 정책 회피): `codex-rs/codex-api/src/requests/chat.rs:198`.
- 외부 JSON config 주입 (`~/.codex/codex-fork.json` / `$CODEX_FORK_CONFIG`, opencode 호환 키 표기): `codex-rs/ollama/src/fork_config.rs`, `codex-rs/utils/oss/src/lib.rs:7-8`, `codex-rs/cli/src/main.rs:745-749`.
- 모델 카탈로그 청소 (`qwen3.5-122b` 단일 항목): `codex-rs/models-manager/models.json`.
- 부수 갱신: `codex-rs/config/src/thread_config/remote.rs:288`, `codex-rs/core/src/personality_migration*.rs`, `codex-rs/model-provider-info/src/model_provider_info_tests.rs`.

전체 변경 일지와 디버깅 함정은 `fork-docs/work-log-2026-05-08.md`. 설계 문서는 `fork-docs/ollama-migration.md`. 운영 환경 airgap 점검은 `fork-docs/airgap-audit-2026-05-08.md`. multi-turn / 영속화 관련은 `fork-docs/multi-turn-and-storage-2026-05-08.md`. 본 fork-docs/arch 시리즈의 색인은 `fork-docs/arch/README.md` (예정).

알려진 잔존 항목 (각각 work-log 4 절 참조):
- `codex-rs/core/tests/chat_completions_payload.rs`, `codex-rs/core/tests/chat_completions_sse.rs` — `#![cfg(any())]` 로 컴파일 제외 상태. upstream protocol drift 정리 후 재활성화 필요.
- `codex-rs/tui/src/chatwidget.rs:459` — `NUDGE_MODEL_SLUG = "gpt-5.4-mini"` 가 dead path. 동작은 깨지지 않지만 silent no-op.
- `WireApi::Responses` 분기 — fork 운영 환경에서 미사용이라 회귀 테스트가 빈약. lmstudio provider 가 형식상 살아있는 정도.

## 6. 테스트 / 스냅샷 관행 (요점)

- workspace 차원에서 `cargo nextest` 가 표준이다 (`justfile:53-54`). 워크스페이스 전체 실행 (`just test`) 은 슬로우.
- 단일 크레이트는 `cargo test -p codex-<crate>` (예: `cargo test -p codex-tui`).
- TUI 는 `insta` 스냅샷이 의무. UI 영향이 있는 PR 은 `.snap` / `.snap.new` 를 동반해야 리뷰어가 시각적 영향을 본다 (CLAUDE.md “Snapshot tests”).
- core 통합 테스트는 `core_test_support::responses` 의 `mount_sse_once` / `ResponseMock` 헬퍼를 사용. 환경변수를 mutate 하지 않고 의존을 주입하는 게 컨벤션 (AGENTS.md Tests 절).
- 바이너리 호출은 `codex_utils_cargo_bin::cargo_bin(...)` 으로. `assert_cmd::Command::cargo_bin` / `escargot` 은 Bazel runfiles 에서 깨진다.

이 fork 가 비활성화한 chat completions 테스트 (`codex-rs/core/tests/chat_completions_payload.rs:7`, `codex-rs/core/tests/chat_completions_sse.rs:4`) 는 `#![cfg(any())]` 로 컴파일에서 제외되어 있어 `just test` 가 통과해도 실제 wire 회귀를 잡지 못한다. end-to-end 검증은 work-log 3 절의 수동 시나리오에 의존한다.

## 7. 한 턴 (turn) 의 단순 데이터 흐름

세부 흐름은 `02-turn-lifecycle.md` 가 다루지만, 오리엔테이션용 한 줄 요약:

1. 사용자 입력은 `codex-tui` (대화형) 또는 `codex-exec` (headless) 가 받는다.
2. TUI 의 경우 `codex-app-server-client` 를 통해 in-process app-server 의 v2 RPC (보통 `thread/send` 류) 로 직렬화된다. exec 는 같은 RPC 를 쓰거나 직접 core API 를 호출한다.
3. `codex-core` 의 thread 가 도구 / 컨텍스트 / 메모리를 정리해 `codex-api` 에 prompt 를 넘긴다.
4. `codex-api` 가 `codex-rs/model-provider-info/src/lib.rs:47-82` 의 `WireApi` 분기에 따라 Responses 또는 Chat Completions HTTP 호출. 이 fork 는 ollama 디폴트라 `WireApi::Chat` 분기로 떨어져 `codex-rs/codex-api/src/sse/chat.rs` 가 SSE 를 파싱한다.
5. SSE 이벤트는 `codex-protocol` 의 `ResponseEvent` 로 정규화되어 다시 core → app-server → TUI/exec 로 역류한다. TUI 는 `chatwidget.rs` 가 이 이벤트를 받아 cell 을 늘리며 렌더한다.

이 5 단계 중 fork 가 손댄 곳은 (3)–(4) 사이의 wire 분기 / 인증 / role 매핑 / readiness 만이다. (1)–(2) 와 (5) 는 upstream 그대로 흘러간다.

## 8. 작업별 “어디부터 봐야 하나”

| 만지려는 것 | 1차 진입점 |
| --- | --- |
| LLM 응답 파싱 / SSE | `codex-rs/codex-api/src/sse/{chat,responses}.rs`, `codex-rs/core/src/client.rs:1507-1570` |
| 디폴트 provider / 모델 슬러그 | `codex-rs/core/src/config/mod.rs:2634`, `codex-rs/models-manager/models.json` |
| Provider 정의 (wire / env / base url) | `codex-rs/model-provider-info/src/lib.rs:415-440` (빌트인), `codex-rs/model-provider-info/src/lib.rs:483-502` (helper) |
| Tool 추가 / 변경 | `codex-rs/tools/src/lib.rs`, `codex-rs/tools/src/<tool>.rs`. Chat Completions 호환 변환은 `codex-rs/tools/src/tool_spec.rs:174-200`. 자세한 흐름은 `09-tools.md` |
| MCP 통합 | `codex-rs/codex-mcp/src/connection_manager.rs` 만 — wrapper 추가 금지 |
| 새 RPC | `codex-rs/app-server-protocol/src/protocol/v2.rs` + `codex-rs/app-server/src/` 핸들러 |
| TUI 입력 / 렌더 | `codex-rs/tui/src/chatwidget/` (서브모듈 분리됨), `codex-rs/tui/src/bottom_pane/`. `chatwidget.rs` 본체는 가급적 신규 메서드 추가 금지. 자세한 구조는 `13-tui-structure.md` |
| Config 스키마 | `codex-rs/config/src/`, `codex-rs/core/src/config/` + `just write-config-schema`. `06-config.md` 참조 |
| 샌드박스 | `codex-rs/linux-sandbox/`, `codex-rs/sandboxing/`, `codex-rs/windows-sandbox-rs/` |
| Fork 한정 env / JSON 주입 | `codex-rs/ollama/src/fork_config.rs`, `codex-rs/cli/src/main.rs:745-749` |

## 마무리

이 fork 의 표면은 좁다. 위험 영역 (`codex-core/src/client.rs`, `codex-core/src/config/mod.rs`, `tui/src/chatwidget.rs`) 을 건드리지 않으면 upstream rebase 비용이 거의 들지 않는다. 새 기능을 넣을 때는 (1) 기존 크레이트 중 적절한 곳, (2) 신규 utils 크레이트, (3) 마지막 수단으로 core — 순서로 검토하면 모노레포 관리 비용이 가장 낮다. core 에 코드를 추가하기 전에 “이게 정말 core 의 책임인가, 아니면 model-provider-info / tools / config 중 어디로 가야 하는가” 를 한 번 더 묻는 습관이 가장 큰 차이를 만든다.
