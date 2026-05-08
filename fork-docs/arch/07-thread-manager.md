# 07 — ThreadManager / Session / Conversation 라이프사이클

이 문서는 codex 의 in-process 에이전트 실행 단위 (`ThreadManager` / `CodexThread` / `Codex` / `Session`) 가 어떻게 구성되어 있고, 어떤 순서로 생성·실행·종료·resume·fork 되는지를 **타입 / 모듈 / 라이프사이클** 측면에서 정리한다. 디스크 쪽 영속화 (`~/.codex/sessions/...jsonl`, `state_5.sqlite`, JSONL 포맷, append-only 동작) 은 `fork-docs/multi-turn-and-storage-2026-05-08.md` 에서 이미 다뤘으므로 본 문서는 **코드 구조** 를 중심으로 보고 영속 동작은 거기서 상호 참조한다.

관련 turn 단위 흐름은 `02-turn-lifecycle.md` 를, wire 변환은 `03-wire-protocol.md` 를 함께 본다.

주 소스:

- `codex-rs/core/src/thread_manager.rs` — `ThreadManager`, `ForkSnapshot`, fork/resume 진입점.
- `codex-rs/core/src/thread_manager_tests.rs` — contract 테스트.
- `codex-rs/core/src/codex_thread.rs` — `CodexThread` (외부 노출 핸들).
- `codex-rs/core/src/session/mod.rs`, `codex-rs/core/src/session/session.rs` — `Codex`, `Session`, `SessionConfiguration`, `CodexSpawnArgs`.
- `codex-rs/protocol/src/protocol.rs` — `InitialHistory`, `ResumedHistory`, `SessionMeta.forked_from_id`.
- `codex-rs/state/` — `StateDbHandle`, `state_5.sqlite` 정의.
- `codex-rs/thread-store/` — `ThreadStore` trait, `LocalThreadStore` / `RemoteThreadStore` / `InMemoryThreadStore`.
- `codex-rs/agent-graph-store/` — parent/child agent edge 저장소.
- `codex-rs/thread-manager-sample/` — `ThreadManager` 단독 사용 예제 (one-turn driver).

---

## 1. 용어 구분 — thread / session / conversation / agent

upstream 코드는 역사적으로 "conversation" 이라는 단어를 자주 썼는데, 최근 리팩터에서 외부 식별자는 **thread** 로 통일됐다. 동의어 / 아닌 것을 정확히 구분한다.

- **thread** — 사용자가 보는 영속 단위. 식별자는 `ThreadId` (UUID, `codex-protocol::ThreadId`). `~/.codex/sessions/.../rollout-*-<thread_id>.jsonl` 파일명, `state_5.sqlite` row, `--resume <id>` 인자에 모두 같은 값이 박힌다. 외부 노출 타입은 `codex-core::CodexThread` (`codex-rs/core/src/codex_thread.rs:99`). `Op` 송신과 `Event` 수신을 감싸는 facade 다.
- **conversation** — 같은 개념의 옛 이름. 코드에는 아직 잔존한다 (`Session::conversation_id: ThreadId`, `RealtimeConversationManager`, `ConversationPathResponseEvent`). 신규 코드에서는 쓰지 않고 thread 로 통일하라는 게 합의 사항. `thread_manager.rs:151-157` 도 `formerly called a conversation` 이라고 명시한다.
- **session** — thread 의 **in-process 런타임**. `codex-core::Session` (`session/session.rs:12`) 은 한 번에 1 개의 active turn 만 가질 수 있는 상태기로, mailbox / `ContextManager` / `RealtimeConversationManager` / MCP 핸들러 / approval / sandbox policy 를 묶는다. 같은 thread 라도 프로세스를 껐다 켜서 resume 하면 **새 `Session` 인스턴스** 가 만들어지지만 `ThreadId` 는 동일하다. 즉 `session ⊂ thread` 라기보다 `session = thread 의 현재 활성화 인스턴스`.
- **`Codex`** — `Session` 위에 얹힌 채널 페어. `Codex { tx_sub, rx_event, session, session_loop_termination }` (`session/mod.rs:364-373`). `Op` 를 `Submission` 으로 감싸 송신하는 `submission_loop` 백그라운드 task 를 소유한다. 외부에서는 보통 `CodexThread` 가 `Codex` 를 한 번 더 감싼 인터페이스만 본다.
- **agent** — 한 thread 안에서 모델이 도구를 통해 서브 thread 를 띄우는 multi-agent 개념. 즉 thread 가 노드, agent edge 가 부모-자식 관계다 (§5). `SessionSource::SubAgent(SubAgentSource::ThreadSpawn { parent_thread_id, depth, .. })` 가 새 thread 가 sub-agent 로 spawn 되었음을 표시한다 (`thread_manager.rs:1297-1303`). `CodexThread` 자체는 agent 인지 사용자가 직접 띄운 thread 인지 구분하지 않는다 — 구분은 `session_source` 필드만 본다.

