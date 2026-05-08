# Codex Fork — 멀티턴 대화 및 상태 저장 구조 (2026-05-08)

> 본 문서는 폐쇄망에 배포되는 fork(`/Users/a08368/vscodeProjects/codex-fork/`) 운영자가 "어디에 무엇이 쌓이는가, 무엇이 프로세스 종료 후에도 살아남는가, 어떤 환경변수로 위치가 바뀌는가, 어떻게 백업/삭제하는가" 를 즉시 답할 수 있도록 작성되었다. 모든 경로는 macOS/Linux 기준이며 `~/.codex` 는 `CODEX_HOME` 미설정 시 기본값이다 (`codex-rs/utils/home-dir/src/lib.rs:13-63`).

---

## 1. 요약

한 번의 turn 은 사용자 입력 → `ContextManager` (in-memory 누적) → `ChatRequestBuilder` (chat completions wire 변환, `developer→user` 매핑 포함) → SSE 스트림 응답 수신 → 응답 ResponseItem 들을 다시 `ContextManager` 와 rollout JSONL 로 동시 기록 흐름이다. 멀티턴은 동일 `Session`/`CodexThread` 가 살아있는 동안 `ContextManager.items` 가 한 vector 로 누적되며, 매 turn 종료 시 같은 `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl` 파일에 append-only 로 flush 된다 (`codex-rs/rollout/src/recorder.rs:1371-1415`). 프로세스가 종료되어도 (a) 이 JSONL 파일과 (b) `~/.codex/state_5.sqlite` 메타데이터가 남아 있어, `codex resume <id>` 또는 `codex resume --last` 로 다음 invocation 에서 동일 thread_id 로 이어서 쓸 수 있다 (`codex-rs/cli/src/main.rs:274-298`, `codex-rs/rollout/src/recorder.rs:931-946`). 외부로 나가는 "session sync"/"thread sync" 기능은 현재 fork 빌드에서 기본 비활성: `experimental_thread_store_endpoint`/`experimental_thread_store=remote` 가 설정된 경우에만 gRPC `RemoteThreadStore` 로 빠지며, 기본은 `LocalThreadStore` (`codex-rs/core/src/thread_manager.rs:276-285`, `codex-rs/thread-store/src/remote/mod.rs:34-52`).

---

## 2. In-memory 구조 (한 프로세스 동안만 유지)

### 2.1 `ThreadManager` — thread/session 라이프사이클

- 위치: `codex-rs/core/src/thread_manager.rs:213-259`
- 한 프로세스에 1개. `threads: Arc<RwLock<HashMap<ThreadId, Arc<CodexThread>>>>` 에 살아있는 thread 들을 보관.
- 생성 경로:
  - `start_thread_*` → `InitialHistory::New` 로 새 thread (`thread_manager.rs:578-641`)
  - `resume_thread_from_rollout` → JSONL 파일을 다시 읽어 `InitialHistory::Resumed` 로 복원 (`thread_manager.rs:642-687`)
  - `fork_thread*` → 기존 thread 의 prefix 를 잘라 새 thread_id 부여 (`thread_manager.rs:805-876`, `ForkSnapshot::TruncateBeforeNthUserMessage` / `ForkSnapshot::Interrupted`, `thread_manager.rs:168-188`)
- `ThreadId` (= conversation_id, UUID) 가 thread 의 영속 핸들. `~/.codex/sessions/...jsonl` 파일명에도 박혀 있음.

### 2.2 `Session` — turn 디스패처

- 위치: `codex-rs/core/src/session/session.rs` 와 `codex-rs/core/src/session/mod.rs:2390-2410` (`record_into_history`, `record_conversation_items`).
- `INITIAL_SUBMIT_ID = ""` 가 첫 `SessionConfigured` 이벤트의 sub_id (`session/mod.rs:415`).
- 생성 시 `initial_history` 를 받아 `ContextManager` 에 미리 채우고, 이후 `Op::UserInput` 들을 받아 `run_turn` (`codex-rs/core/src/session/turn.rs:137`) 로 분배.
- `record_conversation_items` 가 in-memory + rollout + 이벤트 송신을 한 번에 처리 (`session/mod.rs:2391-2399`).

