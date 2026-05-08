# OpenAI → 원격 Ollama 디폴트 전환

## Context

이 fork 의 LLM 호출을 OpenAI 가 아니라 **원격 Ollama 엔드포인트의 `qwen3.5-27b`** 로 보내는 것이 목표다. 별도 설정 파일이나 `--oss` 플래그 없이도 `codex` 명령만 치면 즉시 원격 Ollama 로 라우팅되어야 한다 — 즉 **코드 디폴트 자체를 바꾼다**.

다행히 Codex 는 이미 Ollama 를 1급 빌트인 provider 로 지원한다 (`codex-rs/model-provider-info/src/lib.rs:399, 416-419`, `codex-rs/ollama/`). 따라서 이 작업은 **새 통합 구현이 아니라 디폴트 라우팅·인증·모델 핀(pin) 변경**이다.

## 대상 엔드포인트 (사용자 opencode 설정에서 가져옴)

opencode 의 `@ai-sdk/openai-compatible` provider 로 다음과 같이 사용 중:

```jsonc
"ollama-remote-27b": {
  "npm": "@ai-sdk/openai-compatible",
  "options": {
    "baseURL": "http://10.250.121.100/v1",
    "apiKey": "sk-davis-27b-22222"
  },
  "models": { "qwen3.5-27b": { "name": "Qwen3.5 27B (Remote)" } }
}
```

| 항목 | 값 | 반영 위치 |
| --- | --- | --- |
| 호스트/baseURL | `http://10.250.121.100/v1` | 환경변수 `CODEX_OSS_BASE_URL` (코드/Git 에 박지 않음) |
| 모델 태그 | `qwen3.5-27b` | `codex-rs/ollama/src/lib.rs:16` `DEFAULT_OSS_MODEL` |
| 인증 토큰 | `sk-davis-27b-22222` | 환경변수 `OLLAMA_API_KEY` (코드/Git 에 박지 않음) |

토큰을 코드/리포지토리에 박지 않는 이유: Git 히스토리에 남으면 즉시 폐기 대상이 된다. 호스트는 사내 IP(10.x) 라 외부 fork 노출 위험이 더 적지만 그래도 환경변수 경로가 깨끗하다.

### Wire API 호환성 — 게이트웨이 확인 결과

`http://10.250.121.100/` 는 nginx/1.29.4 로 응답하고 OpenAI 호환 `/v1/*` 라우트를 노출한다. 무인증 / 잘못된 키로 `/v1/responses` 와 `/v1/chat/completions` 둘 다 `401 auth_error` 를 반환한다. **`/v1/responses` 가 404 가 아닌 401 이라는 것은 라우트가 살아있다는 강한 신호** — Codex 의 Responses API 와 그대로 호환 가능. 따라서 Codex 에 `wire_api = "chat"` 을 부활시킬 필요 없음.

키와 모델 매핑 주의: 키 라벨(`sk-davis-122b-...`)이 게이트웨이의 실제 권한 매핑과 다를 수 있다. 실험에서 122b 모델로 요청해도 응답이 `Invalid API key for model: qwen3.5-27b` 로 떨어졌는데, 이는 게이트웨이가 키→모델 강제 매핑을 갖고 있고 이 키가 27b 에 묶여 있다는 의미. **사용 가능한 모델은 키 발급자에게 확인 필요**. 이 fork 의 `DEFAULT_OSS_MODEL` 은 일단 `qwen3.5-27b` 로 박혀 있다.

게이트웨이 endpoint 확인을 다시 하고 싶다면 (사용자 셸에서 실행):

```bash
KEY='<당신의 토큰>'
curl -sS -o /tmp/r -w "HTTP %{http_code}\n" -X POST \
  http://10.250.121.100/v1/responses \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"model":"qwen3.5-27b","input":"reply: pong","stream":false}'
head -c 1500 /tmp/r; unset KEY
```

200 + JSON 본문이 떨어지면 OK. 401 이라도 메시지가 모델 권한 문제이면 Codex 측은 정상이고 키/모델 매핑만 조정하면 된다.

## 변경 지점 (전부 `codex-rs/` 하위)

### A. 기본 provider 를 `ollama` 로 변경

**파일**: `codex-rs/core/src/config/mod.rs:2633`

```rust
// before
.unwrap_or_else(|| "openai".to_string());

// after
.unwrap_or_else(|| codex_model_provider_info::OLLAMA_OSS_PROVIDER_ID.to_string());
```