요약:

| 개념 | 타입 | 영속? | 비고 |
| --- | --- | --- | --- |
| thread | `ThreadId`, `CodexThread` | yes (sessions JSONL + state DB) | 사용자가 보는 단위 |
| session | `Session`, `Codex` | no (in-memory) | thread 의 활성 인스턴스 |
| conversation | (legacy) | — | thread 의 옛 이름 |
| agent edge | `AgentGraphStore` row | yes (`state_5.sqlite`) | thread spawn 부모/자식 |

---

## 2. ThreadManager — 동시성 모델과 라이프사이클

`ThreadManager` (`thread_manager.rs:213-216`) 는 한 프로세스에 1 개. 내부적으로 `Arc<ThreadManagerState>` 를 들고 있고 (`thread_manager.rs:242-259`), `state.threads: Arc<RwLock<HashMap<ThreadId, Arc<CodexThread>>>>` 가 살아있는 thread 들을 보관하는 유일한 진실의 출처다.

생성 시 주입되는 의존성 (`ThreadManager::new`, `thread_manager.rs:309-355`):

- `auth_manager` — 토큰 / 자격 증명.
- `models_manager` — provider 별 model catalog (refresh 전략 포함).
- `environment_manager` — `codex-exec-server` 의 sandbox / cwd 정책.
- `skills_manager` / `plugins_manager` / `mcp_manager` — thread 간 공유되는 도구 / skill 캐시.
- `skills_watcher` — `~/.codex/skills/**` 변경 시 캐시 invalidate (`build_skills_watcher`, `thread_manager.rs:110-149`).
- `thread_store: Arc<dyn ThreadStore>` — 영속 저장소 (Local / Remote / InMemory).
- `state_db: StateDbHandle` — `state_5.sqlite` 연결 풀.
- `agent_graph_store: Arc<dyn AgentGraphStore>` — parent/child edge.
- `session_source: SessionSource` — 이 매니저가 만드는 thread 의 기본 출처 (cli / vscode / exec / mcp / subagent).

### 2.1 동시성 모델

- thread 등록 / 조회는 `RwLock<HashMap>` 만 잡는다. 한 thread 에 보내는 `Op` 는 read lock 을 잡고 `Arc<CodexThread>` 만 복제한 뒤 그 thread 의 자기 채널로 보낸다 (`ThreadManagerState::send_op`, `thread_manager.rs:954-962`). 따라서 **여러 thread 는 진짜로 병렬** 로 돈다 — 매니저는 단순 dispatcher 다.
- 한 thread 안에서는 `Session` 이 single active turn 을 보장한다 (`session.rs:9-12` 주석). thread 간 동시성은 매니저가, thread 내 직렬성은 `Session` 이 책임진다. `Session::active_turn: Mutex<Option<ActiveTurn>>` 와 `mailbox` (`session.rs:26-28`) 가 한 turn 을 직렬화하고, idle 상태에서 들어온 입력은 `idle_pending_input` 큐에 잠시 보류된다.
- `subscribe_thread_created()` (`thread_manager.rs:528-530`) 가 broadcast channel 을 노출해 IDE / app-server 가 새 thread 가 등록되는 즉시 알 수 있다 (`THREAD_CREATED_CHANNEL_CAPACITY = 1024`). resumed thread 도 같은 채널로 통지된다.
- 종료 시 `shutdown_all_threads_bounded(timeout)` (`thread_manager.rs:753-799`) 가 모든 thread 에 `Op::Shutdown` 을 `FuturesUnordered` 로 동시에 보내고, `Complete` / `SubmitFailed` / `TimedOut` 으로 분류해 `ThreadShutdownReport` 를 돌려준다. 완료된 것만 맵에서 제거한다 — 타임아웃된 thread 는 매니저에 남아 재시도 가능하게 한다. 이 패턴 덕에 SIGTERM 시 graceful shutdown 이 부분 실패해도 남은 thread 에 다시 신호를 보낼 수 있다.
- `ThreadManagerState` 자체가 `Arc` 로 감싸져 있는 이유는 `AgentControl::new(Arc::downgrade(&state))` (`thread_manager.rs:885-887`) — sub-agent 가 부모 매니저를 weak ref 로만 들어 cycle 없이 형제 thread 에 access 할 수 있게 한다.