### 2.3 `ContextManager` — 모델에 보낼 history 누적기

- 위치: `codex-rs/core/src/context_manager/history.rs:33-71`
- 내부 필드:
  - `items: Vec<ResponseItem>` — 가장 오래된 항목이 앞. 사용자 메시지(role=`user`/`developer`), 어시스턴트 메시지(role=`assistant`), 추론 블록(`Reasoning`), 함수 호출(`FunctionCall`), 함수 출력(`FunctionCallOutput`), local shell 호출, 커스텀 툴 호출, web search, image gen, compaction marker 까지 모든 turn-time 페이로드를 보관.
  - `history_version: u64` — compaction/rollback 마다 bump.
  - `token_info: Option<TokenUsageInfo>` — 직전 API 응답 사용량.
  - `reference_context_item: Option<TurnContextItem>` — context diff 의 baseline (cwd, environment, plugins, skills 등).
- `record_items` (`history.rs:99-113`) 는 `is_api_message` 필터를 통과한 항목만 push, `process_item` 에서 truncation 정책 적용 (`TruncationPolicy::Tokens(...)`).
- `for_prompt(input_modalities)` (`history.rs:119-`) 가 **현재 turn 에 보낼 슬라이스** 를 만들 때 호출되어 `normalize_history` 로 dangling tool_call 보정, image stripping 등을 수행 (`codex-rs/core/src/context_manager/normalize.rs:14-`).
- `developer→user` wire 매핑 자체는 `ContextManager` 가 아닌 `ChatRequestBuilder` 단계에서 일어난다 (§5 참고). 즉 in-memory 에는 `developer` role 그대로 살아있다.

### 2.4 보조 누적 자료

- `TurnContext` (`codex-rs/core/src/session/turn_context.rs`) — turn 단위 read-only snapshot (model_info, sub_id, truncation_policy, plugins, skills, collaboration_mode 등).
- `ActiveTurn`/`SessionTask` — 현재 turn 의 cancellation token, pending input queue.
- `TokenUsageInfo` 는 `ContextManager` 가 직접 보유. 자동 compaction 의 trigger 신호로 사용 (§4).

---

## 3. 디스크 저장소

기본 루트: `~/.codex/` (env `CODEX_HOME` 으로 override; `codex-rs/utils/home-dir/src/lib.rs:13-63`). SQLite 만 따로 떼서 옮기고 싶을 때는 `CODEX_SQLITE_HOME` 환경변수가 있다 (`codex-rs/state/src/lib.rs:62`, `codex-rs/config/src/config_toml.rs:265`).

### 3.1 `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`

- 정의: `codex-rs/rollout/src/lib.rs:21` (`SESSIONS_SUBDIR = "sessions"`).
- 경로 계산: `codex-rs/rollout/src/recorder.rs:1371-1401`. 파일명 형식 `rollout-YYYY-MM-DDThh-mm-ss-<thread_id>.jsonl`.
- 포맷: JSONL (한 줄 = 한 `RolloutLine`). 첫 줄은 `SessionMeta` (id, cwd, source, base_instructions, dynamic_tools 등). 그 후 `ResponseItem`/`EventMsg`/`Compacted`/`TurnContext` 들이 시간순으로 append.
- 쓰기 시점: `RolloutRecorder` 의 background tokio task. `record_items` → `RolloutCmd::AddItems` → 파일 append (`recorder.rs:103-115`, 백그라운드 writer 1417-).
- 어떤 항목이 들어가는지: `codex-rs/rollout/src/policy.rs:14-181`.
  - 항상 들어감: `Message`, `Reasoning`, `FunctionCall`, `FunctionCallOutput`, `LocalShellCall`, `CustomToolCall*`, `WebSearchCall`, `ImageGenerationCall`, `Compaction*`, `SessionMeta`, `TurnContext`, `Compacted`.
  - **Limited mode** (`persist_extended_history=false`, 기본): `UserMessage`, `AgentMessage`, `AgentReasoning*`, `PatchApplyEnd`, `TokenCount`, `ContextCompacted`, `Turn(Started|Complete|Aborted)`, `McpToolCallEnd`, `WebSearchEnd`, `ImageGenerationEnd`, plan ItemCompleted 까지.
  - **Extended mode**: 추가로 `Error`, `GuardianAssessment`, `ExecCommandEnd`, `CollabAgent*End`, `DynamicToolCall*` 등이 함께 기록 (`policy.rs:121-131`).
