# 03 — Wire Protocol Layer

LLM 호출이 실제로 HTTP 위에서 어떻게 직렬화되는지 정리한다. 이 fork 의 핵심 작업은 upstream 이 한 번 제거했던 **Chat Completions** wire 경로를 부활시킨 것이고 (`fork-docs/work-log-2026-05-08.md` §2.2 / §2.5 참조), 그 경로의 빌더 / 파서 위치를 추적하는 것이 본 문서의 목적이다.

진입점은 `codex-rs/core/src/client.rs:1496` 의 `Codex::stream` 한 곳이다. 여기서 provider 의 `wire_api` 필드를 보고 두 갈래로 분기한다 — 한쪽은 OpenAI Responses API (`/v1/responses`), 다른 한쪽은 OpenAI 호환 게이트웨이용 Chat Completions API (`/v1/chat/completions`).

## 1. WireApi enum

정의: `codex-rs/model-provider-info/src/lib.rs:44-55`.

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WireApi {
    #[default]
    Responses,
    Chat,
}
```

- `WireApi::Responses` — upstream OpenAI 의 신형 `/v1/responses` 엔드포인트. multi-turn `previous_response_id`, `output_schema`, reasoning 아이템, web_search / image_generation 같은 first-party tool 까지 풀 표면을 지원한다. lmstudio 빌트인 provider 가 여전히 이 값을 쓴다 (`model-provider-info/src/lib.rs:436-438`).
- `WireApi::Chat` — fork 가 부활시킨 변종. 사내 nginx-fronted Qwen 게이트웨이가 `/v1/chat/completions` 만 노출하므로 필요. `Display`/`Deserialize` 양쪽에 `"chat"` 매핑을 다시 박아 두었다 (`lib.rs:57-65`, `:67-82`).

기본값 (`Default::default()`) 은 `Responses` 다. 그러나 fork 의 디폴트 provider 는 `ollama` 이고 (`codex-rs/core/src/config/mod.rs:2634`), 그 빌트인 provider 가 명시적으로 `WireApi::Chat` 를 박는다 (`model-provider-info/src/lib.rs:415-425`):

```rust
let mut p = create_oss_provider(DEFAULT_OLLAMA_PORT, WireApi::Chat);
p.env_key = Some("OLLAMA_API_KEY".to_string());
```

분기점은 `codex-rs/core/src/client.rs:1507`:

```rust
let wire_api = self.client.state.provider.info().wire_api;
match wire_api {
    WireApi::Responses => { /* websocket → http fallback */ }
    WireApi::Chat => { self.stream_chat_completions_api(...).await }
}
```

## 2. Responses API 경로

엔드포인트 클라이언트: `codex-rs/codex-api/src/endpoint/responses.rs`. `ResponsesClient::stream_request` (`responses.rs:69`) 가 `POST /v1/responses` 를 친다.

요청 페이로드 타입은 `ResponsesApiRequest` (`codex-rs/codex-api/src/common.rs:165-186`):

| 필드 | 비고 |
| --- | --- |
| `model` | 모델 슬러그 |
| `instructions` | 빈 문자열이면 직렬화에서 빠짐 (`skip_serializing_if`) |
| `input: Vec<ResponseItem>` | 그대로 직렬화. Responses API 는 messages/function_calls/reasoning/web_search_call 등 모든 변종을 native 로 받는다 |
| `tools: Vec<Value>` | `tools/src/tool_spec.rs:153` 의 `create_tools_json_for_responses_api` 가 만든 JSON 그대로 |
| `tool_choice`, `parallel_tool_calls` | |
| `reasoning: Option<Reasoning>` | effort + summary |
| `store: bool` | true 면 서버가 response 를 저장하고 다음 턴에 `previous_response_id` 로 이어붙일 수 있다 |
| `stream: bool` | 항상 SSE |
| `include`, `service_tier`, `prompt_cache_key`, `text`, `client_metadata` | optional |

Responses 고유 기능:

- **`previous_response_id` chaining**: HTTP 직접 요청 (`ResponsesApiRequest`) 에는 들어 있지 않지만, websocket 변종 (`ResponseCreateWsRequest`, `common.rs:212-236`) 에 `previous_response_id: Option<String>` 가 있다. WebSocket 경로에서 서버측 thread 를 이어 붙이는 용도. HTTP path 에서는 입력 `Vec<ResponseItem>` 자체로 전체 history 를 매번 재전송한다.
- **`output_schema` / `text.format`**: structured output. `create_text_param_for_request` (`common.rs:268`) 가 `verbosity` + `output_schema` 를 합쳐 `TextControls` 를 만든다.
- **`attach_item_ids`**: Azure Responses 엔드포인트일 때만 (`store == true && is_azure_responses_endpoint()`) 입력 아이템에 원래 id 를 다시 박아 준다 (`endpoint/responses.rs:84`, `requests/responses.rs:11-37`). Reasoning / Message / FunctionCall 등의 id 를 보존.

SSE 파서는 `codex-rs/codex-api/src/sse/responses.rs` 의 `process_responses_event`. `response.reasoning_*`, `response.output_item.*` 등 OpenAI 의 풍부한 이벤트 종류를 `ResponseEvent::*` 로 normalize 한다.

## 3. Chat Completions 경로 (fork 복원분)

`d2394a2494^` history 에서 끌어와 다시 넣은 세 파일이 핵심이다:

- `codex-rs/codex-api/src/endpoint/chat.rs` — `ChatClient`. `POST /v1/chat/completions` 으로 SSE 스트리밍.
- `codex-rs/codex-api/src/requests/chat.rs` — `ChatRequestBuilder`. `Vec<ResponseItem>` → Chat Completions JSON.
- `codex-rs/codex-api/src/sse/chat.rs` — `process_chat_sse`. 역방향 (`{choices:[{delta:{...}}]}` → `ResponseEvent`).

mod 트리 재노출은 `codex-api/src/{endpoint,requests,sse}/mod.rs` 와 `lib.rs` 에서 `chat::*` 를 `pub use` 하는 식.

### 3.1 message 빌더 — `ChatRequestBuilder::build`

파일: `codex-rs/codex-api/src/requests/chat.rs:58-322`.

순서:

1. `messages` 첫 항목으로 `{"role":"system","content": instructions}` 를 push (`:60`).
2. `last_emitted_role` 추적 루프 (`:64-96`) — 입력 `ResponseItem` 들을 한번 훑어 마지막으로 게이트웨이에 보낼 role 이 무엇이 되는지 미리 결정. trailing user 자동 합성 / reasoning anchor 결정에 쓰인다.
3. Reasoning anchor 매핑 (`:96-154`) — 직전 assistant 메시지 또는 직후 function_call 에 reasoning 텍스트를 attach 할 인덱스를 `reasoning_by_anchor_index` 에 모은다.
4. 본 직렬화 루프 (`:158-303`) — 각 `ResponseItem` 변종을 Chat Completions 모양으로 push.

#### Role 매핑 (`developer → user`)

`requests/chat.rs:198`:

```rust
let outbound_role = if role == "developer" { "user" } else { role };
```

게이트웨이가 표준 `system|user|assistant|tool` enum 만 받기 때문이다. 1차 시도 (`developer → system`) 는 `System message must be at the beginning.` 로 거부됐다 — 게이트웨이 정책상 leading system 메시지 1개만 허용. 그래서 `developer → user` 로 떨어뜨렸다 (work-log §2.5).

`last_emitted_role` 추적은 원본 role (`"developer"`) 그대로 둔다. trailing-user 합성 로직이 원본 의미에서 동작해야 정확하기 때문에 의도적으로 미변경.

#### Tool call 직렬화

`function` 호출은 OpenAI 표준 `tool_calls` 모양으로 묶는다 (`requests/chat.rs:208-224`):

```rust
let tool_call = json!({
    "id": call_id,
    "type": "function",
    "function": { "name": name, "arguments": arguments }
});
push_tool_call_message(&mut messages, tool_call, reasoning);
```

`push_tool_call_message` (`:324-360`) 는 직전 assistant 메시지가 이미 `tool_calls: [...]` 어레이를 들고 있으면 거기에 append, 아니면 `{"role":"assistant","content":null,"tool_calls":[..]}` 신규 메시지 push. 연속된 function_call 을 한 assistant 메시지로 묶는 것이 Chat Completions 의 요구사항이다.

`local_shell_call`, `custom_tool_call` 도 같은 helper 를 통해 `tool_calls` 어레이에 들어가지만 type 이 `"local_shell_call"` / `"custom"` 으로 다르다 — 게이트웨이가 알 수 없는 type 이므로 실질적으로 표준 OpenAI 호환 게이트웨이에서는 no-op 에 가깝다.

#### Tool 응답 처리

`FunctionCallOutput` 은 `role:"tool"` 로 떨어뜨린다 (`requests/chat.rs:240-264`):

```rust
messages.push(json!({
    "role": "tool",
    "tool_call_id": call_id,
    "content": content_value,
}));
```

이미지가 섞인 출력은 `[{"type":"text",...},{"type":"image_url",...}]` 형태의 multi-part content 로 직렬화. 텍스트만 있으면 그냥 string.

#### 페이로드 마무리

`requests/chat.rs:305-310`:

```rust
let payload = json!({
    "model": self.model,
    "messages": messages,
    "stream": true,
    "tools": self.tools,
});
```

`tools` 는 호출자 (`core/src/client.rs:1587`) 가 미리 변환한 chat completions 모양 어레이. Responses API 의 `tool_choice`, `parallel_tool_calls`, `reasoning`, `previous_response_id`, `service_tier`, `prompt_cache_key`, `output_schema` 등 추가 컨트롤은 **모두 빠진다**. `core/src/client.rs:1573-1577` 에서 `prompt.output_schema.is_some()` 이면 `CodexErr::UnsupportedOperation` 으로 즉시 거부한다.

### 3.2 SSE 디코딩 — `process_chat_sse`

`codex-rs/codex-api/src/sse/chat.rs:59`. 입력은 `data: {json}\n\n` 라인의 stream, 출력은 `mpsc::Sender<Result<ResponseEvent, ApiError>>`.

상태:

- `assistant_item: Option<ResponseItem>` — assistant 메시지 누적.
- `reasoning_item: Option<ResponseItem>` — reasoning 텍스트 누적 (`Reasoning` 변종).
- `tool_calls: HashMap<usize, ToolCallState>` + `tool_call_index_by_id` + `tool_call_order` — `index` / `id` 두 가지 키 모두로 delta 청크를 합친다 (`:201-253`). 두 번째 이후 delta 가 `id` 를 안 보낼 수도 있어서 인덱스 기반 fallback 이 필요 (Qwen 게이트웨이의 경우 자주 발생).

이벤트 매핑:

| 입력 (`delta` 필드) | 출력 (`ResponseEvent`) |
| --- | --- |
| `delta.content` (string 또는 `[{type:"text",text:...}]`) | `OutputItemAdded(Message)` 1회 + `OutputTextDelta` n회 |
| `delta.reasoning` (string 또는 `{text}` / `{content}`) | `OutputItemAdded(Reasoning)` 1회 + `ReasoningContentDelta` |
| `delta.tool_calls[]` | 누적만, `finish_reason == "tool_calls"` 시 flush |
| `finish_reason == "stop"` | `OutputItemDone(reasoning?)` + `OutputItemDone(assistant?)` + `Completed` |
| `finish_reason == "length"` | `Err(ApiError::ContextWindowExceeded)` |
| `finish_reason == "tool_calls"` | 모은 `ToolCallState` 들을 `ResponseItem::FunctionCall` 로 emit |
| `[DONE]` 또는 `DONE` sentinel | `flush_and_complete` 로 `Completed` 보냄 |

`Completed` 의 `response_id` 는 빈 문자열, `token_usage` 는 `None`, `end_turn` 도 `None` 으로 둔다 (`:103-109`) — Chat Completions 응답에 등가물이 없다.

## 4. Tool spec 변환

위치: `codex-rs/tools/src/tool_spec.rs:174-200`.

```rust
pub fn create_tools_json_for_chat_completions_api(
    tools: &[ToolSpec],
) -> Result<Vec<Value>, serde_json::Error> {
    let responses_api_tools_json = create_tools_json_for_responses_api(tools)?;
    let tools_json = responses_api_tools_json
        .into_iter()
        .filter_map(|mut tool| {
            if tool.get("type") != Some(&Value::String("function".to_string())) {
                return None;
            }
            let map = tool.as_object_mut()?;
            let name = map.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            map.remove("type");
            Some(serde_json::json!({
                "type": "function",
                "name": name,
                "function": map,
            }))
        })
        .collect::<Vec<Value>>();
    Ok(tools_json)
}
```

전략: Responses API 용 tool JSON 을 먼저 만든 다음 Chat Completions 형태로 한 단계 wrap. 그래서 새로운 ToolSpec variant 가 추가돼도 base 변환만 거치면 자동으로 흘러간다.

drop 되는 변종 (`type != "function"`):

- `web_search` (`ToolSpec::WebSearch`, `tool_spec.rs:99-127`)
- `image_generation` (`ToolSpec::ImageGeneration`, `:87-95`)
- `local_shell` (`ToolSpec::LocalShell`)
- `tool_search` (`ToolSpec::ToolSearch`)
- `Namespace` 컨테이너의 wrapper 자체 (children 은 function 이라 살아남는다)
- `Freeform` (`custom` type)

이 fork 운영 환경에서 게이트웨이가 native web_search / image_generation 을 지원하지 않으므로 drop 이 맞다. 단, 그 결과 모델이 해당 tool 을 호출할 수단이 없어진다는 점은 알려진 제약이다.

## 5. 인증 헤더 (env_key → Authorization: Bearer)

흐름은 `provider.env_key` 환경변수 이름 → 환경에서 값 읽기 → `BearerAuthProvider` → `Authorization` 헤더.

1. `ModelProviderInfo::api_key` (`model-provider-info/src/lib.rs:275-289`):

   ```rust
   pub fn api_key(&self) -> CodexResult<Option<String>> {
       match &self.env_key {
           Some(env_key) => {
               let api_key = std::env::var(env_key).map_err(|_| EnvVarError { ... })?;
               Ok(Some(api_key))
           }
           None => Ok(None),
       }
   }
   ```

2. `bearer_auth_for_provider` (`codex-rs/model-provider/src/auth.rs:92-104`) 가 위 결과를 받아 `BearerAuthProvider::new(api_key)` 로 감싼다.
3. `BearerAuthProvider::add_auth_headers` (`codex-rs/model-provider/src/bearer_auth_provider.rs:31-47`):

   ```rust
   if let Some(token) = self.token.as_ref()
       && let Ok(header) = HeaderValue::from_str(&format!("Bearer {token}"))
   {
       let _ = headers.insert(http::header::AUTHORIZATION, header);
   }
   ```

4. `EndpointSession::stream_with` (chat / responses 양쪽이 공유) 가 매 요청 직전에 `auth.apply_auth(request)` 를 호출 (`codex-api/src/auth.rs:55-59` 의 default impl 이 위 헤더를 푸시).

ollama 빌트인 provider 는 `env_key = "OLLAMA_API_KEY"` 라서, 셸 env 또는 `~/.codex/codex-fork.json` (work-log §2.7) 에서 박은 값이 그대로 `Authorization: Bearer sk-davis-...` 로 흘러간다. 로컬 Ollama 처럼 인증이 없는 케이스를 위해 `env_key` 가 비어 있으면 `BearerAuthProvider::token = None` 이라 `Authorization` 헤더 자체가 안 붙는다 (게이트웨이가 401 을 던지면 `probe_server` 가 "도달 OK" 로 해석 — work-log §2.4).

## 6. Wire 디버깅

### 6.1 `RUST_LOG=codex_api=debug`

`codex-api` 모듈은 `tracing` 인스트루먼트가 깔려 있다.

- `endpoint/chat.rs:51-60` 의 `#[instrument(name="chat.stream_request", ...)]` — 매 chat 요청에 span 1개.
- `endpoint/responses.rs:59-68` — Responses 측 동격 span.
- `sse/chat.rs:138` — `trace!("SSE event: {}", sse.data)` 로 raw chunk 확인 가능 (`RUST_LOG=codex_api=trace`).
- `sse/chat.rs:156` — JSON 파싱 실패 시 `debug!` 로 raw payload 와 에러 같이 찍는다. 게이트웨이가 비표준 frame 을 보낼 때 첫번째로 의심.