### 2.2 단일 thread 라이프사이클

```
┌─────────────────┐ start_thread          ┌────────────────────┐
│  no entry       ├──────────────────────►│ spawn_thread       │
│ (HashMap miss)  │                       │  → Codex::spawn    │
└─────────────────┘                       │  → Session::new    │
                                          │  → submission_loop │
                                          │    spawn (tokio)   │
                                          └────────┬───────────┘
                                                   │ first event:
                                                   │ SessionConfigured
                                                   ▼
                                          ┌────────────────────┐
                                          │ finalize_thread    │
                                          │ _spawn: insert into│
                                          │ HashMap, return    │
                                          │ NewThread          │
                                          └────────┬───────────┘
                                                   │ Op::UserInput
                                                   ▼
                                          ┌────────────────────┐
                          turn-by-turn  │ active session     │
                          loop          │ (one active turn   │
                                          │  at a time)        │
                                          └────────┬───────────┘
                                                   │ Op::Shutdown
                                                   ▼
                                          ┌────────────────────┐
                                          │ shutdown_and_wait  │
                                          │ → submission_loop  │
                                          │   exits → JoinHandle│
                                          │   resolves         │
                                          └────────────────────┘
```

핵심 진입점 (모두 `thread_manager.rs`):

- `start_thread(config)` / `start_thread_with_tools` / `start_thread_with_options` — 새 thread (`InitialHistory::New`).
- `resume_thread_from_rollout(config, path, ...)` — JSONL 파일에서 history 복원 (§4).
- `fork_thread(snapshot, config, path, ...)` / `fork_thread_from_history` — 기존 thread 의 prefix 잘라 새 thread 생성 (§3).
- `get_thread(id)` / `list_thread_ids()` / `remove_thread(id)` / `shutdown_all_threads_bounded(...)` — 조회 / 정리.

`finalize_thread_spawn` (`thread_manager.rs:1238-1281`) 가 약속하는 invariant: spawn 직후 첫 이벤트는 반드시 `EventMsg::SessionConfigured` 이며 그 sub_id 는 `INITIAL_SUBMIT_ID = ""` (`session/mod.rs:415`) — 그렇지 않으면 `CodexErr::SessionConfiguredNotFirstEvent` 가 떨어진다. 이 boundary 가 있어 호출자는 thread 등록과 첫 이벤트 수신을 한 번의 await 로 묶어 처리할 수 있다. 또한 `HashMap::Entry::Vacant` 체크로 race 조건 (동시에 같은 thread_id 로 두 번 등록 시도) 을 막고, 중복이면 spawn 된 `Codex` 를 즉시 shutdown 시켜 자원 누수를 방지한다.

### 2.3 `CodexSpawnArgs` 의 책임

`Codex::spawn_internal` (`session/mod.rs:446-673`) 은 `ThreadManager` 가 모은 모든 의존성을 `CodexSpawnArgs` (`session/mod.rs:384-413`) 한 구조체로 받아 다음 순서로 처리한다:

