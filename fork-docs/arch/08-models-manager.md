# 08 — models-manager 서브시스템

이 문서는 `xtech` 가 **모델 카탈로그**(`/models` 응답에 해당하는 `ModelInfo[]`) 를 어떻게 가져오고, 캐시하고, picker / 기본 모델 선택 / guardian review 같은 다운스트림에 노출하는지 정리한다. 핵심 코드는 `codex-rs/models-manager/` (5개 모듈) 와 `codex-rs/protocol/src/openai_models.rs` 두 곳에 모여 있다.

설정·provider 레이어 (`ModelProviderInfo`, `ChatGPT` 인증 등) 는 `06-config.md` / `03-wire-protocol.md` 가 다루고, 이 문서는 “카탈로그가 만들어지는 파이프라인” 자체에 집중한다.

## 1. 카탈로그 소스

세 가지 카탈로그 소스가 한 매니저 안에서 합쳐진다.

### 1.1 Bundled `models.json`

- 파일: `codex-rs/models-manager/models.json` (169 lines).
- `include_str!` 로 빌드타임에 박힌다 (`codex-rs/models-manager/src/lib.rs:15`).
- 진입 함수 `bundled_models_response() -> Result<ModelsResponse, serde_json::Error>` 가 한 번 파싱해 `ModelsResponse` 를 돌려준다 (`lib.rs:13-16`).
- **fork 변경**: 이 fork 는 upstream 의 GPT-5 / GPT-5-codex / o-series / `gpt-5.2-codex` 등 모든 슬러그를 잘라내고, 다음 두 항목만 남겼다.

| slug | display_name | visibility | priority | 비고 |
| --- | --- | --- | --- | --- |
| `qwen3.5-122b` | Qwen3.5 122B | `list` | 0 | fork 의 사실상 유일한 사용자 노출 모델 (사내 게이트웨이) |
| `codex-auto-review` | Codex Auto Review | `hide` | 29 | guardian 자동 리뷰 전용 슬러그 (picker 비노출) |

`visibility` 는 `protocol/src/openai_models.rs:170` 의 `ModelVisibility { List, Hide, None }` enum. `List` 만 picker 에 뜨고, `Hide` 는 “API 로는 살아있지만 picker 에 안 보임” — 자세한 의미는 §3 / §7 참조.

### 1.2 Remote `/models` refresh

- 클라이언트: `codex-rs/model-provider/src/models_endpoint.rs:81-109` 의 `OpenAiModelsEndpoint::list_models`.
- URL: `{provider.base_url}/models` (5초 timeout — `models_endpoint.rs:31`).
- fork 의 ollama provider 는 사내 게이트웨이를 가리키므로 외부 인터넷으로 빠지지 않는다 (§6).
- `(Vec<ModelInfo>, Option<String>)` 를 반환 — 두 번째가 ETag.

### 1.3 ETag 기반 머지

- 매니저는 응답을 통째로 교체하지 않고 **slug 단위 upsert** 한다: `OpenAiModelsManager::apply_remote_models` (`manager.rs:321-334`).
- 즉 bundled `models.json` 에 정의된 `codex-auto-review` 는 게이트웨이가 그 슬러그를 돌려주지 않아도 카탈로그에 그대로 남는다 — guardian 의존성을 보장한다 (§7).
- ETag 는 `RwLock<Option<String>>` 으로 들고 있다가 (`manager.rs:182`) `refresh_if_new_etag` 에 들어온 값과 비교한다 (`manager.rs:252-263`).

## 2. 로딩 흐름

런타임 카탈로그는 `OpenAiModelsManager` 가 두 단계로 만든다.

```
bundled models.json ──► bundled_models_response() ──► ModelsResponse
                                                        │ (ModelInfo[])
                                                        ▼
                       load_remote_models_from_file() ──► RwLock<Vec<ModelInfo>>
                                                        │
        ┌───────────────────────────────────────────────┤
        │                                               │
   (RefreshStrategy 분기 — §5)                          │
        │                                               │
        ▼                                               ▼
   list_models endpoint                            try_load_cache (디스크)
        │                                               │
        └──────────► apply_remote_models (slug upsert) ◄┘
                                                        │
                                                        ▼
                                             build_available_models
                                              (ModelPreset 변환 + 정렬 + 필터)
```

