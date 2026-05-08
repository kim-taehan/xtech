# 02 — Turn 라이프사이클

이 문서는 사용자가 `xtech` (TUI 또는 `xtech exec "..."`) 에 한 줄을 입력해 LLM 응답을 받기까지, 코드가 어떤 경로로 흐르는지 단계별로 정리한다. 각 단계는 file:line 인용을 달아두었으니 읽으면서 source 를 따라가도 된다. 보조 자료로는 저장 측면을 다룬 `fork-docs/multi-turn-and-storage-2026-05-08.md`, chat completions 분기 도입 경위를 정리한 `fork-docs/work-log-2026-05-08.md`, 그리고 lifecycle contract 를 가장 잘 보여주는 통합 테스트 `codex-rs/core/tests/suite/client.rs:354-410` (resume 후 한 turn 송신) 와 `codex-rs/core/tests/suite/compact.rs` 를 참고하라.

---

## 0. 라이프사이클 한눈에

```
사용자 입력 (TUI keypress / CLI argv)
   │
   ▼
[1] Op enum 으로 직렬화  (Op::UserTurn / Op::UserInput / Op::UserInputWithTurnContext)
   │   exec: AppCommand → JSON-RPC TurnStart → submit_core_op
   │   tui : AppCommand::UserTurn → submit_op → app-server JSON-RPC TurnStart
   │
   ▼
[2] Codex::submit() → tx_sub (async_channel) → submission_loop
   │
   ▼
[3] handlers::user_input_or_turn_inner → new_turn_with_sub_id → spawn_task(RegularTask)
   │
   ▼
[4] tasks::start_task: ActiveTurn 등록 → tokio::spawn → RegularTask::run
   │
   ▼
[5] run_turn 루프
   │   - record_user_prompt_and_emit_turn_item (history + rollout + UserMessage 이벤트)
   │   - run_pre_sampling_compact
   │   - loop:
   │       - clone_history().for_prompt() → Vec<ResponseItem>
   │       - run_sampling_request → try_run_sampling_request
   │       - client_session.stream(&prompt) ─► [6]
   │       - SSE 이벤트 처리 (OutputItemAdded/Delta/OutputItemDone/Completed)
   │       - tool 호출은 in_flight FuturesOrdered 에 적재 → drain_in_flight 가 record_conversation_items
   │       - needs_follow_up 이면 다음 sampling 라운드, 아니면 hooks 마무리 후 break
   │
   ▼
[6] ModelClient::stream() → WireApi 분기
   │   WireApi::Chat      → stream_chat_completions_api → ChatRequestBuilder → ApiChatClient
   │   WireApi::Responses → stream_responses_websocket / stream_responses_api
   │
   ▼
[7] SSE chunk → ResponseEvent (codex-api/src/sse/chat.rs::process_chat_sse)
   │   tool_calls → ResponseItem::FunctionCall, finish_reason → ResponseEvent::Completed
   │
   ▼
[8] on_task_finished → EventMsg::TurnComplete + flush_rollout + active_turn 정리
```

---

## 1. Input → Op

### 1.1 TUI 경로

사용자가 composer 에 글을 치고 Enter 를 누르면 `chatwidget.rs::submit_user_message` (`codex-rs/tui/src/chatwidget.rs:5585`) → `submit_user_message_with_history_and_shell_escape_policy` (`chatwidget.rs:5618`) 가 호출된다. 이 안에서 텍스트 / 이미지 / mention 들을 `Vec<UserInput>` (`codex-rs/protocol/src/user_input.rs`) 로 빌드한 뒤 `AppCommand::user_turn(...)` (`codex-rs/tui/src/app_command.rs:149-176`) 로 감싼다.

```rust
let op = AppCommand::user_turn(
    items, cwd, approval_policy, permission_profile,
    model, effort, summary, service_tier,
    final_output_json_schema, collaboration_mode, personality,
);
self.submit_op(op.clone());           // chatwidget.rs:5870
```

`submit_op` 는 TUI 의 in-process app-server 에 JSON-RPC `turn/start` 를 던진다 (`codex-rs/tui/src/app/thread_routing.rs:506`, `try_submit_active_thread_op_via_app_server`). 이미 활성 turn 이 있으면 `turn_steer` 로 보내 in-flight queue 에 합류시킨다 (`thread_routing.rs:521-583`).