1. **plugin / skill 로드** — `plugins_manager.plugins_for_config(...)` 로 effective skill roots 결정, 그 위에 `skills_manager.skills_for_config(...)` 로 활성 skill 집합 확정. 실패한 skill 은 warn 으로만 기록하고 진행한다.
2. **base_instructions 우선순위 결정** — `config.base_instructions` → `conversation_history` 의 `SessionMeta.base_instructions` → 모델 기본값. 즉 resume 한 thread 는 원본의 base_instructions 를 그대로 쓴다 (`session/mod.rs:537-547`).
3. **dynamic_tools 결정** — 호출자가 명시했으면 그대로, 아니면 state DB 의 thread-start tools, 아니면 rollout `SessionMeta.dynamic_tools` (`session/mod.rs:551-574`).
4. **`SessionConfiguration` 빌드 후 `Session::new` 호출** — 이 시점에 새 `ThreadId` 가 발급되고 (`session.conversation_id`) `SessionConfigured` 이벤트가 큐에 푸시된다.
5. **`submission_loop` task spawn** — `tokio::spawn(submission_loop(session, config, rx_sub))`. 이 task 가 종료될 때까지의 future 를 `Shared<BoxFuture>` 로 박제해 (`session_loop_termination`) 여러 곳에서 await 할 수 있게 한다 (`shutdown_and_wait`, `wait_until_terminated`).

---

## 3. Fork — 무엇이 새로 만들어지고 무엇이 공유되는가

`codex fork [--last | <id>]` 또는 app-server 의 thread fork RPC 가 들어오면 `ThreadManager::fork_thread*` 가 호출된다. 사용자 관점에서 fork 는 "이 turn 직전 상태로 분기" 다. 코드 관점에서는 **새 `ThreadId` + 새 rollout JSONL + 새 `Session` 인스턴스** 를 만들고, history 만 잘라 채워주는 동작이다.

### 3.1 ForkSnapshot

snapshot 모드는 두 가지 (`thread_manager.rs:168-188`):

- `ForkSnapshot::TruncateBeforeNthUserMessage(n)` — 0-based n 번째 user message 직전에서 끊는다. 범위 밖이면서 mid-turn 이면 active turn 의 시작 직전까지로 fallback (즉 미완료 turn 은 버린다).
- `ForkSnapshot::Interrupted` — 현재 persisted snapshot 을 그대로 두되, mid-turn 이면 `<turn_aborted>` 마커를 append 해 "지금 인터럽트한 상태" 를 합성한다 (`append_interrupted_boundary`, `thread_manager.rs:1444-1480`).

`ForkSnapshot::From<usize>` (`thread_manager.rs:192-196`) 가 legacy `fork_thread(usize, ...)` callsite 호환을 책임진다.

### 3.2 새로 만들어지는 것

1. **새 `ThreadId`** — `Codex::spawn_internal` 가 매번 `Session::new` 안에서 새로 발급 (UUID v4).
2. **새 rollout JSONL** — 파일명에 새 `thread_id` 가 박힘. 첫 줄 `SessionMeta` 의 `forked_from_id: Some(parent_thread_id)` 가 부모와의 링크다 (`rollout/recorder.rs:85-101`, `protocol.rs:2734`).
3. **새 `Session` / `Codex` / `submission_loop` task** — config 도 새로 적용된다 (호출자가 override 한 cwd, model 등이 그대로 들어간다).
4. **새 `ContextManager`** — fork snapshot 의 `RolloutItem` 들을 `InitialHistory::Forked(...)` 로 받아 `Session::new` 가 다시 채운다.

### 3.3 공유되는 것

- `ThreadStore`, `state_db`, `agent_graph_store`, `models_manager`, `mcp_manager`, `skills_manager`, `plugins_manager`, `skills_watcher`, `environment_manager`, `auth_manager` — 모두 `Arc` 로 동일 인스턴스를 가져간다 (`spawn_thread_with_source` 의 `Arc::clone`, `thread_manager.rs:1198-1226`).
- 부모 thread 의 `~/.codex/sessions/.../<parent>.jsonl` 파일은 **건드리지 않는다**. 부모는 계속 자기 파일에 append, 자식은 자기 새 파일에 append. 두 파일은 `forked_from_id` 한 줄로만 묶인다.
- `SessionMeta.forked_from_id` 는 `InitialHistory::forked_from_id()` (`protocol.rs:2417-2431`) 로 다시 읽을 수 있고, 이를 통해 자식이 자기 부모를 추적한다. 즉 fork tree 의 ancestry 는 **rollout 파일 자체에 자기 기술적으로 박혀 있어** state DB 가 비어있어도 복원 가능하다.
- dynamic_tools, base_instructions 같은 thread-start 시점 결정값은 `Codex::spawn_internal` 이 우선순위를 따져 다시 계산한다 (`session/mod.rs:543-574`): 명시적 인자 → state DB 에 저장된 fork 부모의 thread-start tools → rollout `SessionMeta` → model default. 즉 fork 자식은 부모와 동일한 도구 세트를 자동 상속한다.

