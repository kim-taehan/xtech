# Codex Fork — 스트리밍 / SSE 레이어 (04-streaming)

> 모델 서버에서 흘러나오는 chunk 한 덩어리가 어떻게 `bytes` 로 잘리고, SSE 라인으로 풀리고, `ResponseEvent` enum 으로 normalize 되어, 결국 TUI/exec 화면의 텍스트 한 글자가 되는지 — 한 번의 turn 이 "stream" 되는 풀 패스를 따라간다. 두 개의 wire 포맷(Responses vs Chat) 이 공존하므로 분기점도 같이 정리한다.

주 파일:
- `codex-rs/codex-api/src/sse/mod.rs` — re-export
- `codex-rs/codex-api/src/sse/responses.rs` — typed Responses SSE 파서
- `codex-rs/codex-api/src/sse/chat.rs` — OpenAI Chat Completions SSE 파서 (fork 가 부활)
- `codex-rs/codex-api/src/common.rs` — `ResponseEvent` enum, `ResponseStream`
- `codex-rs/codex-api/src/endpoint/responses.rs`, `endpoint/chat.rs` — HTTP→SSE 진입점
- `codex-rs/core/src/session/turn.rs` — `ResponseEvent` 소비, EventMsg 변환
- `codex-rs/core/tests/common/responses.rs` — `mount_sse_once`, `ev_*` 헬퍼
- `codex-rs/core/tests/common/streaming_sse.rs` — gated chunk TCP 서버 (idle/timeout 검증)
- `codex-rs/core/tests/chat_completions_sse.rs` — fork 가 복원했지만 현재 `#![cfg(any())]` 로 비활성

---

## 1. SSE 기본 흐름 (chunked transfer → typed event)

서버 → 클라이언트 흐름은 4단계 파이프라인이다.

1. **HTTP chunked transfer**. 모델 서버는 `content-type: text/event-stream`, `Transfer-Encoding: chunked` 로 응답 시작. 한 chunk 는 SSE 한 frame 일 수도, 여러 frame 일 수도, frame 의 일부일 수도 있다 (네트워크 보장 없음).
2. **`reqwest::bytes_stream`** (`codex-client::ByteStream` 으로 alias). `Stream<Item = Result<Bytes, TransportError>>` — 단순히 raw byte 청크의 비동기 stream. `endpoint::responses::ResponsesClient::stream_request` / `endpoint::chat::ChatClient::stream_request` 가 만든 `StreamResponse { status, headers, bytes }` 안에 들어있다.
3. **`eventsource_stream::Eventsource`**. `bytes_stream` 위에 얹는 어댑터로 `data:` / `event:` 프레임 경계를 알아보고 `eventsource_stream::Event { event, data, id, retry }` 한 개를 한 번에 deliver. SSE 스펙의 라인 버퍼링(빈 줄 = frame 종료, `\n\n` 등) 을 여기서 흡수.
4. **JSON / Choices delta 파서**. 두 갈래로 분기 — Responses 는 `responses.rs::ResponsesStreamEvent` 로 typed deserialize, Chat 은 `chat.rs::process_chat_sse` 가 `serde_json::Value` 로 받아 incremental state 를 재조립. 결과는 공통 `ResponseEvent` enum (`common.rs:68-107`) 으로 통일되어 `mpsc::Sender<Result<ResponseEvent, ApiError>>` 채널로 흐른다.

`spawn_response_stream` / `spawn_chat_stream` 는 위 파이프라인을 `tokio::spawn` 한 별도 task 에 돌려놓고, 호출자에게는 `ResponseStream { rx_event, upstream_request_id }` 만 돌려준다 (`codex-api/src/common.rs:288-300`). 이 stream 자체가 `futures::Stream<Item = Result<ResponseEvent, ApiError>>` 라서 turn 루프는 `.next().await` 만 부른다.

---

## 2. Responses vs Chat — 두 개의 파서

### 2.1 Responses (`sse/responses.rs`)

upstream 표준. 서버가 친절하게 **typed event** 를 한 줄씩 보내준다. `kind` 필드 (= SSE `event:` 헤더) 로 분기:

```
response.created          → ResponseEvent::Created
response.output_item.added → ResponseEvent::OutputItemAdded(ResponseItem)
response.output_item.done  → ResponseEvent::OutputItemDone(ResponseItem)
response.output_text.delta → ResponseEvent::OutputTextDelta(String)
response.custom_tool_call_input.delta
response.function_call_arguments.delta → ResponseEvent::ToolCallInputDelta { item_id, call_id, delta }
response.reasoning_summary_text.delta  → ResponseEvent::ReasoningSummaryDelta
response.reasoning_text.delta          → ResponseEvent::ReasoningContentDelta
response.reasoning_summary_part.added  → ResponseEvent::ReasoningSummaryPartAdded
response.metadata          → (의 `openai_verification_recommendation`) → ModelVerifications
response.completed         → ResponseEvent::Completed { response_id, token_usage, end_turn }
response.failed            → 에러 분류 후 `ApiError::*`
response.incomplete        → ApiError::Stream(...)
```

핵심 진입 함수:

- `process_responses_event(ResponsesStreamEvent) -> Result<Option<ResponseEvent>, ResponsesEventError>` (`responses.rs:297-431`) — 한 SSE 이벤트를 `ResponseEvent` 로 매핑. 모르는 `kind` 는 `trace!` 후 무시.
- `process_sse(stream, tx_event, idle_timeout, telemetry)` (`responses.rs:433-519`) — `Eventsource` 어댑터를 돌리며 idle timeout 적용, `response.failed` 에서 에러 분류, `response.completed` 직후 stream 강제 종료.

`response.failed` 의 에러 코드는 SSE 단계에서 분류된다 (`is_context_window_error`, `is_quota_exceeded_error`, `is_cyber_policy_error`, `is_invalid_prompt_error`, `is_server_overloaded_error`, `try_parse_retry_after`). 어떤 건 `ApiError::ContextWindowExceeded` 처럼 fatal, 어떤 건 `ApiError::Retryable { delay }` 로 재시도 가능 — 이 분기에 따라 `core::client` 의 retry 가 다르게 동작한다.

또 하나 중요한 점: `spawn_response_stream` 는 `process_sse` 를 부르기 **전에** HTTP 응답 헤더를 훑어 다음을 미리 emit 한다 (`responses.rs:97-114`).

- `OpenAI-Model` → `ResponseEvent::ServerModel(model)` (서버가 다른 모델로 routing 한 경우 경고)
- 모든 `RateLimitSnapshot` → `ResponseEvent::RateLimits(...)` (`rate_limits::parse_all_rate_limits`)
- `X-Models-Etag` → `ResponseEvent::ModelsEtag`
- `X-Reasoning-Included` → `ServerReasoningIncluded(true)`
- `x-codex-turn-state` → `OnceLock<String>` 에 저장 (turn 간 헤더 echo)

즉 "데이터 stream" 에는 SSE body 뿐 아니라 헤더-derived 메타도 같이 흘러나온다. 이것은 호출자 입장에서 한 channel 로 통합되어 보인다.

### 2.2 Chat Completions (`sse/chat.rs`)

이쪽은 chunk 형식이 본질적으로 다르다. 서버가 보내는 한 frame 의 body 는 OpenAI Chat 포맷의 partial JSON:

```jsonc
{ "choices": [
    { "delta": { "content": "안녕", "role": "assistant" } },
    { "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "{\"x" } }] } },
    { "finish_reason": "stop" }     // 또는 "tool_calls", "length"
] }
```

stream 종료는 typed event 가 아니라 **sentinel** `data: [DONE]` (또는 일부 mock 의 `data: DONE`) 으로 알린다. `process_chat_sse` (`chat.rs:59-332`) 는 이 사실 때문에 typed parser 와 달리 **stateful re-assembly** 를 한다:

- `assistant_item: Option<ResponseItem>` — 누적되는 assistant 메시지. 첫 `delta.content` 도착 시 빈 메시지를 만들어 `OutputItemAdded` 를 emit, 이후 chunk 마다 `OutputTextDelta` emit.
- `reasoning_item: Option<ResponseItem>` — `delta.reasoning` (string / `{text}` / `{content}` 셋 다 지원), 또는 `message.reasoning` (final-message 형) 을 누적.
- `tool_calls: HashMap<usize, ToolCallState>` — 아래 §4 에서 자세히.
- 마지막에 `[DONE]` 또는 `finish_reason == "stop"` 을 보면 `flush_and_complete` 로 `OutputItemDone(reasoning) → OutputItemDone(assistant) → Completed` 순으로 토해낸다. `finish_reason == "length"` 는 `ApiError::ContextWindowExceeded`. `finish_reason == "tool_calls"` 면 `tool_call_order` 순서대로 `FunctionCall` ResponseItem 을 만들어 `OutputItemDone` 로 보낸다.