가장 자주 쓰는 조합:

```bash
RUST_LOG=codex_api=debug,codex_core=info cargo run --bin codex -- exec "say pong"
```

### 6.2 `ResponsesRequest` test helper

`codex-rs/core/tests/common/responses.rs:86` 의 `ResponsesRequest(wiremock::Request)` 래퍼가 통합 테스트의 어서션 문법을 단순화한다.

- `body_json()` (`:105-114`) — content-encoding 이 zstd 면 자동 decompress 후 `serde_json::Value` 로 파싱.
- `instructions_text()`, `tool_by_name(namespace, tool)`, `function_call_output(call_id)`, `function_call_output_text(call_id)`, `body_contains_text(text)` 등 도메인 헬퍼.

`mount_sse_once` (위 파일의 다른 곳) 가 `ResponseMock` 을 반환하고, 거기서 `single_request()` / `requests()` / `last_request()` 로 `ResponsesRequest` 들을 꺼내 어서션을 건다. SSE 응답 body 는 `ev_*` 컨스트럭터 + `sse(...)` 로 조립.

주의: 이름이 `ResponsesRequest` 라 Responses API 전용처럼 보이지만, 실제로는 wiremock 의 raw HTTP request 래퍼라 `chat/completions` 요청에도 그대로 쓸 수 있다. 다만 fork 의 chat completions path 통합 테스트는 현재 격리되어 있다 (아래 §7 참조).