`OLLAMA_OSS_PROVIDER_ID` 상수는 `codex-rs/model-provider-info/src/lib.rs:399` 에 이미 정의되어 있음 (`"ollama"`).

### B. 디폴트 OSS 모델을 `qwen3.5-27b` 로 변경

**파일**: `codex-rs/ollama/src/lib.rs:16`

```rust
// before
pub const DEFAULT_OSS_MODEL: &str = "gpt-oss:20b";

// after
pub const DEFAULT_OSS_MODEL: &str = "qwen3.5-27b";
```

영향 경로:
- `codex-rs/utils/oss/src/lib.rs:11` 의 `get_default_model_for_oss_provider`
- `codex-rs/ollama/src/lib.rs:22-49` 의 `ensure_oss_ready` 자동 pull 로직

원격 호스트가 모델을 보유하지 않을 때 자동 pull 은 실패할 수 있는데, `ensure_oss_ready` 는 fetch_models 실패 시 경고만 로그하고 진행 (`ollama/src/lib.rs:42-45`) 하므로 치명적이지 않다.

### C. Ollama provider 정의에 인증 토큰 지원 추가

**파일**: `codex-rs/model-provider-info/src/lib.rs:487-507` 부근 (`create_oss_provider_with_base_url`)

현재 ollama provider 는 `env_key: None` 으로 인증 헤더가 없다. Bearer 토큰을 환경변수로 주입할 수 있도록 ollama 전용 생성 함수를 분리한다.

```rust
// 개념도
pub fn create_ollama_provider() -> ModelProviderInfo {
    let base_url = resolve_oss_base_url(DEFAULT_OLLAMA_PORT);
    ModelProviderInfo {
        name: "ollama".into(),
        base_url: Some(base_url),
        env_key: Some("OLLAMA_API_KEY".into()),  // ← 신규: Bearer 인증
        env_key_instructions: Some(
            "Set OLLAMA_API_KEY to your remote Ollama bearer token (e.g. sk-...).".into()
        ),
        wire_api: WireApi::Responses,
        requires_openai_auth: false,
        ..Default::default()
    }
}
```

`built_in_model_providers` (`lib.rs:402-428`) 에서 ollama 항목을 위 함수 호출로 교체한다. lmstudio 는 기존 `create_oss_provider` 로 그대로 둔다.

`env_key` 가 설정되면 `codex-api` 클라이언트가 자동으로 `Authorization: Bearer $OLLAMA_API_KEY` 헤더를 붙이므로 **HTTP 클라이언트 코드는 손댈 필요 없다**.

### D. `--oss` 플래그 없이도 자동 readiness 체크

현재: `codex-rs/exec/src/lib.rs:367-395, 590` 와 `codex-rs/tui/src/lib.rs:816-845, 1020` 에서 `cli.oss == true` 일 때만 `ensure_oss_provider_ready` 가 호출된다. ollama 가 디폴트가 되면 일반 `codex` 흐름에서도 health-check 가 돌아야 한다.

변경: 두 진입점에서 트리거 조건을 `config.model_provider_id ∈ {ollama, lmstudio}` 로 확장. 기존 헬퍼 (`ensure_oss_provider_ready`, `get_default_model_for_oss_provider`) 그대로 재사용.

### F. 원격 게이트웨이에서도 readiness 가 안전하도록 보강

위 D 를 활성화하면, `OllamaClient` 가 Ollama admin endpoint (`/api/tags`, `/api/pull`) 를 두드린다. 그러나 nginx 같은 OpenAI 호환 게이트웨이는 그 라우트가 없어 매번 codex 시작이 fatal 로 떨어진다. 두 군데를 보수적으로 수정:

1. **`codex-rs/ollama/src/client.rs` 의 `probe_server`**: 모든 HTTP 응답(401/403/404 포함) 을 "도달 OK" 로 간주. transport-level connection error 만 fatal. 인증 가드된 `/v1/models` 가 401 을 던져도 codex 는 진행.
2. **`codex-rs/ollama/src/lib.rs` 의 `ensure_oss_ready`**: `fetch_models()` 가 빈 리스트를 반환하면 (게이트웨이가 `/api/tags` 를 노출하지 않는 케이스) 자동 pull 시도를 건너뛰고 사용자의 모델 선택을 신뢰. 실제 라우팅 실패는 `/v1/responses` 호출 시점에 자연스럽게 표면화된다.

테스트:
- `test_try_from_oss_provider_err_when_server_unreachable` (port 1 사용) — connection error 만 fatal 인지 검증.
- `test_probe_treats_http_response_as_reachable` — wiremock 으로 401 응답 시뮬, probe 가 통과해야 함.