여기서 발생하는 **non-trivial 차이**:

- Responses 는 서버가 typed event 로 모든 lifecycle 을 알려주지만, Chat 은 클라이언트가 직접 "메시지가 시작됐다 / 끝났다" 를 추론해야 한다.
- `Completed` 의 `response_id` 는 Chat 경로에서 항상 빈 문자열 (`String::new()`). `token_usage` 도 `None`. 즉 Chat wire 로 들어오면 turn 종료 후 토큰 카운트가 없음 — `core/session/turn.rs:2121` 의 `update_token_usage_info` 가 None 을 받게 된다. 운영 시 fork 환경에서 token 메트릭이 0 으로 보이면 이 경로 때문이다.
- Chat 경로는 헤더 기반 `ServerModel` / `RateLimits` 를 emit 하지 않는다 (`spawn_chat_stream` 에는 그 로직이 없음, `chat.rs:23-42`).

fork 의 ollama 게이트웨이는 chat completions 만 노출하므로 실 운영에서 이 경로를 탄다.

---

## 3. ResponseEvent enum — 의미와 emit 시점

`codex-rs/codex-api/src/common.rs:68-107` 에서 정의된 variants:

| variant | emit 시점 | 기본 의미 |
| --- | --- | --- |
| `Created` | Responses: `response.created` 한 번. Chat: emit 되지 않음 | turn 의 모델 호출이 서버에 수락됨. TTFT 측정 시작점 (`turn_timing.rs:144-155`). |
| `OutputItemAdded(ResponseItem)` | 새 message / reasoning / tool call 이 시작될 때 | UI 가 빈 카드를 미리 그리고 delta 로 채울 수 있게 한다. `turn.rs:2009-2074` 에서 `handle_non_tool_response_item` 으로 변환. |
| `OutputTextDelta(String)` | assistant 텍스트 한 조각이 도착할 때마다 | TUI streaming 텍스트의 원천. `active_item` 이 `AgentMessage` 인 경우 `assistant_message_stream_parsers.parse_delta` 로 markdown/code-fence 부분 파싱 후 `EventMsg::AgentMessageContentDelta` 송신 (`turn.rs:2132-2160`). |
| `ReasoningSummaryDelta { delta, summary_index }` | reasoning summary 텍스트 chunk | `EventMsg::ReasoningContentDelta` 로 변환 (turn.rs:2179-2196). |
| `ReasoningContentDelta { delta, content_index }` | raw reasoning 본문 chunk | `EventMsg::ReasoningRawContentDelta` (turn.rs:2209+). |
| `ReasoningSummaryPartAdded { summary_index }` | reasoning 의 새 섹션 시작 | `EventMsg::AgentReasoningSectionBreak` 로 단락 구분. |
| `ToolCallInputDelta { item_id, call_id, delta }` | custom tool 의 incremental arguments | `ToolArgumentDiffConsumer` 가 partial JSON 을 누적/diff 해 UI 에 부분 인자 표시. |
| `OutputItemDone(ResponseItem)` | 한 item 이 완성 | `handle_output_item_done` 이 분기: assistant message → 최종 텍스트 commit, FunctionCall → ToolRouter 에 dispatch. **이 시점이 실제 tool 실행을 트리거한다.** |
| `Completed { response_id, token_usage, end_turn }` | turn 마무리 | `update_token_usage_info`, `should_emit_turn_diff = true`, `end_turn == Some(false)` 면 `needs_follow_up` 로 다음 sampling 강제. `turn.rs:2109-2131`. |
| `ServerModel(String)` | 응답 헤더에서 발견 | 사용자가 요청한 모델과 서버 routing 모델이 다를 때 1회만 경고 (`maybe_warn_on_server_model_mismatch`). |
| `RateLimits(RateLimitSnapshot)` | 응답 헤더에서 0..N 개 | `update_rate_limits` 만 호출, `Completed` 와 묶어 `TokenCount` 이벤트로 합쳐 송신 (중복 방지). |
| `ModelsEtag(String)` | `X-Models-Etag` 헤더 | `models_manager.refresh_if_new_etag` 비동기 갱신 트리거. |
| `ModelVerifications(...)` | `response.metadata` | 모델 사용 자격 추가 검증 권장 — 한 번만 emit. |
| `ServerReasoningIncluded(bool)` | `X-Reasoning-Included` 헤더 | 클라이언트가 reasoning token 추정을 다시 하지 않도록 플래그. |

