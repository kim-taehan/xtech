# xtech 아키텍처 분석 문서 — 인덱스

`xtech` (forked from `openai/codex`) 의 코드베이스를 19개 슬라이스로 나눠 정리한 시리즈. 각 문서는 ~200-310 줄, file:line 인용 포함, 한국어 산문 + 영어 식별자.

처음 fork 분석 시 권장 순서: **01 → 02 → 03 → 06 → 09**. 이 5개로 큰 그림이 잡힌 뒤, 손볼 영역에 맞춰 상세 문서로 들어가시면 됩니다.

---

## 권장 입문 코스 (5개)

| 순서 | 문서 | 한 줄 요약 |
|---|---|---|
| 1 | [01-overview.md](01-overview.md) | 100여 개 크레이트의 그룹 분류 / 의존 위계 / hot vs stable 영역 |
| 2 | [02-turn-lifecycle.md](02-turn-lifecycle.md) | 사용자 입력 → 모델 호출 → tool 실행 → 응답까지 데이터/타입 흐름 |
| 3 | [03-wire-protocol.md](03-wire-protocol.md) | Responses vs Chat Completions 두 wire 분기, fork 가 살린 chat path |
| 4 | [06-config.md](06-config.md) | `~/.xtech/config.toml` + `~/.config/xtech/xtech.json` + env vars 합성 순서 |
| 5 | [09-tools.md](09-tools.md) | shell / apply_patch / search / MCP — 빌트인 도구 등록과 dispatch |

---

## A. 코어 실행 흐름

| 문서 | 다루는 것 |
|---|---|
| [02-turn-lifecycle.md](02-turn-lifecycle.md) | submission_loop → run_turn → try_run_sampling_request 의 5개 핵심 file:line. `WireApi::Chat` vs `Responses` 분기점 |
| [03-wire-protocol.md](03-wire-protocol.md) | Responses path / Chat path 빌더, `developer → user` 매핑, tool spec wrap, 인증 헤더 체인 |
| [04-streaming.md](04-streaming.md) | SSE 4단계 파이프라인, `process_responses_event` vs `process_chat_sse`, tool_call delta 재조합, `stream_idle_timeout` |
| [05-error-handling.md](05-error-handling.md) | 3-layer 에러 매핑, `request_max_retries` vs `stream_max_retries`, rate-limit 처리, FunctionCallError 분기 |

## B. 저장 / 상태

| 문서 | 다루는 것 |
|---|---|
| [07-thread-manager.md](07-thread-manager.md) | thread vs session vs conversation 용어, ThreadManager 라이프사이클, fork/resume |
| [08-models-manager.md](08-models-manager.md) | 모델 카탈로그 로딩, slug 매칭, ETag/cache TTL, fork 의 `qwen3.5-122b` 단일 슬러그 정책 |
| (cross-ref) [../multi-turn-and-storage-2026-05-08.md](../multi-turn-and-storage-2026-05-08.md) | 디스크 저장 측면 — JSONL rollout, state SQLite, shell snapshots |

## C. 능력 (도구 / 스킬 / 플러그인 / MCP)

| 문서 | 다루는 것 |
|---|---|
| [09-tools.md](09-tools.md) | 빌트인 13종 + ToolRegistry/ToolRouter dispatch + apply_patch Freeform/Function 모드 |
| [10-skills.md](10-skills.md) | prompt-skill 로딩 위계, user/developer role 주입 분리, 충돌 해결 |
| [11-plugins.md](11-plugins.md) | Plugin manifest, curated repo 3-tier sync, 폐쇄망 차단점 (`features/src/lib.rs:942`) |
| [12-mcp.md](12-mcp.md) | client (외부 MCP server 연결) vs server (`xtech mcp-server`), OAuth, rmcp-client 책임 분담 |

## D. UI

| 문서 | 다루는 것 |
|---|---|
| [13-tui-structure.md](13-tui-structure.md) | 모듈 맵, hot 파일 LoC, 렌더 트리, 스타일 컨벤션, snapshot 테스트 |
| [14-tui-event-loop.md](14-tui-event-loop.md) | 4-way `tokio::select!` 메인 루프, Crossterm 매핑, app-server 양방향, FrameRequester 효율 |

## E. 운영 / 설정

| 문서 | 다루는 것 |
|---|---|
| [06-config.md](06-config.md) | 합성 순서, `CODEX_HOME`, `apply_fork_config_to_env`, env var 우선순위표 |
| [15-build-deploy.md](15-build-deploy.md) | Cargo + Bazel, justfile 레시피, release profile, universal binary, .pkg 패키징, GitHub release |

## F. 보안 / 샌드박스