## 7. 알려진 함정

### 7.1 `developer → user` 매핑

work-log §2.5 의 핵심 사건. 변경 직전까지 게이트웨이가 `400 Unexpected message role.` (또는 `developer → system` 1차 시도 시 `System message must be at the beginning.`) 로 거부했다. 단방향 매핑이라 모델 입장에서 "user 가 한 발화" 와 "developer instruction" 의 구분 신호가 사라지는 절충이 있다 — 현재는 instructions 가 leading system 메시지 1개로 들어가기 때문에 큰 문제는 없지만, multi-stage developer turn 이 늘어나면 prefix 컨벤션 (`[SYSTEM]` 등) 추가를 검토해야 한다.

### 7.2 reasoning 필드 처리

Chat Completions 표준에는 `reasoning` 이 없다. 그럼에도 fork 는 양방향 모두에서 비표준 `reasoning` 필드를 살린다:

- 빌더 측 (`requests/chat.rs:200-205`, `:354-358`): assistant 메시지나 tool_calls 어셈블리 메시지에 `obj.insert("reasoning", ...)` 로 박는다. 게이트웨이가 모르면 무시하지만 일부 Qwen 변종은 인식해서 chain-of-thought 컨텍스트로 활용한다.
- 파서 측 (`sse/chat.rs:170-181`, `:256-265`): `delta.reasoning` 이 `string` / `{text}` / `{content}` 셋 중 어느 모양으로 와도 받아 `ResponseEvent::ReasoningContentDelta` 로 흘린다.

