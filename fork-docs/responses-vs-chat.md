# `/v1/responses` vs `/v1/chat/completions` 비교

OpenAI 의 두 LLM 호출 wire 형식. 이 fork 가 왜 Chat 쪽을 살려야 했는지 이해하려면 두 API 의 차이를 알아야 한다.

## 한 줄 요약

- **`/v1/chat/completions`** — 2023년 이후 광범위. 거의 모든 OpenAI 호환 게이트웨이 (vLLM / Ollama / lmstudio / OpenRouter / 사내 nginx 프록시) 가 이 형식만 지원. **사실상 표준.**
- **`/v1/responses`** — 2024년 OpenAI 가 도입한 차세대 agent-친화 API. 서버 사이드 상태 / typed 이벤트 / reasoning 1급 시민. **OpenAI 본가 + 일부 Azure 미러 한정**, 사내/오픈소스 게이트웨이엔 거의 미존재.

## 비교표

| 항목 | `/v1/chat/completions` | `/v1/responses` |
|---|---|---|
| **도입 시기** | 2023~ | 2024~ |
| **상태 모델** | stateless | server-side state (`previous_response_id` chain) |
| **입력 형식** | `messages[]` (role + content) | `input[]` 또는 messages, 더 구조화된 item 배열 |
| **Tool 호출 (요청)** | `tools[].type=="function"` 단일 형식 | `function`, `web_search`, `image_generation`, `local_shell`, `file_search`, ... 다양한 type |
| **Tool 호출 (응답)** | `assistant.tool_calls[]` (id, function.{name, arguments}) | structured `output_item` (`type=tool_use` 등) |
| **Tool 결과** | `role:"tool"` 메시지에 `tool_call_id` | `function_call_output` item |
| **스트림 형식** | OpenAI 제정 `choices[].delta` chunk | typed event (`response.created`, `output_item.added`, `output_text.delta`, `completed`, ...) |
| **Reasoning** | 비표준 — 벤더가 `reasoning` 필드 임의 추가 | **1급 시민** — `output_item` 의 `type=reasoning` |
| **출력 스키마** | `response_format` (`json_object`, `json_schema`) | `text.format` / `output_schema` (더 표현력 있음) |
| **System 메시지** | `role:"system"`, **맨 앞 1개만 허용** (대부분 게이트웨이) | `instructions` 필드 (메시지 외부) + 자유 |
| **Developer 메시지** | ❌ 없음 — 사내 codex 의 developer role 은 매핑 필요 | ✅ 자체 지원 |
| **호환성** | de facto 표준 — vLLM / Ollama / lmstudio / OpenRouter / Bedrock proxy / Anthropic-Bedrock proxy 등 다 됨 | OpenAI / 일부 Azure 한정 |
| **인증** | `Authorization: Bearer ...` | `Authorization: Bearer ...` |
| **이 프로젝트 (사내 게이트웨이)** | ✅ 작동 | ❌ 라우트 미노출 (404) |

## 같은 요청, 두 가지 형태

### Chat Completions
```jsonc
POST /v1/chat/completions
{
  "model": "qwen3.5-122b",
  "messages": [
    {"role": "system", "content": "You are a coding assistant."},
    {"role": "user",   "content": "fix the bug in foo.rs"}
  ],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "shell",
        "description": "Run a shell command",
        "parameters": { /* JSON Schema */ }
      }
    }
  ],
  "stream": true
}
```

응답 (stream chunk 예):
```jsonc
data: {"choices":[{"delta":{"role":"assistant","content":""},"index":0}]}
data: {"choices":[{"delta":{"content":"먼저 "},"index":0}]}
data: {"choices":[{"delta":{"tool_calls":[{"id":"call_1","type":"function",
        "function":{"name":"shell","arguments":"{\"cmd\":\"cat foo.rs\"}"}}]},"index":0}]}
data: [DONE]
```

### Responses
```jsonc
POST /v1/responses
{
  "model": "gpt-5.5",
  "instructions": "You are a coding assistant.",
  "input": [
    {"role": "user", "content": "fix the bug in foo.rs"}
  ],
  "tools": [
    {"type": "function", "name": "shell", "parameters": { /* JSON Schema */ }}
  ],
  "stream": true,
  "previous_response_id": "resp_abc123",
  "text": {"format": {"type": "text"}}
}
```