### 1.2 exec 경로

`codex exec "say pong"` 도 결국 같은 `Op` 표면을 친다. `codex-rs/exec/src/lib.rs:600-650` 에서 prompt argv 를 `UserInput::Text { text, text_elements: Vec::new() }` 로 만들어 `InitialOperation::UserTurn` (`exec/src/lib.rs:157-165`) 에 담는다. 이후 `exec/src/lib.rs:752-786` 에서 in-process app-server 클라이언트로 `ClientRequest::TurnStart { params: TurnStartParams { thread_id, input, ... } }` JSON-RPC 를 보낸다. 즉 TUI 와 exec 모두 동일한 v2 API (`thread_id` + `TurnStartParams`) 를 거친다 — 이 경계가 IDE / 외부 클라이언트와도 공유된다.

### 1.3 Op variants

`codex-rs/protocol/src/protocol.rs:407-560` 에 정의된 `Op` enum 중 turn 을 시작시키는 variant 는 셋이다.

- `Op::UserInput` (`protocol.rs:435`) — 가장 단순. items 만 새로 보내고 turn-context override 없음. legacy 경로.
- `Op::UserInputWithTurnContext` (`protocol.rs:452`) — items 와 함께 cwd/approval/sandbox/model/etc. override 를 같은 큐 항목으로 묶어 보낸다. override 가 거부되면 입력도 시작되지 않는다.
- `Op::UserTurn` (`protocol.rs:530`) — 가장 풍성한 variant. cwd, approval_policy, sandbox_policy 등이 **모두 필수**. TUI / exec 가 실제로 사용하는 경로다.

app-server 의 `turn/start` 핸들러는 이 셋 중 하나를 골라 채운다 (`codex-rs/app-server/src/request_processors/turn_processor.rs:449-477`). override 가 하나라도 있으면 `Op::UserInputWithTurnContext`, 없으면 `Op::UserInput` 으로 직렬화된다 (TUI 의 풀 turn 은 override 가 항상 있기 때문에 사실상 `UserInputWithTurnContext` 로 들어온다).

---

## 2. Submission → Session

### 2.1 `Codex::submit` 와 `tx_sub` 채널

app-server 가 `submit_core_op` (`turn_processor.rs:281-290`) 에서 `thread.submit_with_trace(op, trace)` 를 호출한다. `CodexThread::submit_with_trace` 는 `Codex::submit_with_trace` (`codex-rs/core/src/session/mod.rs:680-693`) 로 위임되고, 여기서 `Submission { id, op, trace }` 를 만들어 `tx_sub.send(sub)` (`session/mod.rs:697-705`) 로 channel 에 적재한다.

채널은 `bounded(SUBMISSION_CHANNEL_CAPACITY)` 로 spawn 시 생성된다 (`session/mod.rs:473`). 수신측은 startup 에서 띄운 `submission_loop` task (`session/mod.rs:658-663`).

### 2.2 `submission_loop`

`codex-rs/core/src/session/handlers.rs:785-993` 의 거대한 match. `Op` 의 모든 variant 를 분기 처리한다. turn 시작 variant (`UserInput` / `UserInputWithTurnContext` / `UserTurn`) 는 한 군데에서 fall-through 된다 (`handlers.rs:879-884`):

```rust
Op::UserInput { .. }
| Op::UserInputWithTurnContext { .. }
| Op::UserTurn { .. } => {
    user_input_or_turn(&sess, sub.id.clone(), sub.op).await;
    false  // not exit
}
```

interrupt / approval / compact / shutdown 등은 여기서 직접 분기되지만, 본 문서가 추적하는 "한 턴" 의 본류는 `user_input_or_turn` 으로 빠진다.

### 2.3 `user_input_or_turn_inner` — 진입점

`handlers.rs:111-283`. 세 단계로 정리된다.

1. Op 별로 `(items, SessionSettingsUpdate, responsesapi_client_metadata)` 튜플 추출 (`handlers.rs:117-232`). `UserTurn` 은 cwd/approval/sandbox 가 `Some(...)` 으로 강제 채워지고, `UserInput` 은 `Default::default()` 로 비워진다.
2. `sess.new_turn_with_sub_id(sub_id, updates)` 로 이번 turn 의 `Arc<TurnContext>` 를 만든다 (`handlers.rs:235`). 이게 turn-scoped read-only snapshot 이다 (자세한 필드는 §3 참고).
3. `sess.steer_input(...)` 결과로 활성 turn 이 없으면 (`SteerInputError::NoActiveTurn`) `sess.spawn_task(turn_context, items, RegularTask::new())` 로 새 task 를 띄운다 (`handlers.rs:263-268`). 활성 turn 이 있으면 steer 큐에 합류 (in-flight 한복판에 들어가는 추가 user message).

