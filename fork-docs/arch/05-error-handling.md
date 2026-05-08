# 05 — Error handling & retry semantics (xtech / codex-fork)

이 문서는 xtech (codex-fork) 빌드에서 LLM 호출 / tool 실행 / SSE 스트림 처리 경로의 에러 분류, 재시도 동작, 그리고 사용자에게 노출되는 메시지를 정리한다. 폐쇄망 배포 관점의 "외부 호출 fatal 여부" 도 마지막에 같이 적었다.

핵심 파일:

- `codex-rs/codex-api/src/error.rs` — API 레이어 에러 enum (`ApiError`).
- `codex-rs/codex-api/src/api_bridge.rs` — `ApiError` → `CodexErr` 변환 (`map_api_error`).
- `codex-rs/codex-client/src/error.rs`, `codex-rs/codex-client/src/retry.rs` — transport 에러 / retry 정책.
- `codex-rs/codex-api/src/sse/responses.rs`, `codex-rs/codex-api/src/sse/chat.rs` — SSE 파서, idle timeout, `response.failed`/`incomplete` 처리.
- `codex-rs/codex-api/src/rate_limits.rs` — rate-limit 헤더 / 이벤트 파서.
- `codex-rs/codex-api/src/endpoint/session.rs`, `codex-rs/codex-api/src/telemetry.rs` — request-level retry 진입점.
- `codex-rs/model-provider-info/src/lib.rs` — `request_max_retries` / `stream_max_retries` 상수.
- `codex-rs/protocol/src/error.rs` — `CodexErr`, `is_retryable`, `UsageLimitReachedError`, `RetryLimitReachedError`.
- `codex-rs/core/src/session/turn.rs` — turn-level stream-retry 루프, `notify_stream_error`.
- `codex-rs/core/src/util.rs` — turn-level `backoff()`.
- `codex-rs/core/src/function_tool.rs` — `FunctionCallError` (tool 실패 분류).
- 참고 테스트: `codex-rs/core/tests/suite/client.rs`, `codex-rs/core/tests/suite/stream_no_completed.rs`.

---

## 1. 에러 분류

세 개의 레이어를 통과하면서 점차 좁아진다.

```
codex-client (TransportError) → codex-api (ApiError) → core (CodexErr)
```

### 1.1 Transport layer — `TransportError`

`codex-rs/codex-client/src/error.rs`:

| variant            | 트리거                                                           |
| ------------------ | -------------------------------------------------------------- |
| `Http { status, headers, body, url }` | 4xx/5xx 응답이 그대로 도달 (raw)             |
| `Timeout`          | request-level timeout. tokio `timeout` 만료                    |
| `Network(String)`  | reqwest connect/read 실패, DNS, TLS, EOF 등                     |
| `Build(String)`    | 요청 빌드 단계 실패 (헤더/URL 인코딩 등). 재시도 안 됨            |
| `RetryLimit`       | retry 루프에서 모든 시도 실패 시 마지막에 반환                    |

### 1.2 API layer — `ApiError`

`codex-rs/codex-api/src/error.rs`. SSE 스트림 본문을 까서 의미별로 좁힌 형태:

- `Transport(TransportError)` — 위 그대로 wrapping.
- `Stream(String)` — SSE 가 끝까지 못 가고 끊김 / `response.incomplete` / parsing 실패.
- `Retryable { message, delay: Option<Duration> }` — `response.failed` 인데 알려진 재시도 가능 사유 (예: rate limit). `delay` 가 있으면 그 값으로 sleep.
- `ContextWindowExceeded`, `QuotaExceeded`, `UsageNotIncluded`, `ServerOverloaded`, `CyberPolicy`, `InvalidRequest`, `RateLimit`, `Api { status, message }` — non-retryable / 분기용.

### 1.3 Core layer — `CodexErr` (`codex-rs/protocol/src/error.rs`)

`map_api_error` (`codex-api/src/api_bridge.rs:17`) 가 변환한다. 핵심 분기:

| HTTP 상태 / 신호                                | `CodexErr` 결과                                     | 재시도 |
| ----------------------------------------------- | -------------------------------------------------- | ------ |
| 400 + body 에 `cyber_policy`                    | `CyberPolicy { message }`                           | X      |
| 400 + "image data ... not represent a valid"    | `InvalidImageRequest`                               | X      |
| 400 그 외                                        | `InvalidRequest(body)`                              | X      |
| 401 / 403                                        | `UnexpectedStatus(...)` (Cloudflare blocked 메시지 처리) | X      |
| 429 + body `usage_limit_reached`                 | `UsageLimitReached(UsageLimitReachedError)`          | X      |
| 429 + body `usage_not_included`                  | `UsageNotIncluded`                                  | X      |
| 429 그 외                                        | `RetryLimit(RetryLimitReachedError)`                | -      |
| 500                                              | `InternalServerError`                               | O      |
| 503 + `server_is_overloaded`/`slow_down`         | `ServerOverloaded`                                  | X      |
| 5xx 그 외                                        | `UnexpectedStatus(...)`                             | O      |
| `TransportError::Timeout`                        | `CodexErr::Timeout`                                 | O      |
| `TransportError::Network(_) / Build(_)`          | `CodexErr::Stream(msg, None)`                       | O      |
| SSE 끊김                                         | `CodexErr::Stream(msg, requested_delay)`            | O      |

`CodexErr::is_retryable()` (`protocol/src/error.rs:170`) 가 turn 루프에서 재시도 여부를 결정한다. **명시적으로 `false` 인 것**: `ContextWindowExceeded`, `QuotaExceeded`, `UsageNotIncluded`, `UsageLimitReached`, `ServerOverloaded`, `CyberPolicy`, `InvalidRequest`, `InvalidImageRequest`, `RefreshTokenFailed`, `Sandbox`, `RetryLimit`, `Fatal`, env-var 누락, abort/interrupt 등. **`true`**: `Stream`, `Timeout`, `UnexpectedStatus`, `ResponseStreamFailed`, `ConnectionFailed`, `InternalServerError`, `Io`, `Json`, `TokioJoin`.

요점: **HTTP 4xx 는 (429 의 일부 케이스 제외) 거의 모두 non-retryable** 이다. 5xx 는 기본적으로 재시도. 4xx 가 client error 로서 즉시 fail-fast 하는 정책이 `RetryOn` 과 `is_retryable` 양쪽 모두에 박혀 있다.

---

## 2. 재시도 로직

두 개의 독립된 재시도 레이어가 있다. **한 번의 turn 동안 최악의 경우 `(request_max_retries+1) * (stream_max_retries+1)` 번까지 호출이 발생할 수 있다.**

### 2.1 Request-level — `request_max_retries` (HTTP handshake 까지)

위치: `codex-rs/codex-client/src/retry.rs::run_with_retry`.

- 정책: `RetryPolicy { max_attempts, base_delay, retry_on }`.
- `model-provider-info` 가 `to_api_provider` 에서 만들어 주입한다 (`model-provider-info/src/lib.rs:254`):
  - `retry_429: false`, `retry_5xx: true`, `retry_transport: true`.
  - `base_delay: 200 ms`.
  - `max_attempts = request_max_retries`. 디폴트 `4`, 하드캡 `100` (`MAX_REQUEST_MAX_RETRIES`).
- backoff: `base * 2^(attempt-1)` 에 `0.9..1.1` jitter 곱셈 (`retry.rs:38`).
- 즉 attempts 0–4 (총 5회) 시 sleep 시퀀스 ≈ 200ms · 400ms · 800ms · 1.6s · 3.2s (±10%).
- 호출 진입점: `EndpointSession::execute_with` / `stream_with` (`codex-api/src/endpoint/session.rs`) 가 `run_with_request_telemetry` 를 통해 자동으로 감싼다.

이 레이어는 **HTTP handshake 가 성공하기 전까지** 의 영역이다. 일단 200 OK 가 와서 SSE 가 시작되면 이 retry 는 더 이상 동작하지 않는다.