응답 (typed event 예):
```jsonc
event: response.created
data: {"id":"resp_xxx","status":"in_progress",...}

event: response.output_item.added
data: {"item":{"id":"item_1","type":"reasoning","summary":[...]}}

event: response.output_item.added
data: {"item":{"id":"item_2","type":"message","role":"assistant","content":[...]}}

event: response.output_text.delta
data: {"item_id":"item_2","delta":"먼저 "}

event: response.output_item.added
data: {"item":{"id":"item_3","type":"function_call","name":"shell",
        "arguments":"{\"cmd\":\"cat foo.rs\"}"}}

event: response.completed
data: {"response":{"id":"resp_xxx","status":"completed",...}}
```

## codex 관점에서의 차이

upstream codex 는 **Responses 를 선호** — agent 루프의 표현력이 더 풍부하고, reasoning 이 1급 시민이라 self-reflection / chain-of-thought 표현이 깔끔함.

이 fork (xtech) 는 사내 게이트웨이가 **Chat 만 지원**하므로 upstream 이 한차례 제거한 Chat 분기를 다시 살려야 했다.

### Chat 분기 복원 시 추가 변환 필요한 것들

`codex-rs/codex-api/src/requests/chat.rs` 의 빌더가 처리:

| codex 내부 (Responses 형식) | Chat 으로 변환 |
|---|---|
| `role:"developer"` 메시지 (instructions / goals / tasks) | `role:"user"` 로 매핑 (Chat 은 developer 모름) |
| `role:"system"` 메시지 여러 개 | 맨 앞 1개로 통합 (게이트웨이 정책 "system must be first") |
| typed reasoning item | assistant 메시지의 비표준 `reasoning` 필드로 inline |
| structured `tool_use` item | `assistant.tool_calls[]` 배열 |
| `function_call_output` item | `role:"tool"` 메시지 + `tool_call_id` |
| `web_search`, `image_generation`, `local_shell` 같은 Chat 미지원 tool 종류 | 요청 시 drop |

상세 구현은 [`fork-docs/arch/03-wire-protocol.md`](arch/03-wire-protocol.md) 참고.

## 결정 가이드

| 시나리오 | 추천 wire |
|---|---|
| 사내 폐쇄망 + 자체 LLM 게이트웨이 (Qwen / DeepSeek / 자체 추론서버) | **Chat** — 다른 선택지 사실상 없음 |
| OpenAI 본가 모델 + agent 풍부한 표현력 필요 | **Responses** |
| 멀티 provider 지원 어시스턴트 만들고 싶음 | **Chat** (de facto 표준) |
| 새 기능 (computer use, file search, structured output) 적극 활용 | **Responses** |

## 코드 reference

| 위치 | 내용 |
|---|---|
| `codex-rs/model-provider-info/src/lib.rs:44-55` | `WireApi` enum 정의 (`Chat` / `Responses`) |
| `codex-rs/core/src/client.rs:1496-1556` | wire_api 분기점 (`stream` 함수) |
| `codex-rs/codex-api/src/endpoint/responses.rs` | Responses 클라이언트 |
| `codex-rs/codex-api/src/endpoint/chat.rs` | Chat 클라이언트 (이 fork 가 복원) |
| `codex-rs/codex-api/src/requests/chat.rs` | Chat 빌더 + role/tool 변환 |
| `codex-rs/codex-api/src/sse/responses.rs` | Responses typed event 파서 |
| `codex-rs/codex-api/src/sse/chat.rs` | Chat choices-delta 재조합 파서 |
| `codex-rs/tools/src/tool_spec.rs:174-200` | `create_tools_json_for_chat_completions_api` (tool spec wrapping) |

## 외부 자료

- OpenAI Responses API 문서: https://platform.openai.com/docs/guides/responses
- OpenAI Chat Completions API 문서: https://platform.openai.com/docs/api-reference/chat
- 마이그레이션 가이드: https://platform.openai.com/docs/guides/migrate-to-responses