| 문서 | 다루는 것 |
|---|---|
| [16-sandboxing.md](16-sandboxing.md) | SandboxMode 3종, Seatbelt/Landlock/bwrap, `CODEX_SANDBOX_*` env, shell-escalation SCM_RIGHTS |
| [17-approval-guardian.md](17-approval-guardian.md) | AskForApproval enum, GuardianAssessment, circuit breaker 3/10, codex-auto-review 모델 lookup |
| (cross-ref) [../airgap-audit-2026-05-08.md](../airgap-audit-2026-05-08.md) | 폐쇄망에서 phone-home 하는 모든 지점과 차단 방법 |

## G. 통합

| 문서 | 다루는 것 |
|---|---|
| [18-app-server.md](18-app-server.md) | JSON-RPC v1 frozen / v2 active, naming 컨벤션, transport 4종 (stdio/uds/ws/off) |
| [19-cloud-tasks.md](19-cloud-tasks.md) | OpenAI 호스팅 원격 task 큐 client. fork 의 ollama 인증으로는 fail-fast — 폐쇄망 자연 차단 |

## H. 전체 / fork

| 문서 | 다루는 것 |
|---|---|
| [01-overview.md](01-overview.md) | 모노레포 8 그룹 분류, 의존 위계, hot 영역, 빌드 시스템 |
| (cross-ref) [../work-log-2026-05-08.md](../work-log-2026-05-08.md) | upstream 대비 fork 가 건드린 부분 (chat-completions 복원, models.json 정리, fork_config 등) |
| (cross-ref) [../ollama-migration.md](../ollama-migration.md) | fork 의 원래 설계 문서 (canonical) |

---

## 함께 보면 좋은 외부 문서

| 문서 | 위치 | 다루는 것 |
|---|---|---|
| `AGENTS.md` | repo root | 본격적 contributor 룰 (canonical) |
| `CLAUDE.md` | repo root | AGENTS.md 의 highlight + Claude Code 사용 가이드 |
| `tui/styles.md` | `codex-rs/tui/styles.md` | TUI 스타일 컨벤션 |
| `config.md` | `codex-rs/config.md`, `docs/config.md` | 사용자용 config 레퍼런스 |

---

## 알려진 후속 작업 / 분석 결과 메모

- **TUI snapshot baseline 미동기화** — fork 의 4건 브랜딩 변경 (`OpenAI Codex` → `xTech code`) 후 `insta` snapshot 이 옛 값 그대로라 `cargo test -p codex-tui` 가 실패할 가능성. `cargo insta accept -p codex-tui` 로 갱신 필요. (출처: [13-tui-structure.md](13-tui-structure.md) §9)
- **chat completions 통합 테스트 비활성** — `core/tests/chat_completions_{payload,sse}.rs` 가 `#![cfg(any())]` 로 컴파일에서 빠짐. 우선 비활성, 나중에 case 단위로 살려야 함. (출처: [03-wire-protocol.md](03-wire-protocol.md) 함정 §)
- **NUDGE_MODEL_SLUG silent no-op** — `tui/src/chatwidget.rs:459` 의 `gpt-5.4-mini` 가 카탈로그에서 사라져 dead path. 동작 영향 없으나 코드 흔적은 남음. (출처: [13-tui-structure.md](13-tui-structure.md), [08-models-manager.md](08-models-manager.md))
- **폐쇄망 P0 3건 미해결** — Statsig OTLP 메트릭 / 큐레이팅 plugin git+REST sync / featured plugin REST sync. 상세는 [../airgap-audit-2026-05-08.md](../airgap-audit-2026-05-08.md). 정식 폐쇄망 배포 직전 패치 필요.
- **Cloud Tasks 정리** — fork 디폴트 인증으로는 즉시 fail-fast 라 위험은 작지만, 폐쇄망 정식 배포 전엔 `#[clap(hide = true)]` 또는 `#[cfg(feature = "cloud-tasks")]` 로 숨기는 게 깔끔. (출처: [19-cloud-tasks.md](19-cloud-tasks.md))
- **Approval Guardian 의 추가 LLM hop** — Guardian 활성 시 critical path 에 코드-리뷰 모델 호출이 추가됨. 비용/지연 증가. (출처: [17-approval-guardian.md](17-approval-guardian.md))

---

## 작성 메타데이터

- 작성일: 2026-05-09
- 분석 기준 커밋: `0942154be3` (post-rename / scrub)
- 작성 방식: 19개 슬라이스를 백그라운드 리서치 에이전트로 병렬 작성, 본 README 가 cross-link.
- 향후: 새 영역에 손댈 때마다 해당 슬라이스 문서를 갱신. 신규 슬라이스가 필요하면 같은 패턴 (`<NN>-<topic>.md` + README 행 추가) 으로 확장.