### 2.2 Stream-level — `stream_max_retries` (HTTP 200 후 SSE 끊김)

위치: `codex-rs/core/src/session/turn.rs::run_sampling_request` (라인 ~1032–1131).

- 디폴트 `5`, 하드캡 `100` (`DEFAULT_STREAM_MAX_RETRIES`).
- backoff: `core/src/util.rs::backoff` — `INITIAL_DELAY_MS=200`, `BACKOFF_FACTOR=2.0`, ±10% jitter. attempt n 에서 `200 * 2^(n-1)` ms.
- **단, `CodexErr::Stream(msg, Some(delay))` 인 경우 server 가 알려준 delay 가 우선**. SSE 안의 rate-limit 메시지 (`Try again in ...`) 또는 `response.failed` 의 명시 delay 가 여기로 들어온다.
- 매 retry 직전에:
  1. `err.is_retryable()` 확인 — false 면 즉시 propagate.
  2. `tracing::warn!("stream disconnected - retrying sampling request ({retries}/{max_retries} in {delay:?})...")` 로그 (`turn.rs:1107`).
  3. retries==0 인 첫 retry 는 release 빌드에서 사용자 noti 를 숨김 (debug 빌드에서는 항상 노출). websocket 전송을 사용하지 않으면 첫 회부터 노출.
  4. 그 외에는 `notify_stream_error` 가 `EventMsg::StreamError(StreamErrorEvent { message: "Reconnecting... {n}/{max}", codex_error_info: ResponseStreamDisconnected, additional_details: <CodexErr Display> })` 를 emit (`session/mod.rs:2950`). TUI 는 이 메시지를 그대로 보여 준다 (테스트 fixture 예: `tui/src/chatwidget/tests/history_replay.rs:874` `"Reconnecting... 2/5"`).
  5. `tokio::time::sleep(delay).await` 후 같은 prompt 로 다시 호출.
- WebSocket 운영 중이라면 retry 가 max 에 도달했을 때 `try_switch_fallback_transport` 로 HTTPS 전송으로 다운그레이드를 시도하고 카운터를 0 으로 리셋한다 (`turn.rs:1083`). 폐쇄망 배포에선 websocket 미사용이므로 이 분기는 실행되지 않는다.

---

## 3. Rate limit 처리

### 3.1 헤더 기반 — `parse_rate_limit_for_limit`

`codex-rs/codex-api/src/rate_limits.rs:56`. 모든 응답 헤더에서 `x-codex-{limit}-primary-used-percent` / `-window-minutes` / `-reset-at` / `-secondary-*` / `x-codex-credits-*` 를 긁어 `RateLimitSnapshot` 으로 만든다. `parse_all_rate_limits` 는 multi-tier (예: `codex` + `codex_secondary`) 도 모두 수집.

`map_api_error` 의 429 분기는:

1. body 가 `usage_limit_reached` 면 헤더에서 `x-codex-active-limit` 를 읽어 해당 limit 의 `RateLimitSnapshot` 을 함께 묶어 `UsageLimitReachedError` 를 반환한다 (`api_bridge.rs:80`).
2. 호출자 (`core/src/session/turn.rs:1067`) 가 이를 받아 `sess.update_rate_limits(...)` 로 세션 상태에 저장하고 `EventMsg::TokenCount(TokenCountEvent { rate_limits })` 를 emit (테스트 검증: `core/tests/suite/client.rs:2641`).
3. 사용자 메시지: `UsageLimitReachedError::Display` 가 plan_type / promo_message / `resets_at` 에 따라 "You've hit your usage limit. ... Try again at 3:42 PM." 식으로 포맷 (`protocol/src/error.rs:453`).

### 3.2 SSE 인라인 — `response.failed { code = "rate_limit_exceeded" }`