### 3.4 mid-turn 안전성

`snapshot_turn_state(history)` (`thread_manager.rs:1358-1409`) 가 fork 대상 history 를 한 번 훑어 `ends_mid_turn` / `active_turn_id` / `active_turn_start_index` 를 계산한다. `ThreadHistoryBuilder` (`codex-app-server-protocol`) 를 재사용해 turn 경계 (legacy event 도 포함) 를 일관되게 인식한다. 이 덕에 "fork 시점이 어시스턴트 응답 도중" 인 경우에도 두 모드 (`TruncateBefore...`, `Interrupted`) 모두 깨지지 않은 history 만 자식에 넘긴다.

---

## 4. Resume — `--resume`, `--last`, by id

CLI 인자 (`codex-rs/cli/src/main.rs:274-298`):

- `codex resume <SESSION_ID>` — UUID 또는 thread_name 으로 lookup. UUID 가 우선.
- `codex resume --last` — picker 없이 가장 최근 세션 자동 선택.
- `codex resume --all` — cwd 필터 해제, 전 세션 picker.

내부 진입점은 모두 `ThreadManager::resume_thread_from_rollout(config, rollout_path, auth_manager, parent_trace)` (`thread_manager.rs:642-687`) 로 수렴한다.

### 4.1 JSONL 재구성 흐름

1. `RolloutRecorder::get_rollout_history(&path).await` → 파일을 한 줄씩 읽어 `Vec<RolloutItem>` 으로 만든다.
2. 결과를 `InitialHistory::Resumed(ResumedHistory { conversation_id, history, rollout_path })` (`protocol.rs:2393-2405`) 로 감싼다. `conversation_id` 는 첫 줄 `SessionMeta.id` 에서 가져온 **원본 thread_id** 다 — fork 와 달리 같은 ID 를 그대로 쓴다.
3. `state.spawn_thread(..., InitialHistory::Resumed, ...)` 진입.
4. **Live thread 검사** (`spawn_thread_with_source`, `thread_manager.rs:1157-1178`):
   - 이미 매니저 안에 같은 `ThreadId` 가 등록돼 있고 `is_running()` 이면 → 그 thread 를 그대로 돌려준다 (`resume_active_thread_from_rollout_returns_running_thread` 테스트, `thread_manager_tests.rs:512-566`).
   - rollout_path 가 다르면 `CodexErr::InvalidRequest` (한 thread 가 두 rollout 으로 동시 활성화되는 것을 차단).
   - 등록돼 있지만 stopped 면 맵에서 제거 후 새 인스턴스 spawn (`resume_stopped_thread_from_rollout_spawns_new_thread` 테스트, `thread_manager_tests.rs:568-627`). `thread_id` 는 같지만 `Arc::ptr_eq` 는 false.
5. `Codex::spawn` → `Session::new` 가 `InitialHistory::Resumed` 의 history 를 받아 `ContextManager` 에 미리 채운다.
6. `apply_goal_resume_runtime_effects()` (`thread_manager.rs:1230-1234`, `codex_thread.rs:141-146`) 가 `GoalRuntimeEvent::ThreadResumed` 를 dispatch — paused goal 은 paused 상태를 유지한다 (`resumed_thread_keeps_paused_goal_paused` 테스트, `thread_manager_tests.rs:1267-`).

### 4.2 InitialHistory variants

`codex-rs/protocol/src/protocol.rs:2400-2406` 의 enum 이 그대로 의미를 정의한다.