게이트웨이가 표준 OpenAI 라면 이 필드들은 silent drop 이라 회귀 위험이 낮다.

### 7.3 `last_emitted_role` tracker 의 목적

`requests/chat.rs:64-96`. 입력 ResponseItem 들을 한번 prepass 해 "최종적으로 messages 배열의 마지막 role 이 무엇이 될지" 미리 계산한다. 두 곳에서 쓰인다:

- 마지막이 `user` 가 아닌 경우, trailing reasoning 을 그 다음에 올 assistant / function_call 에 anchor 한다 (`:96-154`). 그래서 reasoning chunk 가 Chat Completions 메시지 시퀀스 안으로 잘 끼어 들어간다.
- 원본 `role` 기준 (developer 매핑 적용 전) 으로 동작해야 의도가 맞다 — `developer → user` 매핑을 추적기에 적용하면 "trailing developer instruction" 을 "trailing user input" 으로 오인해 reasoning anchoring 이 어긋난다. 그래서 매핑은 직렬화 한 줄 (`:198`) 에서만 적용하고 추적기는 그대로 둔 것이 의도된 분리.

### 7.4 chat completions 통합 테스트 비활성

`codex-rs/core/tests/chat_completions_payload.rs:7`, `codex-rs/core/tests/chat_completions_sse.rs:4` 모두 `#![cfg(any())]` 로 컴파일에서 빠져 있다. `requests/chat.rs:366` 와 `sse/chat.rs:399` 의 inline `mod tests` 도 동일. 이유는 upstream protocol drift (`ResponseItem::Message` 의 `end_turn` 제거, `FunctionCall` 의 `namespace` 추가, `FunctionCallOutputPayload` 의 `content` → `body` 등) 다. 해당 테스트들은 wire 변경의 회귀를 잡아주는 역할이라 end-to-end 가 안정화된 후 변종별로 fixup 해 살리는 것이 잔존 항목.

### 7.5 upstream rebase 시 충돌 표면

`WireApi::Chat`, `codex-api/src/{endpoint,requests,sse}/chat.rs`, `tools/src/tool_spec.rs:174` 의 `create_tools_json_for_chat_completions_api`, `core/src/client.rs:1546-1554` 의 `WireApi::Chat` 분기는 모두 upstream 이 의도적으로 제거한 코드 경로다. rebase 시 large conflict 가 예상된다 (work-log §4 의 잔존 항목 마지막 불릿).