`spawn_task` 는 `codex-rs/core/src/tasks/mod.rs:292-301` 에서 정의되어, 기존 task 를 `TurnAbortReason::Replaced` 로 정리한 뒤 `start_task` (`tasks/mod.rs:303-431`) 로 위임한다. `start_task` 는 `ActiveTurn` 슬롯을 잡고 `tokio::spawn` 으로 `RegularTask::run` 을 띄운 뒤 `RunningTask` 를 등록한다.

---

## 3. Turn 구성: TurnContext, History, Prompt

### 3.1 `TurnContext`

`codex-rs/core/src/session/turn_context.rs` 의 read-only snapshot. 한 turn 동안 고정되는 모든 정보 — `model_info`, `sub_id`, `cwd`, `approval_policy`, `sandbox_policy`, `truncation_policy`, `personality`, `collaboration_mode`, `final_output_json_schema`, `turn_metadata_state`, `turn_timing_state`, `turn_skills` 등 — 가 한 구조체에 모인다. 다음 턴이 시작되면 새 `TurnContext` 가 만들어지고 이전 것은 `Drop` 된다.

### 3.2 `ContextManager` (= history)

`codex-rs/core/src/context_manager/history.rs:33-71`. 핵심 필드는 `items: Vec<ResponseItem>` 으로, 가장 오래된 ResponseItem 이 앞에 온다. 사용자 메시지(`role=user`/`developer`), assistant 메시지, reasoning 블록, FunctionCall, FunctionCallOutput, custom tool 호출, web search, image gen, compaction marker 까지 모두 같은 vector 에 누적된다.

`ContextManager::record_items` (`history.rs:99-113`) 는 `is_api_message` 필터를 통과한 항목만 push 하고 turn-context 의 truncation_policy 를 적용한다. 모델에 보낼 슬라이스를 만들 때는 `for_prompt(input_modalities)` (`history.rs:119-122`) 가 호출되어 `normalize_history` 가 dangling tool_call 보정과 image stripping 을 수행한다 (`codex-rs/core/src/context_manager/normalize.rs`).

### 3.3 turn 시작 시 history 갱신

`run_turn` (`codex-rs/core/src/session/turn.rs:137-661`) 의 진입부에서 다음 일이 순차적으로 일어난다.

1. `run_pre_sampling_compact` (`turn.rs:156-166`) — 누적 토큰이 `auto_compact_token_limit` 을 초과하면 turn 시작 *전에* compact.
2. `record_context_updates_and_set_reference_context_item` (`turn.rs:170`) — cwd / environments / plugins / skills / collaboration mode 등이 직전 turn 대비 바뀌었으면 그 diff 를 ContextualUserFragment 로 합성하여 history 에 push.
3. skill / plugin injection items 합성 (`turn.rs:269-356`).
4. `record_user_prompt_and_emit_turn_item` (`turn.rs:325`, body in `session/mod.rs:2933-2948`) — 사용자 입력을 `ResponseItem::Message { role: "user", ... }` 로 history + rollout 에 commit, 그리고 `UserMessage` turn item event 를 송신.

### 3.4 `Prompt` 구성

매 sampling 루프 iteration 마다 `clone_history().for_prompt(...)` 으로 `Vec<ResponseItem>` 을 새로 떠 와서 `build_prompt` (`turn.rs:965-982`) 에 넘긴다.

```rust
Prompt {
    input,                                                  // for_prompt 결과
    tools: router.model_visible_specs(),                    // 활성 tool spec
    parallel_tool_calls: turn_context.model_info.supports_parallel_tool_calls,
    base_instructions,                                      // sess.get_base_instructions().await
    personality: turn_context.personality,
    output_schema: turn_context.final_output_json_schema.clone(),
    output_schema_strict: !is_guardian_reviewer_source(...),
}
```

이 `Prompt` 가 wire 변환의 입력이다.