스트림이 시작된 뒤 서버가 rate-limit 을 통보하는 경로. `try_parse_retry_after` (`codex-api/src/sse/responses.rs:521`) 가 메시지에서 `try again in <N> (s|ms|seconds?)` 정규식을 추출 → `Duration` 을 `ApiError::Retryable.delay` 에 채워 보낸다. `map_api_error` 가 이걸 `CodexErr::Stream(msg, Some(delay))` 로 옮기고, turn-level retry 가 이 delay 만큼 sleep 후 재시도.

**주의: HTTP `Retry-After` 헤더 자체를 읽는 경로는 현재 없다.** rate-limit 정보는 (a) `x-codex-*` 패밀리 헤더, (b) SSE 본문 메시지 정규식 둘 중 하나로만 들어온다.

### 3.3 `codex.rate_limits` SSE 이벤트

`parse_rate_limit_event` (`rate_limits.rs:131`) 가 정상 흐름 중간의 `codex.rate_limits` push 를 파싱해 `RateLimitSnapshot` 으로 만들고, 세션이 `EventMsg::TokenCount` 로 다시 노출한다. UI 측면에서 "한도가 거의 다 찼습니다" 같은 사전 경고에 쓰인다.

---

## 4. Stream 중단 / 재시작 — 로그 흐름 예

`stream disconnected - retrying sampling request` 가 떨어지는 케이스:

1. **`response.incomplete`** — 서버가 partial 결과만 보내고 끝남. `process_responses_event` 가 `ApiError::Stream(format!("Incomplete response returned, reason: {reason}"))` 반환 (`sse/responses.rs:381`). reason 이 `content_filter` 인 사례가 테스트에 있다 (`core/tests/suite/client.rs:2820`).
2. **idle timeout** — `process_sse` / `process_chat_sse` 가 `tokio::time::timeout(idle_timeout, stream.next())` 으로 폴링하다가 `Err(_)` → `ApiError::Stream("idle timeout waiting for SSE")` 반환 (`sse/responses.rs:463`, `sse/chat.rs:130`). idle 디폴트는 300 s (`DEFAULT_STREAM_IDLE_TIMEOUT_MS`).
3. **EOF before completed** — SSE 가 `[DONE]` / `response.completed` 없이 닫힘. `Ok(None)` → `ApiError::Stream("stream closed before response.completed")` (`sse/responses.rs:457`).
4. **transport 에러 도중 발생** — `Ok(Some(Err(e)))` → `ApiError::Stream(e.to_string())`.
5. **`response.failed`** — 서버가 명시적 에러 이벤트를 보낸 경우. 위 §1.2 / §3.2 분류로 분기.

테스트에서 본 흐름 (`core/tests/suite/stream_no_completed.rs`, `stream_max_retries=1`):

```
1차 attempt   → 서버가 incomplete SSE 송신 → ApiError::Stream("Incomplete response ...")
                → CodexErr::Stream(_, None) → is_retryable=true
                → warn!("stream disconnected - retrying sampling request (1/1 in ~200ms)...")
                → notify_stream_error → EventMsg::StreamError("Reconnecting... 1/1")
                → sleep 200ms
2차 attempt   → 정상 SSE → ResponseEvent::Completed → EventMsg::TurnComplete
```

실제 폐쇄망 Qwen 게이트웨이에서 사용자가 본 `Reconnecting... N/5` 토스트가 정확히 이 path 다. idle 300s 동안 게이트웨이가 KEEP-ALIVE 만 보내고 응답 없을 때, 또는 nginx 가 504 로 끊을 때 1번이 점화된다.

---

## 5. Tool call 실패

Tool 실행 자체가 실패해도 turn 은 죽지 않는다. `codex-rs/core/src/function_tool.rs::FunctionCallError` 가 세 가지로 분류:

| variant                       | 의미                                                          | 모델에 보고                          |
| ----------------------------- | ------------------------------------------------------------ | ----------------------------------- |
| `RespondToModel(String)`      | tool 자체가 실패했지만 turn 은 계속. 메시지를 모델에 다시 넘김  | `ResponseInputItem::FunctionCallOutput { output: <message> }` 로 history 에 push, `needs_follow_up=true` 로 다음 sampling 호출 트리거 |
| `MissingLocalShellCallId`     | `LocalShellCall` 이 call_id 없이 옴 (모델 버그)                | 동일하게 FunctionCallOutput 에 에러 텍스트 넣어 모델이 재요청하도록 유도         |
| `Fatal(String)`                | turn 을 죽여야 하는 내부 에러                                  | `CodexErr::Fatal(message)` 로 propagate, retry 없음                              |