---

## 4. Tool call delta 처리 (Chat 경로 핵심 미세조정)

Chat completions 는 tool 호출 인자(`function.arguments`) 를 chunk 단위 string 으로 보낸다. 한 호출이 여러 frame 에 걸쳐 도착하므로 클라이언트가 직접 stitching 해야 한다. `chat.rs:201-253` 의 알고리즘:

```
ToolCallState { id, name, arguments: String }   // arguments 는 누적 String

각 delta.tool_calls[i] 에 대해:
  index 결정:
    1) tool_call.index 가 있으면 그것
    2) tool_call.id 가 이미 본 적 있으면 mapping table (tool_call_index_by_id) 에서 lookup
    3) 둘 다 없으면 last_tool_call_index 재사용 (id 누락된 후속 chunk)
    4) 그것도 없으면 새 index 할당 (next_tool_call_index, 빈 슬롯)

  state.id.get_or_insert(tool_call.id)              // id 는 첫 등장만 보존
  state.name.get_or_insert(function.name)           // 빈 문자열은 무시
  state.arguments.push_str(function.arguments)      // arguments 는 모두 concat
  last_tool_call_index = Some(index)
```

이 stitching 의 의의:

- **chunk 경계가 JSON 경계가 아님**. `"{\"foo\":"` + `"1}"` 처럼 토큰 중간에 잘려 와도 단순 `push_str` 으로 잘 합쳐진다 (parsing 은 tool 실행 시점에 한 번에).
- **`finish_reason == "tool_calls"` 직전까지** 부분 인자가 도착할 수 있음. 그래서 `process_chat_sse` 는 finish 시점에야 비로소 `tool_call_order` 순서로 `ResponseItem::FunctionCall { call_id, name, arguments }` 한 덩어리로 emit. 즉 Chat 경로에서는 `ToolCallInputDelta` 를 **emit 하지 않고**, 완성된 인자만 한 번에 보낸다 → tool argument streaming UI 가 안 보인다는 한계가 있음 (Responses 만 incremental 표시).
- `finish_reason == "stop"` 인데 partial tool_call 이 있으면 그 호출은 **버린다** (test `drops_partial_tool_calls_on_stop_finish_reason` 참고).
- `name` 이 끝까지 비어 있는 호출은 `"Skipping tool call at index .. because name is missing"` debug 로그로 drop.

Responses 경로는 더 단순하다 — 서버가 한 호출을 하나의 `OutputItemDone(FunctionCall)` 로 묶어 보내고, 진행 중에는 `ToolCallInputDelta` 만 흘려보낸다. 클라이언트는 누적할 필요가 없고 `ToolArgumentDiffConsumer` 가 incremental 표시만 담당.

---

## 5. `stream_idle_timeout` — 침묵 끊기

`provider.stream_idle_timeout()` 은 `model-provider-info::ModelProviderInfo::stream_idle_timeout_ms` (default `DEFAULT_STREAM_IDLE_TIMEOUT_MS`, `model-provider-info/src/lib.rs:308-312`) 에서 `Duration` 으로 변환. 두 파서 모두 한 SSE event 를 기다리는 동안 이 timeout 을 적용한다:

```rust
// responses.rs:445, chat.rs:114
let response = timeout(idle_timeout, stream.next()).await;
```

분기:
- `Ok(Some(Ok(sse)))` — 정상 frame, 처리 후 다시 loop.
- `Ok(Some(Err(e)))` — transport 레벨 에러 → `ApiError::Stream(e.to_string())` 송신 후 task 종료.
- `Ok(None)` — server 가 close (graceful EOF). Responses 는 `response.completed` 를 못 본 채 닫혔으면 `ApiError::Stream("stream closed before response.completed")` 로 보낸다. Chat 은 `[DONE]` 못 보고 닫혔으면 `flush_and_complete` 로 누적된 거 토해내고 정상 종료 (이상한 mock 호환성을 위해서, comment 참조).
- `Err(_)` (timeout) — `ApiError::Stream("idle timeout waiting for SSE")` 로 `ResponseEvent::Err` 보내고 task 종료.