- 읽기 시점: `resume`/`fork` 진입 시 `RolloutRecorder::get_rollout_history` (`recorder.rs:931-946`) 가 전체 파일을 한번에 읽어 `ResumedHistory { conversation_id, history, rollout_path }` 반환.
- 보존 정책: 자동 만료 없음. 사용자가 archive 하면 같은 트리 구조로 `~/.codex/archived_sessions/YYYY/MM/DD/...jsonl` 로 이동 (`codex-rs/rollout/src/lib.rs:22`, `recorder.rs:1206`).

### 3.2 `~/.codex/state_5.sqlite` (+ WAL)

- 파일명: `STATE_DB_FILENAME=state`, `STATE_DB_VERSION=5` → `state_5.sqlite` (`codex-rs/state/src/lib.rs:64-67`, `codex-rs/state/src/runtime.rs:210-220`).
- `~/.codex/logs_2.sqlite` 도 동일 디렉토리에 생성 (`logs_db_path`, `state/src/runtime.rs:222-228`).
- 내용: thread metadata 색인 (id, cwd, model_provider, source, created_at, updated_at, archived_at, rollout_path, dynamic_tools, memory_mode, parent/child agent edges 등). Thread 본체 history 는 들어가지 않음 — 인덱스/색인용.
- 마이그레이션은 `codex-rs/state/src/migrations.rs`. 버전 차이가 큰 옛 DB 파일은 startup 에서 정리 (`runtime.rs:230-`).
- 쓰기 시점: `apply_rollout_items` (`codex-rs/rollout/src/state_db.rs:601-644`) — rollout 에 새 item 이 추가될 때마다 builder 가 metadata 를 채워 SQLite 로 upsert. 또한 startup backfill (`init` 시 `metadata::backfill_sessions`, `state_db.rs:103-179`) 이 디스크에 있는 JSONL 들을 스캔해 SQLite 와 일치시킨다.
- 읽기 시점: `codex resume` picker 의 thread 목록, `find_rollout_path_by_id` (`state_db.rs:390-403`), `list_threads_db` (`state_db.rs:301-387`).
- env override: `CODEX_SQLITE_HOME`. 미지정 시 `codex_home` 과 동일 디렉토리.
- 보존 정책: 자동 만료 없음. 단 rollout JSONL 이 사라진 thread 행을 list 시 자동 삭제 (`state_db.rs:362-378`, "stale_db_path_dropped").

### 3.3 `~/.codex/shell_snapshots/<thread_id>.<nonce>.{sh|ps1}`

- 정의: `codex-rs/core/src/shell_snapshot.rs:35` (`SNAPSHOT_DIR = "shell_snapshots"`), 경로 `shell_snapshot.rs:135-141`.
- 내용: 사용자 셸 환경변수 export 스냅샷. exec 툴이 `set -e; . <snapshot>` 로 source 하여 매 명령 실행 시 PATH/alias 등을 복원.
- 쓰기 시점: thread 기동 시 + 주기적 refresh.
- 보존 정책: 3일 (`SNAPSHOT_RETENTION = 60*60*24*3`, `shell_snapshot.rs:34`). 실행 중인 thread 가 사용 중이면 보호.
- 민감도: 사용자 환경변수의 평문 export 가 그대로 들어간다 — 폐쇄망에서도 토큰/시크릿이 환경변수에 있다면 디스크에 남으므로 백업 정책 시 주의.