처리 위치: `core/src/stream_events_utils.rs:288–342`. 결과 텍스트는 그대로 다음 prompt 의 input 으로 합류해 모델이 "이 도구가 왜 실패했는지" 보고 보정하도록 한다.

대표 예:

- **shell 실행 실패** (`run_command_stream` 의 `CodexErr::Spawn` / sandbox denied 등) — sandbox 거부는 `CodexErr::Sandbox(SandboxErr::Denied)` 로 떨어지고, tool runtime 이 사람-친화 메시지를 만들어 `RespondToModel` 로 변환. exit code, stdout/stderr 가 합쳐진 텍스트가 모델로 돌아간다 (`get_error_message_ui`, `protocol/src/error.rs:597`).
- **apply_patch 충돌** — `core/src/apply_patch.rs:71` `RespondToModel(format!("patch rejected: {reason}"))`. 모델은 reason 을 보고 다시 패치를 수정해서 보낸다.
- **MCP 도구 timeout / 거부** — `tools/registry.rs:329` 등이 모두 `RespondToModel` 로 정규화.

즉 tool 영역에서는 **"실패는 모델 입장에서 또 다른 입력"** 으로 취급하고, retry 카운터를 따로 두지 않는다. 모델이 같은 도구를 무한히 다시 부르는 것은 turn-level 의 다른 안전장치 (예: tool budget) 가 막는다.

---

## 6. 에러 격리 / 컨벤션

### 6.1 `anyhow::Result` vs custom enum

- **API / transport / SSE 경계**: 무조건 custom enum. `ApiError`, `TransportError`, `CodexErr` 가 `thiserror::Error` 로 정의된다. `anyhow::Error` 는 이 레이어에 없다. variant 기반이라 `is_retryable()` / `to_codex_protocol_error()` 같은 분기가 가능.
- **테스트 코드**: `anyhow::Result<()>` 로 자유롭게 받음 (`core/tests/suite/client.rs:2580` `async fn ... -> anyhow::Result<()>`).
- **CLI / exec 진입점 / 헬퍼**: 외부 IO 를 묶을 때만 `anyhow::Context` 사용. 내부 코어 로직은 항상 `Result<T, CodexErr>` 또는 `Result<T, FunctionCallError>`.
- `core/src/util.rs::error_or_panic` 가 debug 빌드에서는 panic, release 에서는 `tracing::error!` 만 — invariant 위반 시 dev 환경에서 빨리 잡고 prod 에서는 살아남는 정책.

### 6.2 panic 안 가게 하는 곳

- SSE 처리 task 는 `tokio::spawn` 으로 분리되어 있어 (`codex-api/src/sse/chat.rs:35`, `codex-api/src/sse/responses.rs:spawn_response_stream`) 파싱 실패해도 main turn loop 까지 panic 이 올라가지 않는다. 채널 send 실패 (`tx_event.send(...).await.is_err()`) 도 graceful return.
- workspace clippy 가 `unwrap_used`, `expect_used` 를 `deny` 로 박아서 production code 에서는 거의 못 쓴다 (`codex-rs/Cargo.toml [workspace.lints.clippy]`). 정규식 등 `OnceLock` 초기화에는 `#[expect(clippy::unwrap_used)]` 로 명시 (`sse/responses.rs:584`).
- 외부 task panic 은 `JoinError` (`CodexErr::TokioJoin`) 로 떨어져 `is_retryable=true` 가 되어 turn-level retry 로 흡수된다.

---

## 7. 운영 관점 — 폐쇄망에서 외부 호출 fatal 여부