핵심 진입점은 `ModelsManager` trait 의 `list_models(refresh_strategy) -> Vec<ModelPreset>` (`manager.rs:80-90`) — 내부에서 `raw_model_catalog` → `build_available_models` 순서로 호출한다.

`build_available_models` (`manager.rs:107-119`) 가 `ModelInfo[] → ModelPreset[]` 변환·정렬·필터를 한 번에 한다.

1. `priority` 오름차순 정렬 (`manager.rs:108`).
2. `ModelPreset::from(ModelInfo)` 변환 — `protocol/src/openai_models.rs:443-476`. 변환 시 `show_in_picker = (visibility == List)` 로 굳어진다 (`openai_models.rs:470`).
3. `ModelPreset::filter_by_auth(presets, chatgpt_mode)` — ChatGPT 모드가 아니면 `supported_in_api == false` 인 프리셋은 빠진다 (`openai_models.rs:492-497`). fork 의 `qwen3.5-122b` / `codex-auto-review` 둘 다 `supported_in_api: true` 이므로 영향 없음.
4. `ModelPreset::mark_default_by_picker_visibility(&mut presets)` — §3.

`StaticModelsManager` (`manager.rs:189-193`) 는 1.2 / 1.3 / 캐시 단계를 모두 건너뛰고 “생성자에 받은 `ModelsResponse` 를 그대로” 노출한다. AWS Bedrock provider (`model-provider/src/amazon_bedrock/mod.rs:99`) 와 일부 테스트가 사용한다.

## 3. Default model 선택 로직

“이 사용자에게 보여줄 default 모델 슬러그” 는 두 단계로 결정된다.

### 3.1 `mark_default_by_picker_visibility`

`protocol/src/openai_models.rs:502-511`:

```rust
pub fn mark_default_by_picker_visibility(models: &mut [ModelPreset]) {
    for preset in models.iter_mut() { preset.is_default = false; }
    if let Some(default) = models.iter_mut().find(|p| p.show_in_picker) {
        default.is_default = true;          // picker-visible 첫 번째
    } else if let Some(default) = models.first_mut() {
        default.is_default = true;          // 그것도 없으면 그냥 첫 번째
    }
}
```

규칙은 단순하다. `priority` 정렬 이후의 `Vec` 에서 **picker 에 뜨는(=`visibility == List`) 첫 모델** 이 default. 모든 모델이 hidden 이면 (예: 게이트웨이가 hide-only 카탈로그를 돌려준 코너 케이스) 첫 항목이 default 가 된다.

fork 의 bundled 카탈로그에서는 `qwen3.5-122b` (priority 0, visibility List) 가 항상 첫 picker-visible 슬롯이라, default 도 항상 `qwen3.5-122b`. `codex-auto-review` 는 `Hide` 라 default 후보에서 자동 탈락.

### 3.2 `default_model_from_available`

`manager.rs:394-401`:

```rust
fn default_model_from_available(available: Vec<ModelPreset>) -> String {
    available.iter()
        .find(|m| m.is_default)
        .or_else(|| available.first())
        .map(|m| m.model.clone())
        .unwrap_or_default()
}
```

trait method `get_default_model(model: &Option<String>, refresh_strategy)` (`manager.rs:139-156`) 가 이 함수를 감싼다 — 사용자가 `Option<String>` 으로 명시적 슬러그를 주면 그대로, 아니면 `is_default` 프리셋의 `model` 필드를 돌려준다. `unwrap_or_default()` 라 카탈로그가 비어 있으면 빈 문자열이 떨어진다 (caller 가 fallback 책임).

### 3.3 `is_default` flag 와 picker visibility 의 관계

- `is_default` 와 `show_in_picker` 는 같은 함수에서 같이 결정되지만 **개념이 다르다**.
- `show_in_picker` 는 `ModelInfo::visibility` 가 `List` 인지로 변환 시점에 굳는다 (`openai_models.rs:470`).
- `is_default` 는 그 후 `mark_default_by_picker_visibility` 가 한 항목에만 켠다.
- picker UI 와 `get_default_model` 모두 `Vec<ModelPreset>` 한 벌을 공유 — TUI 모델 picker 가 보여주는 “highlight 된 default” 와 새 thread 가 자동 선택되는 슬러그가 동일하게 유지되는 이유.