| variant | 언제 | history 출처 | thread_id |
| --- | --- | --- | --- |
| `New` | `start_thread` | 없음 | 새 UUID |
| `Cleared` | `/clear` 류 | 없음 (하지만 컨텍스트는 의도적 삭제) | 동일 thread 유지 |
| `Resumed(ResumedHistory)` | `--resume` | JSONL 파일 전체 | 원본 thread_id 재사용 |
| `Forked(Vec<RolloutItem>)` | `--fork`, `/fork` | JSONL 파일의 prefix | 새 UUID, `forked_from_id` 로 부모 링크 |

`InitialHistory::get_resumed_thread_source()` 는 resume 일 때만 `ThreadSource::Resumed` 를 돌려준다 (`thread_manager.rs:621-622`). fork 의 경우 호출자가 명시적으로 `thread_source` 를 넘겨야 한다.

---

## 5. Cross-thread 자원

같은 프로세스의 모든 thread 가 공유하는 것 / thread 마다 독립적인 것을 구분한다.

### 5.1 공유 자원

- **`state_db: StateDbHandle`** — `~/.codex/state_5.sqlite` (`codex-rs/state/src/lib.rs:66-67`, `STATE_DB_FILENAME = "state"`, `STATE_DB_VERSION = 5`). 한 connection pool 을 모든 thread 가 공유. dynamic_tools 캐시, agent graph edges, 메모리, 로그 인덱스 등이 여기 산다. 자세한 테이블은 `multi-turn-and-storage-2026-05-08.md`.
- **`thread_store: Arc<dyn ThreadStore>`** — `LocalThreadStore` (rollout JSONL + state DB) / `RemoteThreadStore` (gRPC) / `InMemoryThreadStore` (테스트). 디스패치는 `thread_store_from_config` (`thread_manager.rs:276-285`) 가 `config.experimental_thread_store` 로 결정한다. `ThreadStore` trait API 는 create / resume / append / persist / flush / shutdown / load_history / read_thread / list_threads / update_metadata / archive 로 구성 (`thread-store/src/store.rs:21-84`).
- **`agent_graph_store: Arc<dyn AgentGraphStore>`** — parent/child thread spawn edge 의 영속화 (`agent-graph-store/src/local.rs`). `list_thread_spawn_descendants(root_id, status_filter)` (`thread_manager.rs:548-555`) 로 한 root 의 전체 sub-agent 트리를 조회.
- **MCP / skill / plugin 캐시** — `McpManager`, `SkillsManager`, `PluginsManager` 모두 `Arc` 1 개. `skills_watcher` 가 파일 변경을 감지하면 `skills_manager.clear_cache()` 한 번으로 모든 thread 가 다음 turn 부터 fresh 하게 본다.

### 5.2 thread 별 자원

- 각자의 `Session` / `Codex` / `submission_loop` tokio task.
- 각자의 `ContextManager` (in-memory history) — 다른 thread 와 absolutely 공유하지 않는다.
- 각자의 rollout JSONL — fork 든 resume 이든 새 thread 는 새 파일.
- 각자의 `RealtimeConversationManager` (audio / streaming).

### 5.3 parent / child agent edges

multi-agent (sub-agent) 구조에서:

- 부모 thread 가 `agent` 도구 등으로 자식 thread 를 spawn 하면 `SessionSource::SubAgent(SubAgentSource::ThreadSpawn { parent_thread_id, depth, .. })` 로 시작한다.
- `ThreadManagerState::parent_rollout_thread_trace_for_source` (`thread_manager.rs:1287-1315`) 가 부모의 rollout trace context 를 자식에게 inject — 부모와 자식의 trace 가 같은 트리에 묶인다.
- `agent_graph_store.upsert_thread_spawn_edge(parent, child, status)` 가 `state_5.sqlite` 에 edge 를 기록 (`agent-graph-store/src/local.rs:36-45`).
- 부모 입장에서 자식 트리를 보고 싶을 때 `ThreadManager::list_agent_subtree_thread_ids(root)` 가 (a) graph store 의 영속 descendants + (b) 메모리에 살아있는 `agent_control` 의 live descendants 를 union 으로 반환 (`thread_manager.rs:537-576`).
- 자식의 `Session::send_op` 는 부모 매니저와 동일한 `ThreadManager` 인스턴스를 통해 dispatch 된다 — `AgentControl` (`thread_manager.rs:885-887`) 이 매니저 state 의 weak ref 를 들고 있어 `Arc` cycle 없이 형제 thread 에 메시지를 보낸다.