### 3.4 `~/.codex/memories/`

- 정의: `codex-rs/memories/write/src/lib.rs:118-136`.
- 구조:
  - `~/.codex/memories/raw_memories.md` — stage-1 raw memory 들의 머지 결과.
  - `~/.codex/memories/rollout_summaries/<thread>.md` — 각 thread 의 롤아웃 요약.
  - `~/.codex/memories/extensions/<name>/instructions.md` — 사용자가 추가한 메모리 확장.
  - `~/.codex/memories/phase2_workspace_diff.md` — 일시 파일 (phase2 consolidator 가 읽고 버림, `lib.rs:113-115`).
- 활성화 조건: `config.memories.generate_memories = true` 일 때만 stage-1 작업이 SQLite (`codex_state::Stage1Output`) 로 들어가고 storage helper 들이 위 마크다운들을 (재)빌드 (`memories/write/src/storage.rs:13-78`). fork 의 `ThreadStoreConfig` 가 `Local` 이고 `generate_memories=false` 가 기본이면 이 디렉토리는 비어있다.
- 비활성 thread 도 SQLite 의 `thread_memory_mode` 컬럼이 `Disabled` 로 기록된다 (`session/session.rs:408-413`).

### 3.5 `~/.codex/skills/.system/...`

- 정의: `codex-rs/skills/src/lib.rs:12-43`.
- 번들된 시스템 skill 들이 startup 에 풀려 들어간다. fingerprint marker 로 idempotent.
- 사용자 skill 은 `~/.codex/skills/<skill-name>/` 로 운영자가 직접 둔다 (`core-skills/src/loader.rs`).
- conversation 데이터는 들어가지 않음.

### 3.6 `~/.codex/external_agent_session_imports.json`

- 정의: `codex-rs/external-agent-sessions/src/ledger.rs:12-98` (`SESSION_IMPORT_LEDGER_FILE`).
- 외부(Claude Code 등) session 파일을 import 했을 때, 중복 import 방지용 ledger. 단순 JSON 파일.

### 3.7 rollout-trace bundle (옵트인)

- 정의: `codex-rs/rollout-trace/src/thread.rs:42` — `CODEX_ROLLOUT_TRACE_ROOT` env 가 설정된 경우에만 bundle 디렉토리를 만든다 (`thread.rs:99-`).
- 구조: `<bundle_dir>/manifest.json`, `<bundle_dir>/raw_events.jsonl`, `<bundle_dir>/payloads/<id>.json`, 그리고 reduced 결과 `state.json` (`codex-rs/rollout-trace/src/bundle.rs:12` `REDUCED_STATE_FILE_NAME = "state.json"`, `writer.rs:50-83`).
- 디버깅/리플레이용. **기본 비활성**. 폐쇄망 운영자가 의도적으로 켜지 않으면 어떤 파일도 생기지 않음.
- 민감도: prompt/툴 입력 등 모든 raw event 가 그대로 저장된다고 README 가 명시 (`rollout-trace/README.md:5`).

### 3.8 `~/.codex/log/`

- `codex-rs/config/src/config_toml.rs:269` — `$CODEX_HOME/log` 가 기본 로그 디렉토리 (config 의 `log.dir` 로 override 가능). conversation 자체보다는 tracing 로그.

### 3.9 그 외