## 4. Slug 매칭

“`config.toml` 의 `model = "qwen3.5-122b-thinking-low"` 같은 변형 슬러그가 들어왔을 때 어떤 `ModelInfo` 메타데이터를 쓸 것인가” 를 결정하는 로직이 두 함수로 갈라져 있다 (`manager.rs:403-437`).

### 4.1 `find_model_by_longest_prefix`

```rust
fn find_model_by_longest_prefix(model: &str, candidates: &[ModelInfo]) -> Option<ModelInfo>
```

후보를 순회하며 `model.starts_with(&candidate.slug)` 를 만족하는 것 중 **가장 긴 슬러그** 를 고른다. 즉 `qwen3.5-122b-fast` 가 들어오면 `qwen3.5-122b` 메타가 매칭되고, `qwen3.5` 정도의 짧은 prefix 는 더 긴 매치가 있을 때 밀린다.

### 4.2 `find_model_by_namespaced_suffix`

```rust
fn find_model_by_namespaced_suffix(model: &str, candidates: &[ModelInfo]) -> Option<ModelInfo>
```

`namespace/model-name` 형태의 단일-슬래시 슬러그만 좁게 처리한다. 절차:

1. `model.split_once('/')` — 슬래시 한 번만 허용. suffix 에 또 슬래시가 있으면 `None`.
2. namespace 는 `[A-Za-z0-9_]+` 만 허용 (`is_ascii_alphanumeric() || c == '_'`). 임의 alias 가 광범위하게 매치되는 사고를 방지.
3. suffix 부분에 다시 `find_model_by_longest_prefix` 를 적용.

예: `custom/qwen3.5-122b-thinking` → namespace=`custom` (ok), suffix=`qwen3.5-122b-thinking` → `qwen3.5-122b` 매치.

### 4.3 결합 — `construct_model_info_from_candidates`

`manager.rs:439-458` 가 두 함수를 묶는다:

```rust
let remote = find_model_by_longest_prefix(model, candidates)
    .or_else(|| find_model_by_namespaced_suffix(model, candidates));
let model_info = if let Some(remote) = remote {
    ModelInfo { slug: model.to_string(), used_fallback_model_metadata: false, ..remote }
} else {
    model_info::model_info_from_slug(model)   // 경고 로그 + fallback 메타
};
with_config_overrides(model_info, config)
```

매치된 경우 원본 `ModelInfo` 를 통째로 복사하되 `slug` 만 입력 슬러그로 바꿔 그대로 반환 — 즉 `qwen3.5-122b-fast` 슬러그를 쓰면 description / context_window / shell_type 등은 `qwen3.5-122b` 의 것을 그대로 받는다. 매치가 없으면 `model_info::model_info_from_slug` 가 `warn!("Unknown model {slug} is used. This will use fallback model metadata.")` 와 함께 최소 메타 (`context_window=272_000`, `visibility=None`, `used_fallback_model_metadata=true`) 를 만들어준다 (`model_info.rs:66-102`).

마지막에 `with_config_overrides` 가 `ModelsManagerConfig` 의 `model_context_window` / `tool_output_token_limit` / `base_instructions` / `personality_enabled` 같은 사용자 측 오버라이드를 덮어 쓴다 (`model_info.rs:23-63`).

## 5. Cache TTL

디스크 캐시는 `ModelsCacheManager` (`models-manager/src/cache.rs`) 가 관리한다.

| 상수 / 경로 | 값 | 정의 위치 |
| --- | --- | --- |
| `MODEL_CACHE_FILE` | `models_cache.json` | `manager.rs:22` |
| `DEFAULT_MODEL_CACHE_TTL` | `Duration::from_secs(300)` (5분) | `manager.rs:23` |
| 디스크 위치 | `${codex_home}/models_cache.json` → fork 기본 `~/.xtech/models_cache.json` | `manager.rs:202` + `06-config.md` §2 |