---

## 6. 테스트 contract — `core/src/thread_manager_tests.rs`

테스트가 강제하는 핵심 invariant. 새 코드가 이 boundary 를 깨지 않는지 확인할 때 참조.

- **truncation 정책** — `truncates_before_requested_user_message`, `out_of_range_truncation_drops_only_unfinished_suffix_mid_turn`, `out_of_range_truncation_drops_pre_user_active_turn_prefix`, `ignores_session_prefix_messages_when_truncating`. 핵심: n 번째 user message 직전 cut, mid-turn out-of-range 는 active turn 시작점 cut, session prefix (cwd/env 메시지 등) 는 user-message 카운팅에서 무시한다.
- **legacy callsite** — `fork_thread_accepts_legacy_usize_snapshot_argument`. `usize → ForkSnapshot` `From` 이 살아있어야 한다.
- **shutdown 일관성** — `shutdown_all_threads_bounded_submits_shutdown_to_every_thread`. 모든 thread 가 `completed` 에 들어가고 `list_thread_ids().is_empty()` 가 보장된다.
- **internal thread 숨김** — `start_thread_keeps_internal_threads_hidden_from_normal_lookups`. `session_source.is_internal()` 인 thread 는 `list_thread_ids` / `get_thread` 에서 노출되지 않는다 (`ThreadManagerState::list_thread_ids`, `get_thread`, `thread_manager.rs:908-926`).
- **resume idempotency** — `resume_active_thread_from_rollout_returns_running_thread` vs `resume_stopped_thread_from_rollout_spawns_new_thread`. live 면 같은 `Arc`, stopped 면 새 인스턴스. 두 경우 모두 `thread_id` 는 동일.
- **resume 시 thread_source 보존** — `resume_stopped_thread_from_rollout_preserves_thread_source`.
- **resume 시 rollout-stored env 무시** — `resume_and_fork_do_not_restore_thread_environments_from_rollout`. 환경은 항상 호출자가 새로 결정한다 (보안 / cwd 일관성).
- **interrupted fork 의 marker 합성** — `interrupted_fork_snapshot_appends_interrupt_boundary` 외 5 개 테스트가 `<turn_aborted>` 마커, contextual user vs developer 마커, 명시적 turn_id 보존, legacy history 호환을 모두 검증.
- **goal resume** — `resumed_thread_keeps_paused_goal_paused`. `apply_goal_resume_runtime_effects` 가 paused goal 을 자동으로 풀지 않는다.
- **explicit environment selection** — `start_thread_accepts_explicit_environment_when_default_environment_is_disabled`. default env 가 꺼진 상태에서도 호출자가 명시 selection 을 넘기면 spawn 이 성공해야 한다.

테스트는 `with_models_provider_and_home_for_tests` (`thread_manager.rs:389-408`) 또는 `state_backed_stores` 헬퍼 (`thread_manager_tests.rs:50-63`) 로 매번 임시 codex_home 을 만들어 격리된다 — process env 를 건드리지 않는다 (CLAUDE.md 규약).

---

## 7. 상호 참조

- **디스크 / 영속 측면** — `fork-docs/multi-turn-and-storage-2026-05-08.md`. JSONL 포맷, `state_5.sqlite` 테이블, `~/.codex/sessions/...` 경로 규칙, 백업 / 삭제 절차.
- **단일 turn 의 inner loop** — `02-turn-lifecycle.md`. `Session::run_turn`, `ContextManager.for_prompt`, SSE 수신.
- **wire 변환** — `03-wire-protocol.md`. `developer→user` mapping, chat completions 번역.
- **외부 노출 API** — `app-server-protocol/src/protocol/v2.rs` 의 thread/* RPC. `ThreadManager` 가 이 RPC 의 backend.
- **단독 사용 예제** — `codex-rs/thread-manager-sample/src/main.rs`. `ThreadManager::new` → `start_thread` → 한 turn → 종료까지의 minimal driver. `codex-core` 를 직접 임베드할 때 reference.