---

## 4. Wire 호출: `ModelClient::stream`

### 4.1 진입

`run_sampling_request` (`turn.rs:993-1132`) 의 retry 루프 안에서 `try_run_sampling_request` (`turn.rs:1831-1867`) 가 `client_session.stream(&prompt, &model_info, ...)` 를 호출한다. `ModelClientSession::stream` 의 본체는 `codex-rs/core/src/client.rs:1496-1556`.

```rust
let wire_api = self.client.state.provider.info().wire_api;
match wire_api {
    WireApi::Responses => {
        // websocket 우선, fallback 으로 HTTP responses API
        ...
        self.stream_responses_api(...).await
    }
    WireApi::Chat => {
        self.stream_chat_completions_api(prompt, model_info, ...).await
    }
}
```

`WireApi::Chat` arm 은 본 fork 가 부활시킨 분기다. 정의는 `codex-rs/model-provider-info/src/lib.rs:47-82` 의 `WireApi` enum (`"chat"` deserializer 복원), 빌트인 ollama provider 가 `Chat` 으로 박혀 들어오는 것은 `model-provider-info/src/lib.rs:415-425`. fork 의 default provider 가 ollama 인 것은 `codex-rs/core/src/config/mod.rs:2634` 에서 정해진다.

### 4.2 `stream_chat_completions_api` 구현

`client.rs:1566-1662`. 핵심 단계:

1. `output_schema` 가 있으면 거부 (`client.rs:1573-1577`) — Chat Completions 에 등가물 없음.
2. `prompt.get_formatted_input()` 으로 ResponseItem vec 추출 (`client.rs:1586`), `create_tools_json_for_chat_completions_api(&prompt.tools)` 로 tools JSON 모양 변환 (`codex-rs/tools/src/tool_spec.rs:174-200`).
3. `ChatRequestBuilder::new(model_slug, instructions, input_items, tools_json)` (`codex-rs/codex-api/src/requests/chat.rs:32-46`) 빌드. 여기서:
   - 첫 메시지는 `{"role":"system","content": instructions}` 1개 (`requests/chat.rs:60`).
   - ResponseItem 의 `role=="developer"` 는 wire 에서 `"user"` 로 매핑 (`requests/chat.rs:198-199`). fork 의 의도적 변경 (배경: `fork-docs/work-log-2026-05-08.md` §2.5).
   - `assistant` 메시지의 reasoning 은 같은 메시지의 별도 `reasoning` 필드로 attach (`requests/chat.rs:200-205`).
   - `FunctionCall` + `FunctionCallOutput` 은 `push_tool_call_message` (`requests/chat.rs:324-360`) 를 거쳐 `assistant(tool_calls=[...]) → tool(...)` 의 chat completions 표준 정렬로 묶인다.
4. `ApiChatClient::new(transport, provider, auth).stream_request(chat_request)` 호출 (`client.rs:1616-1618`). 본체는 `codex-rs/codex-api/src/endpoint/chat.rs:61-79` 의 `ChatClient::stream_request` — POST `chat/completions` 에 `Accept: text/event-stream` 을 박고 `spawn_chat_stream` 으로 SSE task 를 띄운 뒤 `ResponseStream { rx_event, upstream_request_id }` 를 반환한다.

`WireApi::Responses` 쪽은 `stream_responses_api` (`client.rs:1150`) / `stream_responses_websocket` (`client.rs:1275`) 에 살고, fork 운영 환경에서는 사실상 사용되지 않지만 lmstudio provider 등을 위해 형식상 유지된다.

---

## 5. 응답 스트림 처리: SSE → ResponseEvent → UI event

### 5.1 SSE 파서

`codex-rs/codex-api/src/sse/chat.rs::process_chat_sse` (`sse/chat.rs:59-330+`). 백그라운드 tokio task 가 SSE 이벤트를 하나씩 파싱하여 `mpsc::Sender<Result<ResponseEvent, ApiError>>` 에 push 한다. 핵심 동작:

- `data: [DONE]` 또는 `data: DONE` 센티넬이 오면 누적된 reasoning/assistant item 을 flush 하고 `ResponseEvent::Completed { response_id: "", token_usage: None, end_turn: None }` 로 마감 (`sse/chat.rs:86-110, 146-150`).
- 매 chunk 의 `delta.content` 는 assistant text 누적, `delta.reasoning` 은 reasoning 누적, `delta.tool_calls` 는 tool_call index 별로 함수명/인자를 stitching (`sse/chat.rs:168-253`).
- `finish_reason == "stop"` → 누적된 reasoning/assistant 를 `ResponseEvent::OutputItemDone` 으로 보내고 `Completed` (`sse/chat.rs:268-291`).
- `finish_reason == "tool_calls"` → 누적된 tool_call 들을 각각 `ResponseItem::FunctionCall { id, name, arguments, call_id, namespace: None }` 로 만들어 `OutputItemDone` 으로 송신 (`sse/chat.rs:299-329`).
- `finish_reason == "length"` → `ApiError::ContextWindowExceeded` (`sse/chat.rs:294-296`).

이 모든 이벤트 타입은 `codex-rs/codex-api/src/common.rs:67-107` 의 `pub enum ResponseEvent` 에 정의되어 있다 (`Created`, `OutputItemAdded`, `OutputItemDone`, `OutputTextDelta`, `ToolCallInputDelta`, `ReasoningSummaryDelta`, `Completed`, `RateLimits`, `ServerModel`, `ModelVerifications`, ...).

### 5.2 core 의 stream consumer

`try_run_sampling_request` (`turn.rs:1883-2227`) 의 거대한 `loop` 가 `stream.next()` 를 한 이벤트씩 받아 처리한다. variants 별 처리:

- `ResponseEvent::OutputItemAdded(item)` (`turn.rs:2009-2074`) — 새 item 의 메타데이터를 `TurnItem` 으로 변환하여 `emit_turn_item_started` 로 UI 에 송신, `active_item` 에 저장.
- `ResponseEvent::OutputTextDelta(delta)` (`turn.rs:2132-2160`) — assistant text 의 incremental delta 를 `AgentMessageContentDelta` event 로 UI 에 흘림.
- `ResponseEvent::OutputItemDone(item)` (`turn.rs:1926-2008`) — 한 item 완성. `handle_output_item_done` (`stream_events_utils.rs:220+`) 가 호출되어, item 이 `FunctionCall` 류이면 tool dispatcher 의 future 를 빌드해 `in_flight: FuturesOrdered<...>` 에 push 한다 (`turn.rs:1994-1996`). 일반 메시지/reasoning 은 history 에 직접 record.
- `ResponseEvent::ToolCallInputDelta` / `ReasoningSummaryDelta` / `ReasoningContentDelta` (`turn.rs:2161-2226`) — argument-diff consumer 와 reasoning UI event 변환.
- `ResponseEvent::Completed { token_usage, end_turn, .. }` (`turn.rs:2109-2131`) — 누적된 assistant text segment 를 flush, `update_token_usage_info` 로 토큰 카운터 갱신, `end_turn=Some(false)` 면 `needs_follow_up=true`. 이 시점이 한 sampling 라운드의 끝이고 loop 는 `break`.

루프가 break 한 후 `drain_in_flight(&mut in_flight, sess, turn_context)` (`turn.rs:1797-1821, 호출 2238`) 가 보류된 tool future 들을 모두 await 하여 각각의 `FunctionCallOutput` 을 `record_conversation_items` (`session/mod.rs:2391-2399`) 로 history + rollout 에 commit 한다.

### 5.3 UI event 송신

`Session::send_event(turn_context, EventMsg)` (`session/mod.rs:1479+`) 가 모든 UI event 의 단일 출구다. 송신 전에 `EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_)` 가 아니면 일부 메타데이터를 추가하고, 뒤이어 rollout 도 함께 persist 한다 (`session/mod.rs:1519-1630`). 즉 동일 `EventMsg` 하나가 (a) UI 채널 (`rx_event`), (b) JSONL rollout 양쪽으로 동시에 흐른다.

---

## 6. Tool call 라운드: 다음 sampling 으로 이어가기

`ResponseEvent::Completed` 가 `end_turn=Some(false)` 였거나, `OutputItemDone` 으로 받은 item 중 하나라도 `FunctionCall` 이었으면 `needs_follow_up=true` 로 마킹된다 (`turn.rs:2000`, `turn.rs:2124-2126`). 이 상태로 `try_run_sampling_request` 가 정상 반환되면 `run_turn` 의 outer loop (`turn.rs:376-658`) 가 한 라운드 더 돈다.