- `~/.codex/auth.json`, `~/.codex/.credentials.json` — 인증 토큰. 본 fork 의 게이트웨이 사용 모드에선 비어있을 수 있으나, 기본 코드에는 여전히 keyring fallback 으로 쓰는 경로가 있다 (`config/src/types.rs:89-107`).
- `~/.codex/agent-graph-store/` 같은 별도 디렉토리는 **존재하지 않는다**. 이름이 동일한 crate 가 있지만 (`codex-rs/agent-graph-store/`) 실제 저장은 `state_5.sqlite` 에 SQLite 테이블로 들어간다 (`codex-rs/agent-graph-store/src/local.rs:13-29`).
- `codex-rs/state/` 는 crate 디렉토리이며, runtime 이 만드는 파일은 §3.2 의 SQLite 두 개뿐이다.
- `~/.codex/external-agent-sessions/` 같은 디렉토리도 존재하지 않는다 — ledger 한 파일만 있을 뿐 (§3.6).

---

## 4. Resume / Fork / Compact

### 4.1 Resume

- CLI: `codex resume [SESSION_ID] [--last] [--all] [--include-non-interactive]` (`codex-rs/cli/src/main.rs:274-298`).
- `SESSION_ID` 는 thread_id (UUID) 또는 thread name. 미지정 시 picker, `--last` 면 최신 자동 선택.
- 내부 흐름: SQLite 에서 thread_id → `rollout_path` 조회 (`state_db::find_rollout_path_by_id`) → `RolloutRecorder::get_rollout_history` 가 JSONL 전체를 다시 파싱 → `InitialHistory::Resumed { conversation_id, history, rollout_path }` 로 `ThreadManager::resume_thread_from_rollout` (`thread_manager.rs:642-687`).
- 결과: 동일 `thread_id` 로 새 `Session` 이 만들어지고, `RolloutRecorder` 는 같은 JSONL 파일을 append 모드로 다시 연다 (`recorder.rs:185-188`, `Resume { path, event_persistence_mode }`). 즉 동일 파일에 누적이 이어진다.
- `~/.codex/state_5.sqlite` 의 `updated_at` 도 갱신된다 (`state_db::touch_thread_updated_at`).

### 4.2 Fork

- CLI: `codex fork [SESSION_ID] [--last] [--all]` (`cli/src/main.rs:300-320`).
- `ForkSnapshot::TruncateBeforeNthUserMessage(n)` 또는 `Interrupted` 모드 (`thread_manager.rs:168-196`):
  - `TruncateBeforeNthUserMessage(n)`: n번째 user 메시지 직전까지의 history 만 가져가 새 `thread_id` 로 시작.
  - `Interrupted`: 현재 보존된 prefix 를 그대로 가져가되, 미완료 turn suffix 가 있으면 `<turn_aborted>` 마커 추가.
- 결과: **새** rollout JSONL 파일이 새 `thread_id` 로 생성된다 (원본 JSONL 은 그대로 유지). SQLite 에는 `forked_from_id` 컬럼으로 부모 링크가 남는다 (`thread-store/src/local/create_thread.rs`).

### 4.3 Compact

- 위치: `codex-rs/core/src/compact.rs`, `codex-rs/core/src/compact_remote.rs`, `codex-rs/core/src/compact_remote_v2.rs`.
- 자동 trigger: `auto_compact_token_limit` (default = `context_window * 0.9`, `codex-rs/protocol/src/openai_models.rs:295-333`). turn 시작 직전 (`run_pre_sampling_compact`, `session/turn.rs:156-163`) 과 turn 종료 후 (`turn.rs:725-736`) 두 군데서 체크.
- 수동 trigger: `/compact` slash command → `run_compact_task` (`compact.rs:92-114`).
- 동작:
  1. 기존 history 를 모델에게 요약 prompt (`templates/compact/prompt.md`) 로 보내 요약문을 받음.
  2. `build_compacted_history` (`compact.rs:447-511`) 로 새 history 를 만든다:
     - `initial_context` (system instructions, environment context, …) 유지.
     - 최근 user 메시지들을 `COMPACT_USER_MESSAGE_MAX_TOKENS=20_000` 까지 token-budget 으로 보존 (오래된 것부터 잘림).
     - 마지막에 요약문을 `role=user` 메시지로 push.
  3. `ContextManager.items` 를 통째로 교체, `history_version` bump.
  4. JSONL 에는 `RolloutItem::Compaction` / `RolloutItem::Compacted` 마커가 append 된다 — 원본 turn 들도 파일에는 그대로 남아있다 (역사 추적 가능).