### E. 문서 / 테스트 갱신

- `codex-rs/config.md`, `docs/config.md`: 디폴트 provider 가 `openai` → `ollama` 로 바뀐 점, `OLLAMA_API_KEY` / `CODEX_OSS_BASE_URL` 환경변수 설명 추가.
- `model-provider-info/src/lib.rs` 하단 테스트 — `built_in_model_providers` 에서 ollama provider 가 `env_key=Some("OLLAMA_API_KEY")` 를 갖는지 확인 케이스 추가.
- `core/src/config/` 의 default provider 테스트가 있다면 `"openai"` 기대값을 `"ollama"` 로 갱신.

## 운영 절차

```bash
export CODEX_OSS_BASE_URL=http://10.250.121.100/v1
export OLLAMA_API_KEY=sk-davis-27b-22222
codex                                                # 별도 플래그 없이 즉시 동작
```

토큰 보관: `~/.zshrc` / `~/.bashrc` 직접 박기 vs 1Password / direnv / `op run` 등 비밀 관리자 사용. 사내 운영 정책을 따르고, 절대 `~/.codex/config.toml` 의 `experimental_bearer_token` 필드에는 박지 않는다 (Git/스냅샷에 노출 위험).

## 검증 방법

### 1. 빌드 / 포맷 / lint

```bash
cd codex-rs
just fmt
just fix -p codex-config -p codex-model-provider-info -p codex-ollama
```

### 2. 단위 테스트

```bash
cargo test -p codex-model-provider-info  # built-in provider 검증
cargo test -p codex-config               # default model_provider 분기
cargo test -p codex-ollama               # DEFAULT_OSS_MODEL 의존 테스트
cargo test -p codex-utils-oss
```

### 3. 엔드투엔드

```bash
export CODEX_OSS_BASE_URL=http://10.250.121.100/v1
export OLLAMA_API_KEY=sk-davis-27b-22222
RUST_LOG=codex_api=debug cargo run --bin codex -- exec "say hi"
```

확인 항목:
- `Authorization: Bearer sk-...` 헤더가 나가는지 (debug 로그)
- `POST /v1/responses` 가 호출되는지
- OpenAI 키 없이 정상 응답이 도착하는지

### 4. 회귀 (환경변수 미설정)

```bash
unset CODEX_OSS_BASE_URL OLLAMA_API_KEY
codex
```

기대: `localhost:11434` 로 가서 Ollama 미실행 안내 / 연결 실패 메시지가 깔끔히 표시. **`OPENAI_API_KEY` 를 묻는 화면이 뜨면 안 됨** (디폴트 분기가 ollama 로 바뀌었는지 검증).

## 핵심 파일 요약

| 파일 | 역할 |
| --- | --- |
| `codex-rs/core/src/config/mod.rs:2633` | 디폴트 provider 분기 |
| `codex-rs/model-provider-info/src/lib.rs:402-507` | 빌트인 provider 카탈로그, ollama 정의 |
| `codex-rs/ollama/src/lib.rs:16` | `DEFAULT_OSS_MODEL` 상수 |
| `codex-rs/utils/oss/src/lib.rs:8-38` | OSS 모델 디폴트·readiness 헬퍼 |
| `codex-rs/exec/src/lib.rs:367-395, 590` | exec 진입점의 OSS 흐름 |
| `codex-rs/tui/src/lib.rs:816-845, 1020` | TUI 진입점의 OSS 흐름 |
| `codex-rs/config.md`, `docs/config.md` | 사용자 문서 |

## 작업량 추정

- 코드 변경: 5개 파일, ~50 LoC
- 테스트 갱신: 2~3개 파일
- 문서: `docs/config.md` + 이 fork-docs 한 편
- 모두 `codex-core` 외부에서 처리 가능 (`AGENTS.md` 의 "resist adding to codex-core" 가이드 준수)

## upstream 반영 가능성

- A (디폴트 변경) / B (모델 변경): **fork 한정.** upstream 은 OpenAI 디폴트 유지 정책이므로 PR 대상 아님.
- C (OLLAMA 인증 토큰 지원): **upstream 후보.** 원격 Ollama (인증 게이트웨이 뒤에 둔 사내 호스팅 등) 는 일반적인 요구라 일반화하면 받을 가능성 있음.
- D (`--oss` 없이 readiness 자동 호출): **fork 한정.** upstream 의 명시적 OSS 옵트인 디자인을 깨뜨리는 변경.