다음 라운드에서:

1. `sess.get_pending_input()` 으로 in-flight 사이 사용자 추가 입력을 drain (`turn.rs:384-388`). 정상 첫 라운드는 새 prompt 가 우선이라 drain 하지 않지만, 두 번째 라운드부터는 drain 된 pending input 도 함께 송신된다.
2. `clone_history().for_prompt(...)` 로 history 를 다시 떠 온다 (`turn.rs:431-435`). 직전 라운드에서 `record_conversation_items` 로 history 에 들어간 `FunctionCall` + `FunctionCallOutput` 페어가 자동으로 다음 wire payload 에 포함된다.
3. `run_sampling_request` 가 retry 루프 째로 다시 호출된다 (`turn.rs:446-458`).
4. 모델이 `FunctionCallOutput` 을 보고 또 tool 을 부르면 같은 패턴으로 라운드가 한 번 더, 최종 assistant text 만 보내면 `Completed { end_turn: Some(true) }` (또는 `end_turn=None` + 더 부를 게 없음) 로 needs_follow_up 이 false 가 되어 outer loop 가 break 한다.

자동 compact 트리거도 이 outer loop 안에서 일어난다: `total_usage_tokens >= auto_compact_limit && needs_follow_up` 이면 `run_auto_compact(...)` 가 mid-turn 으로 한 번 돌고 (`turn.rs:486-505`), 그 후 같은 라운드를 재시도한다.

테스트 reference 로 `core/tests/suite/client.rs:735-790` (function_call → output → second turn) 와 `core/tests/suite/compact.rs` 의 mid-turn compact 시나리오를 보면 이 라운드 모델이 깔끔하게 격리되어 있다.

---

## 7. Turn 종료

### 7.1 `run_turn` 종료

outer loop 가 `break` 하는 경로는 셋이다 (`turn.rs:563, 622-623, 627-655`).

- 정상: needs_follow_up=false + stop hook block 없음 + after_agent hook 정상.
- abort: `CodexErr::TurnAborted` (cancellation token 발화, `turn.rs:627-630`).
- 에러: 그 외 `CodexErr` (메시지 송신 후 break, `turn.rs:650-655`).

이때 `run_turn` 은 `Option<String>` (last_agent_message) 을 반환한다.

### 7.2 `RegularTask::run` 의 마무리

`codex-rs/core/src/tasks/regular.rs:71-86` 의 inner loop 는 `run_turn` 결과를 받고, `sess.has_pending_input()` 이 true 면 빈 input 으로 한 번 더 turn 을 돈다 (steer 로 들어온 후속 사용자 입력 처리). 더 이상 pending 이 없으면 `last_agent_message` 를 그대로 return.

### 7.3 `on_task_finished`

`tasks/mod.rs:556-784`. 이 함수가 turn 의 진짜 종료점이다.

1. `active_turn` 에서 자기 task 제거, `pending_input` 회수 (`tasks/mod.rs:571-594`).
2. 회수된 pending_input 을 `record_pending_input` 으로 history 에 추가 (`tasks/mod.rs:595-608`) — 다음 turn 시작 시점에 이미 history 에 있는 상태가 된다.
3. token usage / network proxy / memory 등 turn 단위 metric/analytics 발행 (`tasks/mod.rs:609-727`).
4. **`EventMsg::TurnComplete(TurnCompleteEvent { turn_id, last_agent_message, completed_at, duration_ms, time_to_first_token_ms })` 송신** (`tasks/mod.rs:745-752`). 이 이벤트가 UI / exec / 통합 테스트가 "한 턴이 끝났다" 고 인지하는 신호다 (테스트는 `wait_for_event(&codex, |ev| matches!(ev, EventMsg::TurnComplete(_)))` 로 기다린다 — `core/tests/suite/client.rs:397, 762, 963` 등).
5. Guardian rejection circuit breaker 의 turn 컨텍스트 정리 (`tasks/mod.rs:753-757`).
6. active_turn 슬롯 비우기, 필요시 `MaybeContinueIfIdle` goal-runtime 이벤트 발행 (`tasks/mod.rs:759-783`).

### 7.4 영속화