- developer 메시지 처리: compaction 후 developer instruction 들은 `initial_context` 재주입(`InitialContextInjection::DoNotInject` vs `BeforeLastUserMessage`, `compact.rs:55-59`)을 통해 다음 정상 turn 시작 전에 새로 합성된다. 즉 in-memory history 에서는 사라졌다가 다음 turn 에 재구성.
- 단, fork 의 `developer→user` wire 매핑(§5)은 compaction 과 무관하게 모든 wire 송신 시점에 적용된다.

---

## 5. Wire 형식 (실제로 모델에게 나가는 페이로드)

- 빌더: `codex-rs/codex-api/src/requests/chat.rs:31-322` (`ChatRequestBuilder`).
- 입력: `Vec<ResponseItem>` (= `ContextManager::for_prompt(...)` 결과) + base instructions + tool spec.
- 출력: `messages: [...]` 배열로 `system → 본문 메시지들 → tool_calls → tool outputs` 순.

### 5.1 role 매핑

- system 메시지는 builder 가 직접 넣는다 (`requests/chat.rs:60`): `{"role":"system","content":<base_instructions>}` 1개만.
- 그 외 ResponseItem 의 role 은:
  - `assistant` → `assistant`.
  - `user` → `user`.
  - **`developer` → `user`** (`requests/chat.rs:198`: `let outbound_role = if role == "developer" { "user" } else { role };`). 이 매핑은 fork 가 게이트웨이 호환을 위해 의도적으로 추가한 것이며, 자세한 결정 배경은 `fork-docs/work-log-2026-05-08.md` 의 §"developer → user 매핑" 참고.
- assistant 메시지의 reasoning 텍스트는 별도 `reasoning` 필드로 같은 메시지에 attach (`requests/chat.rs:200-205`).

### 5.2 tool_call / tool_output 정렬 규칙

- chat completions 표준대로 `tool_calls: [...]` 가 한 assistant 메시지에 묶여야 한다. `push_tool_call_message` (`requests/chat.rs:324-360`) 가 이전 메시지가 `assistant` + `content=null` 이면 `tool_calls` 배열에 append, 아니면 새 assistant 메시지를 만든다.
- 그 다음 각 `FunctionCallOutput` 은 `{"role":"tool","tool_call_id":<call_id>,"content":...}` 로 직후에 따라붙는다 (`requests/chat.rs:240-291`).
- 결과적으로 `assistant(tool_calls=[a,b,c]) → tool(a) → tool(b) → tool(c)` 순서가 유지된다.

### 5.3 dedup / 이미지 / reasoning

- 동일한 assistant 텍스트가 연속되면 두 번째는 drop (`requests/chat.rs:181-188`).
- 이미지가 있으면 `content` 가 `[{"type":"text"...}, {"type":"image_url"...}]` 배열 형태로 나간다.
- 마지막 user 메시지 이후 등장한 reasoning 들은 가까운 assistant/FunctionCall 에 attach 되어 별도 메시지로는 송신되지 않는다 (`requests/chat.rs:96-154`).

### 5.4 헤더

- `session_id` 헤더에 conversation_id, `x-openai-subagent` 헤더에 sub-agent 종류 (`requests/chat.rs:312-315`).

---

## 6. 운영자용 절차 (세션 관리 recipe)

### 6.1 새 세션 시작

```sh
codex                    # TUI, 새 thread
codex exec "..."         # headless, 새 thread
```