`fork-docs/airgap-audit-2026-05-08.md` 의 결과를 에러-경로 관점에서 다시 정리:

| 외부 호출                                              | fatal? | 사용자에게 보이는 증상                                                |
| ----------------------------------------------------- | ------ | -------------------------------------------------------------------- |
| Statsig OTLP metrics (`ab.chatgpt.com/otlp/...`)       | **non-fatal**. OTLP exporter 가 5–30s 단위로 재시도, 모두 connection error 로 떨어지지만 turn 진행에 영향 없음. 로그 noisy. |
| Curated plugin sync (`github.com/openai/plugins.git`) | **non-fatal but UX-blocking**. startup 시 long timeout 까지 hang → tracing warn. session 이 시작되긴 한다. |
| Featured plugin GET (`chatgpt.com/backend-api/plugins/...`) | **non-fatal**. 401/네트워크 에러 → warn 로그, `featured_plugin_ids` 가 비어 있는 채로 진행. |
| Update check (`api.github.com/.../codex/releases/latest`) | **non-fatal**. 백그라운드. 실패해도 TUI 동작 정상. |
| Agent identity JWKS (`chatgpt.com/.../jwks`)          | API key (Ollama) 사용자는 호출하지 않음. ChatGPT 로그인 경로에서만 fatal-ish (인증 실패 → `RefreshTokenFailed`, non-retryable). |
| LLM 호출 (사내 Qwen 게이트웨이)                       | **fatal**. 게이트웨이가 다운이면 `request_max_retries` 까지 backoff 후 `CodexErr::RetryLimit` 또는 `UnexpectedStatus` 로 turn 종료. |

LLM 외 호출 중 **`CodexErr::Fatal` 로 변환되어 turn 을 즉시 죽이는 경우는 현재 없다**. 모두 `tracing::warn!` + degraded-mode 진행이 디폴트라, 폐쇄망에서 외부 도메인이 막혀도 사용자는 (a) noisy log, (b) startup 시 약간의 hang 정도만 본다. 단, P0 항목은 사용 사실 telemetry 가 외부로 새는 문제이므로 *동작* 이 아니라 *보안* 관점에서 차단해야 한다 (audit 문서의 권장 조치 참고).

---

## 부록 — 재시도 동작 빠른 매트릭스

```
┌─────────────────────────┬────────────────┬─────────────────────┬──────────────────┐
│ 사건                     │ 잡히는 위치     │ 재시도 카운터       │ 사용자 메시지     │
├─────────────────────────┼────────────────┼─────────────────────┼──────────────────┤
│ DNS / connect 실패       │ codex-client    │ request_max_retries │ (조용히 retry)    │
│ 5xx 응답                 │ codex-client    │ request_max_retries │ (조용히 retry)    │
│ 4xx 응답 (cyber/invalid) │ map_api_error   │ 없음 (즉시 fail)     │ 에러 toast        │
│ 429 + usage_limit        │ map_api_error   │ 없음                │ "You've hit ..."  │
│ 429 그 외                │ map_api_error   │ 없음 → RetryLimit   │ "exceeded retry"  │
│ SSE idle timeout         │ sse/*.rs       │ stream_max_retries  │ "Reconnecting..." │
│ SSE 중간 EOF             │ sse/*.rs       │ stream_max_retries  │ "Reconnecting..." │
│ response.incomplete      │ sse/responses  │ stream_max_retries  │ "Reconnecting..." │
│ response.failed (rate)   │ sse/responses  │ stream_max_retries  │ "Reconnecting..." │
│ response.failed (quota)  │ sse/responses  │없음 (QuotaExceeded) │ "Quota exceeded"  │
│ tool 실행 실패           │ stream_events_  │ 없음 (다음 turn 으로)│ 모델이 보정 메시지│
│                          │ utils.rs       │                     │                  │
│ tool Fatal               │ stream_events_  │ 없음 (turn 종료)     │ "Fatal error: ..."│
│                          │ utils.rs       │                     │                  │
└─────────────────────────┴────────────────┴─────────────────────┴──────────────────┘
```