`spawn_task` 의 tokio::spawn 본문 (`tasks/mod.rs:384-416`) 에서 `task_for_run.run(...)` 직후 **`sess.flush_rollout()` 가 호출된 뒤** `on_task_finished` 가 호출된다 (`tasks/mod.rs:395-411`). 즉 `TurnComplete` 가 송신되는 시점에 이번 turn 동안 누적된 ResponseItem / EventMsg 가 모두 `~/.codex/sessions/.../rollout-*.jsonl` 에 commit 되어 있는 것이 보장된다. 이 보장이 `codex resume <thread_id>` 의 무손실 복원을 가능케 한다 — rollout 측 자세한 내용은 `fork-docs/multi-turn-and-storage-2026-05-08.md` §3.1 / §4.1 참고.

### 7.5 abort / interrupt 경로

`Op::Interrupt` 가 들어오면 `submission_loop` 가 `interrupt(&sess)` 호출 → `abort_all_tasks(TurnAbortReason::Interrupted)` (`tasks/mod.rs:475+`) → 각 RunningTask 의 cancellation_token cancel → `select! { _ = done.notified() => ..., _ = sleep(GRACEFULL_INTERRUPTION_TIMEOUT_MS) => ... }` 후 강제 abort (`tasks/mod.rs:798-820`). 이 경로는 `EventMsg::TurnAborted(_)` 를 별도로 송신하고, `flush_rollout` 도 함께 수행하므로 미완료 turn 의 prefix 까지는 디스크에 남는다.

---

## 8. 정리: 한 turn 에 등장하는 핵심 파일

| 단계 | 파일:라인 | 역할 |
| --- | --- | --- |
| Op 빌드 (TUI) | `codex-rs/tui/src/chatwidget.rs:5856-5872` | `AppCommand::user_turn` + `submit_op` |
| Op 빌드 (exec) | `codex-rs/exec/src/lib.rs:617-650` | `UserInput::Text` → `InitialOperation::UserTurn` |
| Op enum | `codex-rs/protocol/src/protocol.rs:407-560` | `Op::UserInput` / `Op::UserInputWithTurnContext` / `Op::UserTurn` |
| 채널 적재 | `codex-rs/core/src/session/mod.rs:680-705` | `Codex::submit` → `tx_sub` |
| 디스패치 | `codex-rs/core/src/session/handlers.rs:785-993` | `submission_loop` 메인 루프 |
| Turn 진입 | `codex-rs/core/src/session/handlers.rs:111-283` | `user_input_or_turn_inner` |
| Task 시작 | `codex-rs/core/src/tasks/mod.rs:292-431` | `spawn_task` / `start_task` |
| Run loop | `codex-rs/core/src/session/turn.rs:137-661` | `run_turn` (outer 라운드 loop) |
| Sampling | `codex-rs/core/src/session/turn.rs:1831-2256` | `try_run_sampling_request` (SSE consumer) |
| Wire 분기 | `codex-rs/core/src/client.rs:1496-1556` | `WireApi::Chat` vs `WireApi::Responses` |
| Chat 빌더 | `codex-rs/codex-api/src/requests/chat.rs:32-322` | `ChatRequestBuilder::build` (dev→user 매핑) |
| Chat 엔드포인트 | `codex-rs/codex-api/src/endpoint/chat.rs:61-79` | `ChatClient::stream_request` |
| SSE 파서 | `codex-rs/codex-api/src/sse/chat.rs:59-330+` | `process_chat_sse` |
| ResponseEvent | `codex-rs/codex-api/src/common.rs:67-107` | event variants |
| History 누적 | `codex-rs/core/src/context_manager/history.rs:99-122` | `record_items` / `for_prompt` |
| Tool drain | `codex-rs/core/src/session/turn.rs:1797-1821` | `drain_in_flight` |
| 종료 | `codex-rs/core/src/tasks/mod.rs:556-784` | `on_task_finished` (`TurnComplete` 송신) |
| Rollout flush | `codex-rs/core/src/tasks/mod.rs:395-411` | `flush_rollout` 직후 종료 이벤트 |

테스트 contract reference: `codex-rs/core/tests/suite/client.rs:354-410, 735-790`, `codex-rs/core/tests/suite/compact.rs` 의 `mount_sse_once` + `wait_for_event(EventMsg::TurnComplete)` 패턴이 한 턴의 lifecycle 전체를 한 함수에서 관찰하는 가장 짧은 예시다.