새 `thread_id` (UUID) 가 생성되고 `~/.codex/sessions/YYYY/MM/DD/rollout-...-<id>.jsonl` 이 만들어진다.

### 6.2 직전 세션 이어가기

```sh
codex resume --last                   # 가장 최근 thread, picker 없이
codex resume <UUID>                   # 특정 thread_id
codex resume <thread-name>            # SQLite 에 등록된 thread name
codex resume --all                    # cwd 무관 전체 picker
codex resume --include-non-interactive  # exec/sub-agent 도 포함
```

이어쓰기 시 동일 JSONL 에 append. `state_5.sqlite` 의 `updated_at` 도 갱신된다.

### 6.3 분기 (fork)

```sh
codex fork --last                # 최근 thread 의 마지막 user 메시지 직전까지 분기
codex fork <UUID>                # 특정 thread 분기
```

새 thread_id, 새 JSONL 이 만들어지고 `forked_from_id` 컬럼으로 부모와 연결된다.

### 6.4 단일 세션 영구 삭제

다음 4개 위치를 함께 정리해야 흔적이 모두 사라진다:

1. `~/.codex/sessions/**/rollout-*-<UUID>.jsonl` — 본문 (`find ~/.codex/sessions -name 'rollout-*-<UUID>.jsonl' -delete`).
2. `~/.codex/archived_sessions/**/rollout-*-<UUID>.jsonl` — archive 했다면 여기로 이동되어 있음.
3. `~/.codex/state_5.sqlite` — 다음 startup 의 list 단계에서 stale row 자동 정리 (`state_db.rs:362-378`). 즉시 정리하려면 SQLite 클라이언트로 `DELETE FROM threads WHERE id='<UUID>';`.
4. `~/.codex/shell_snapshots/<UUID>.*` — 셸 스냅샷.

### 6.5 모든 세션 wipe

```sh
# 진행 중인 codex 프로세스 종료 후
rm -rf ~/.codex/sessions ~/.codex/archived_sessions
rm -f  ~/.codex/state_5.sqlite ~/.codex/state_5.sqlite-wal ~/.codex/state_5.sqlite-shm
rm -f  ~/.codex/logs_2.sqlite  ~/.codex/logs_2.sqlite-wal  ~/.codex/logs_2.sqlite-shm
rm -rf ~/.codex/shell_snapshots
rm -rf ~/.codex/memories                 # auto-memory 가 켜져 있던 경우
rm -f  ~/.codex/external_agent_session_imports.json
```

`~/.codex/skills/.system` 은 conversation 데이터가 아니므로 굳이 지울 필요 없음. `~/.codex/log/` 는 트레이싱 로그.

### 6.6 위치 변경 환경변수

| 변수 | 효과 | 정의 위치 |
| --- | --- | --- |
| `CODEX_HOME` | `~/.codex` 자체를 통째로 다른 디렉토리로 옮김. **존재하는 디렉토리** 여야 한다 (`utils/home-dir/src/lib.rs:23-50`). | `codex-rs/utils/home-dir/src/lib.rs:13-63` |
| `CODEX_SQLITE_HOME` | `state_5.sqlite`, `logs_2.sqlite` 만 따로 다른 디렉토리로 분리. JSONL 은 `CODEX_HOME` 따라감. | `codex-rs/state/src/lib.rs:62`, `codex-rs/config/src/config_toml.rs:265` |
| `CODEX_ROLLOUT_TRACE_ROOT` | 켜질 때만 trace bundle 을 그 경로 아래에 쓴다. 기본 미설정 = 끔. | `codex-rs/rollout-trace/src/thread.rs:42` |

`XDG_CONFIG_HOME` 등 XDG 변수는 **사용하지 않는다**: fork 는 `~/.codex` 하드코딩 + `CODEX_HOME` 만 본다.

---