이 timeout 에러는 `core::client` 의 retry layer 와 상호작용한다. `provider.stream_max_retries()` (`DEFAULT_STREAM_MAX_RETRIES` 기본값) 회까지는 stream 을 다시 열 수 있고, 그 뒤로는 turn 이 실패한다. fork 환경에서 ollama 게이트웨이가 첫 토큰 생성에 오래 걸리면 (큰 prompt + slow GPU) 여기서 idle timeout 에 걸려 stream 이 끊기는 사례가 자주 보고된다 — 운영 시 `stream_idle_timeout_ms` 를 5분(=300_000) 이상으로 늘리는 것이 권장 (`config_tests.rs:6685` 의 fixture 참고).

---

## 6. UI 까지 도달하는 경로

`ResponseEvent` 를 만들어내는 곳은 `codex-api`, 소비하는 곳은 `core` 의 turn 루프. 흐름:

```
sse/responses.rs / sse/chat.rs
        │  Result<ResponseEvent, ApiError>  (mpsc 1600 buffer)
        ▼
codex-api::ResponseStream  (futures::Stream)
        │
        ▼
core::client::ModelClient::stream  (retry / wrap)
        │
        ▼
core::session::turn::sample_response_loop  (turn.rs:1900-2230)
        │  per-event → EventMsg::*  (codex_protocol::protocol)
        ▼
sess.send_event(turn_context, EventMsg::*)
        │  broadcast tokio::sync 채널 → Session 의 event subscribers
        ├──────────────► app-server (codex-app-server) — JSON-RPC notification
        │                  (thread/eventNotification 등 v2 protocol)
        ├──────────────► exec/exec_events.rs — JSON ND 라인으로 stdout 출력
        └──────────────► tui (chatwidget 등) — Ratatui 위젯 갱신
```

핵심 변환 매핑(turn.rs):

- `OutputTextDelta` → assistant 가 active 면 `AgentMessageContentDelta`. markdown stream parser 를 통해 부분 파싱이 들어가므로 코드펜스 중간이 잘려도 UI 는 비교적 안전.
- `OutputItemAdded` → `handle_non_tool_response_item` 으로 turn item placeholder 생성, `emit_turn_item_started`.
- `OutputItemDone(message)` → `flush_assistant_text_segments_for_item` 으로 잔여 markdown segment flush, `handle_output_item_done` 으로 최종 commit.
- `OutputItemDone(FunctionCall)` → `handle_output_item_done` 안에서 `ToolRouter` 에 dispatch → `ToolCallRuntime` 가 tool 실행을 in-flight queue 에 push.
- `Completed` → `update_token_usage_info`, `RateLimitSnapshot` 과 합쳐서 `TokenCount` 이벤트 발송, turn loop break.

`exec` 진입점은 `codex-rs/exec` 에서 `EventMsg` 를 직렬화해 stdout 으로 흘리고, `tui` 는 같은 `EventMsg` 를 ratatui 위젯에 매핑한다. 둘 다 `core::session` 위쪽에 붙는 별도 consumer 일 뿐, SSE 파서 자체는 모른다.

---

## 7. 디버깅 / 테스트 패턴

### 7.1 `core_test_support::responses` (`tests/common/responses.rs`)

대부분의 core integration 테스트는 wiremock + 이 헬퍼들로 SSE 를 위조한다.

- `ev_response_created(id)`, `ev_completed(id)`, `ev_completed_with_tokens(id, n)`, `ev_assistant_message(id, text)`, `ev_function_call(call_id, name, args)`, `ev_message_item_added(...)`, `ev_model_verification_metadata(...)` 등 — 한 SSE event 의 raw JSON `Value` 를 만든다.
- `sse(events: Vec<Value>) -> String` — 위 Value 들을 `event: <kind>\ndata: {...}\n\n` 으로 직렬화. 빈 body (1-key) 인 경우는 `event:` 만 emit (responses 의 phantom event 흉내).
- `sse_completed(id)` — `[response.created, response.completed]` 두 개를 묶은 minimal stream.
- `mount_sse_once(server, body)` — `MockServer` (wiremock) 의 `/responses` 엔드포인트에 SSE body 1회 응답을 mount, `ResponseMock` 핸들 반환. `ResponseMock::single_request().await` 로 `body_json`, `function_call_output`, `header`, `query_param` 등을 검사.
- `mount_sse_once_match(server, matcher, body)` — wiremock matcher (`body_json_partial`, `query_param_contains`) 를 추가로 걸 수 있는 변형.
- `mount_compact_json_once*` — non-SSE compaction 응답용.

테스트 작성 패턴:

```rust
let server = MockServer::start().await;
let body = sse(vec![
    ev_response_created("resp-1"),
    ev_assistant_message("msg-1", "hello"),
    ev_completed("resp-1"),
]);
let mock = mount_sse_once(&server, body).await;
// ... codex 실행 ...
let request = mock.single_request().await;
assert_eq!(request.body_json::<Value>()["model"], "...");
```

### 7.2 SSE 파서 단위 테스트 (`responses.rs::tests`, `chat.rs::tests`)

- `collect_events(chunks: &[&[u8]])` — 청크 경계가 SSE 라인 경계와 어긋나도 reassembly 가 잘 되는지 확인하기 위해 `tokio_test::io::Builder` 로 byte 단위 stream 을 흘려보낸다. fragmented HTTP chunk 시뮬레이션의 표준 방법.
- `run_sse(events: Vec<Value>)` — events 를 SSE body 로 묶고 `process_sse` 를 직접 호출.
- `stream_from_fixture(path, idle_timeout)` (`responses.rs:35-61`) — 디스크의 fixture 파일을 SSE 처럼 재생. `CODEX_RS_SSE_FIXTURE` 환경변수가 set 되면 `client.rs` 의 `stream_responses_api` 가 실제 서버 대신 이 fixture 를 쓴다 — 결정적 회귀 테스트용. 예: `core/tests/cli_responses_fixture.sse`.

### 7.3 idle timeout / chunked 시나리오 (`tests/common/streaming_sse.rs`)

`StreamingSseServer::start_streaming_sse_server(responses)` — 진짜 TCP listener 로 도는 미니 HTTP 서버. 각 chunk 에 `oneshot::Receiver<()>` 로 **gate** 를 붙일 수 있어, 의도적으로 침묵을 만들어 `stream_idle_timeout` 동작을 검증한다. `wiremock` 으로는 chunk-by-chunk 타이밍 제어가 어려워 따로 만든 도구.

### 7.4 fork-only 에 비활성된 chat completions 테스트

- `core/tests/chat_completions_sse.rs` (`#![cfg(any())]`) — fork 가 d2394a2494^ 에서 복원했으나 protocol drift (`ResponseEvent::Completed { end_turn }` 추가, `ResponseItem::FunctionCall { namespace }` 추가) 때문에 컴파일 안 됨. 활성화하려면 `Prompt::default().input` 의 `ResponseItem::Message` 에서 `phase: None` (이미 OK), `end_turn: None` 필드 제거 또는 protocol 쪽 정렬이 필요. 한 케이스씩 cfg gate 빼면서 통과시키는 게 권장 경로.
- `core/tests/chat_completions_payload.rs` — request body 검증 쪽도 같은 사유로 비활성. 활성화 순서: payload → sse → end-to-end.

테스트 디버깅 팁:
- `tracing_test::traced_test` + `logs_assert!` 로 `codex.sse_event` / `codex.api_request` span 검사 (위 chat 테스트의 `chat_sse_emits_failed_on_parse_error` 참고).
- `wait_for_event` (core_test_support) 로 `EventMsg::*` 까지 기다리되 `_with_timeout` 변형은 회피 — 실패 시 진단이 어렵다. timeout 은 wiremock 이 안 받으면 자연스레 stuck → 대부분 mock matcher 가 잘못 걸린 경우.

---

## 부록: Responses 와 Chat 의 한 줄 비교

| 축 | Responses | Chat |
| --- | --- | --- |
| 종료 신호 | `response.completed` typed event | `data: [DONE]` sentinel + `finish_reason` |
| message 시작 알림 | `response.output_item.added` | 클라이언트 추론 (`assistant_item.is_none()`) |
| reasoning chunk | typed `response.reasoning_text.delta` (+ summary) | `delta.reasoning` 의 string / `{text}` / `{content}` 셋 변형 |
| tool argument streaming | `ToolCallInputDelta` per chunk | 누적 후 finish 에 한 번에 (`ToolCallInputDelta` emit 안 함) |
| token usage | `Completed.token_usage` 채워짐 | 항상 `None` (fork 한계) |
| 헤더 메타 emit | `ServerModel`, `RateLimits`, `ModelsEtag`, `ServerReasoningIncluded`, `ModelVerifications` | 없음 (`upstream_request_id` 만 보존) |
| 에러 분류 | `response.failed` body 의 `error.code` 에서 분기 | `finish_reason == "length"` → `ContextWindowExceeded`, 그 외는 generic |
| fork 사용처 | upstream 표준 / cloud OpenAI | 이 fork 의 ollama / Qwen 게이트웨이 |