캐시 파일 스키마 (`cache.rs:160-169`):

```rust
struct ModelsCache {
    fetched_at: DateTime<Utc>,
    etag: Option<String>,
    client_version: Option<String>,   // CARGO_PKG_VERSION_MAJOR.MINOR.PATCH
    models: Vec<ModelInfo>,
}
```

`load_fresh(expected_version)` (`cache.rs:31-74`) 가 적용 가능 여부를 결정한다.

1. 파일 없으면 `None` (cache miss).
2. `cache.client_version` 이 `expected_version` 과 다르면 `None` — fork 의 Cargo 버전이 올라가면 자동 무효화.
3. `is_fresh(ttl)` (`cache.rs:171-182`) — `Utc::now() - fetched_at <= ttl`. `ttl == 0` 이면 무조건 stale.
4. 모두 통과하면 `Some(cache)` 반환.

### 5.1 RefreshStrategy 분기

`refresh_available_models` (`manager.rs:268-299`) 의 흐름:

- `should_refresh_models()` = `uses_codex_backend() || has_command_auth()`. ollama provider 는 둘 다 false 이므로, fork 기본 사용자 시나리오에서는 “refresh 자체가 발생하지 않고” bundled `models.json` 만 사용된다 (단, `Offline`/`OnlineIfUncached` 일 때는 캐시 적용은 시도).
- `Online` — 항상 `fetch_and_update_models` (네트워크).
- `Offline` — 캐시만 시도 (`try_load_cache`). 네트워크 절대 안 탐.
- `OnlineIfUncached` — 캐시 hit 이면 끝, miss 면 네트워크.

### 5.2 ETag 기반 short-circuit

`refresh_if_new_etag(etag)` (`manager.rs:252-263`) — 호출자가 외부 신호 (예: SSE 응답 헤더) 로부터 새 ETag 를 알게 됐을 때 사용. 현재 ETag 와 같으면 **`renew_cache_ttl`** 만 호출해 `fetched_at = now` 로 갱신 (`cache.rs:95-102`). 다르면 `RefreshStrategy::Online` 로 강제 refresh.

## 6. 폐쇄망 / airgap 이슈

이 매니저가 일으킬 수 있는 외부 호출은 §1.2 `OpenAiModelsEndpoint::list_models` 한 곳뿐이다. `airgap-audit-2026-05-08.md` §5.1 가 같은 결론을 적어두고 있다:

> 호출: `OpenAiModelsEndpoint::list_models` (`codex-rs/model-provider/src/models_endpoint.rs:81-109`). URL: `{provider.base_url}/models` — fork 의 ollama provider 는 사내 게이트웨이를 가리키므로 **외부로 빠지지 않음**. cache TTL 5분, disk cache `models_cache.json`. cache miss + closed gateway 면 5s timeout 후 warn — fatal 아님.

요약하면:

- fork 기본 provider (ollama) 는 `should_refresh_models()` 에서 false 가 떨어져 refresh 자체가 비활성. 사내 게이트웨이가 죽어 있어도 `OpenAiModelsEndpoint::list_models` 는 호출되지 않는다.
- 사용자가 명시적으로 OpenAI / ChatGPT auth 를 붙이는 경우에만 refresh 가 활성화된다 — 이 경우에도 게이트웨이 도메인은 `model_provider.base_url` 에서 결정된다.
- `availability_nux` 메시지 안에 외부 URL 이 들어 있을 수 있으나 자동 fetch / 자동 open 하지 않음 (`airgap-audit-2026-05-08.md` §8 참조).

cosmetic URL / DotSlash artifact / Statsig OTEL 등 다른 leak 경로는 이 매니저와 무관 — `airgap-audit-2026-05-08.md` 의 §6 / §7 / §8 을 보라.

## 7. fork 가 건드린 곳

### 7.1 `models.json` 단일 슬러그 정책

upstream 은 GPT-5 / GPT-5-codex / o3 / o4-mini 등 두 자릿수 슬러그를 묶어 보낸다. 이 fork 는 “사내 Qwen 게이트웨이 + guardian” 의 두 가지 책임만 남기고 잘라냈다:

- `qwen3.5-122b` — `visibility: list`, `priority: 0`, picker 의 default. 이 슬러그가 사실상 모든 사용자 turn 의 모델 — `06-config.md` §3 의 `model_provider = ollama` 디폴트와 짝을 이룬다.
- `codex-auto-review` — `visibility: hide`, `priority: 29`, `max_context_window: 1_000_000`.

상위 doc (`fork-docs/airgap-audit-2026-05-08.md` §8.2) 에는 “단일 `qwen3.5-122b` 항목으로 정리됨” 이라 적혀 있는데, 실제로는 두 항목 — 두 번째가 hidden 이라 사용자에게는 single 처럼 보인다.

### 7.2 `codex-auto-review` 가 `Hide` 인 이유 — guardian 의존성

guardian (`codex-rs/core/src/guardian/`) 은 사용자가 위험하다고 판단된 명령 / 패치를 자동으로 “두 번째 모델로 리뷰” 시키는 fork 의 보안 레이어다. 핵심 상수:

```rust
// codex-rs/core/src/guardian/mod.rs:43
const GUARDIAN_PREFERRED_MODEL: &str = "codex-auto-review";
```

review 진입점 (`guardian/review.rs:630-656`) 은 `models_manager.list_models(RefreshStrategy::Offline)` 로 카탈로그를 받은 뒤, **picker 가시성과 무관하게** 슬러그 하나로 매칭한다:

```rust
let preferred_model = available_models.iter()
    .find(|preset| preset.model == super::GUARDIAN_PREFERRED_MODEL);
```

즉:

- `codex-auto-review` 가 picker 에 뜨면 사용자가 실수로 메인 모델로 골라 turn 을 돌릴 수 있다 — `Hide` 로 막는다.
- 그러나 `Hide` 는 `ModelPreset` 자체가 `Vec` 에 그대로 남는다 (`filter_by_auth` 는 `supported_in_api` 만 본다). guardian 이 슬러그 매치로 찾을 수 있다.
- `mark_default_by_picker_visibility` 도 List 만 보므로 default 후보에서도 자동 탈락. `qwen3.5-122b` 가 항상 default.

이 “hidden 이지만 카탈로그에 살아있다” 패턴은 `apply_remote_models` 의 slug-upsert 동작 (§1.3) 과 궁합이 맞는다 — 게이트웨이가 `codex-auto-review` 슬러그를 돌려주지 않아도 bundled 항목이 남기 때문에 guardian 이 죽지 않는다.

### 7.3 매니저 본체 변경

`models-manager/src/` 의 Rust 코드는 fork 가 거의 손대지 않았다. fork 의 차이는 1) `models.json` 의 슬러그 셋, 2) `model_provider` 디폴트가 `ollama` 라서 `should_refresh_models()` 가 보통 false, 3) `codex_home` 이 `~/.xtech` 라 캐시 파일 위치가 바뀐다는 것 정도. 매니저 trait / cache / preset 변환 로직은 upstream 그대로 따라간다.

## 8. 호출자 지도

빠른 reference. `models_manager.list_models(...)` 의 주요 호출 사이트:

- `core/src/session/mod.rs:522` — 세션 시작 시 `OnlineIfUncached`. 사용자가 picker 를 처음 열 때 데이터를 따뜻하게 만든다.
- `core/src/session/review.rs:34` — `/review` 슬래시 커맨드 진입.
- `core/src/guardian/review.rs:633` — guardian 자동 리뷰 (§7.2). `Offline` 으로 절대 네트워크 안 탐.
- `core/src/tools/handlers/multi_agents_common.rs:303` — multi-agent 도구의 모델 enumeration. `Offline`.
- `core/src/thread_manager.rs:513` — `ThreadManager::list_models` 이 SDK 측 RPC 로 노출.
- 테스트 다수: `core/tests/suite/{model_switching,remote_models,personality,models_cache_ttl,...}.rs` — `OpenAiModelsManager` / `StaticModelsManager` 를 분리해서 검증.

`bundled_models_response()` 직접 호출 (`StaticModelsManager` 와 함께 자주 쓰임) 도 6개 테스트 파일에서 확인됨 — `core/tests/common/test_codex.rs:570` 등.