## 7. 폐쇄망 관점 체크리스트 (멀티턴/저장 한정)

> 일반적인 네트워크 egress 감사는 별도 air-gap audit 문서가 다룬다. 여기서는 멀티턴/저장 컴포넌트가 추가로 외부와 통신할 가능성이 있는 지점만 추린다.

- **`RemoteThreadStore` (gRPC)** — `codex-rs/thread-store/src/remote/mod.rs:34-52`. `ThreadStoreClient::connect(endpoint)` 로 외부 gRPC 서버에 thread 메타/본문을 위임한다. **활성 조건**: `config.toml` 에 `experimental_thread_store = { kind = "remote", endpoint = "..." }` 또는 deprecated `experimental_thread_store_endpoint = "..."` 가 있을 때만 (`codex-rs/core/src/config/mod.rs:1726-1738`). 운영자는 두 키가 비어있는지 확인. 본 fork 의 기본 config 에는 둘 다 미설정.
- **`AgentGraphStore`** — `codex-rs/agent-graph-store/src/local.rs` 의 `LocalAgentGraphStore` 만 wired in (`thread_manager.rs:287-289`). 외부 호출 없음. (단, 만약 향후 `RemoteAgentGraphStore` 가 추가되면 여기서 빠질 수 있음 — 현재 trait dispatch 만 있음.)
- **`RolloutRecorder`** — 100% 로컬 파일 I/O. 외부 호출 없음 (`codex-rs/rollout/src/recorder.rs`).
- **`StateRuntime` (SQLite)** — 100% 로컬. `sqlx::sqlite` 만 사용 (`codex-rs/state/src/runtime.rs`).
- **`memories/` writer** — phase-1/phase-2 가 LLM 호출을 만들지만 (`memories/write/src/phase1.rs`, `phase2.rs`), 그 호출은 정상 thread 처럼 fork 의 chat completions 게이트웨이를 통과한다. 별도의 호스트로 빠지지 않음. **활성 조건**: `config.memories.generate_memories = true`.
- **`rollout-trace`** — 외부 송신 없음. 단 `CODEX_ROLLOUT_TRACE_ROOT` 가 켜진 디렉토리에 prompt/툴 입력 raw 가 평문으로 쌓이므로 (`rollout-trace/README.md:5`), 폐쇄망 환경에서도 실수로 켜지 않도록 주의.
- **`shell_snapshots`** — 외부 송신 없음. 단 사용자 환경변수가 평문 export 로 디스크에 남는다 — 시크릿 환경변수가 있다면 백업 정책에서 제외할지 검토.
- **`external_agent_session_imports.json`** — 외부 송신 없음. 단순 import 중복 방지 ledger.
- **wire 자체** — chat completions 호출은 `ChatRequestBuilder` (§5) 가 만들어 fork 의 게이트웨이로만 나간다. 본 fork 가 추가한 `developer→user` 매핑이 적용된 상태이므로, 모든 turn 의 모든 메시지(시스템 instructions 와 user 입력 포함)가 게이트웨이의 access log 에 노출된다는 점만 확인.

폐쇄망에서 살펴봐야 할 단 한 줄:

```
codex-rs/core/src/config/mod.rs:1726-1738   # thread_store_config — Remote 분기 차단 확인
```

---

## 8. 운영자가 한 파일만 기억해야 한다면

**`~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<thread_id>.jsonl`** 이다. 이 JSONL 한 개에 한 thread 의 모든 turn (사용자 입력, 모델 응답, tool 호출, tool 출력, compaction 마커) 이 시간순으로 들어 있고, `state_5.sqlite` 는 이 파일을 가리키는 인덱스에 불과하다. 이 JSONL 이 살아있으면 `codex resume <thread_id>` 로 전체 대화를 재구성할 수 있고, 이 JSONL 이 사라지면 어떤 SQLite 행이 남아있어도 다음 startup 의 stale-path 정리에서 자동 폐기된다.
