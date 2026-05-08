# 06. Configuration system

이 문서는 xtech fork 의 설정 로딩 파이프라인을 정리한다. 사용자가 `xtech` 또는 `codex` 바이너리를 실행했을 때 최종 `Config` 객체가 어떻게 빌드되는지 — TOML 레이어의 합성 순서, fork 만의 JSON 보조 로더, 환경변수 우선순위, 디버깅 진입점 — 을 한 번에 파악할 수 있도록 작성했다.

상세 규칙은 upstream `AGENTS.md` / `docs/config.md` 에 있고, 이 문서는 fork 차이점에 집중한다.

## 1. 레이어 합성 순서

설정 빌드는 **여러 레이어를 한 방향으로 머지** 하는 모델이다. 핵심 진입점은 `codex-rs/config/src/loader/mod.rs::load_config_layers_state` 이며, 거기서 만들어진 `ConfigLayerStack` 을 `codex-rs/core/src/config/mod.rs::Config::build_config_with_layer_stack` 가 받아 최종 `Config` 로 변환한다.

precedence 는 `codex-rs/app-server-protocol/src/protocol/v2/config.rs::ConfigLayerSource::precedence` 에 박혀 있다 (숫자가 낮을수록 약함):

| precedence | layer | 출처 | 비고 |
| --- | --- | --- | --- |
| 0 | `Mdm` | macOS 관리 정책 | fork 기본 사용처 없음 |
| 10 | `System` | `/etc/codex/config.toml` (Unix) / `%ProgramData%\OpenAI\Codex\config.toml` (Windows) | |
| 20 | `User` | `${CODEX_HOME}/config.toml` (fork 기본 `~/.xtech/config.toml`) | |
| 25 | `Project` | `cwd` 부터 git 루트까지 발견된 모든 `./.codex/config.toml` | trust 가 통과돼야 활성화 |
| 30 | `SessionFlags` | CLI `-c key=value` (`codex-rs/utils/cli/src/config_override.rs`) | |
| 40, 50 | `LegacyManagedConfigToml*` | 폐기 예정 `managed_config.toml` | requirements 로 변환됨 |

`load_config_layers_state` 는 위 순서로 `Vec<ConfigLayerEntry>` 를 만들고, 각 엔트리의 `toml::Value` 를 `codex-rs/config/src/merge.rs::merge_toml_values` 로 합친다. 머지 규칙은:

- 양쪽이 `Table` 이면 키 단위로 재귀 머지 (overlay 가 이긴다).
- 그 외에는 overlay 값으로 덮어쓴다 (배열은 통째로 교체 — 누적 머지 아님).

추가로 `codex-rs/config/src/key_aliases.rs::normalize_key_aliases` 가 호출돼 deprecated 키를 canonical 키로 바꾼 뒤 머지된다.

이렇게 만들어진 “effective TOML” 을 `try_into::<ConfigToml>()` 로 디시리얼라이즈하고, 그 위에 main.rs 에서 만든 `ConfigOverrides` (Rust 구조체, TOML 이 아님) 를 적용해 최종 `Config` 가 된다 — 즉 우선순위는:

```
defaults → System → User → Project(cwd↑) → CLI -c (SessionFlags) → ConfigOverrides
                                                                    ↑
                                                                    main.rs 의 Rust 구조체
```

`ConfigOverrides` 는 `~/.xtech/config.toml` 에는 없는 런타임 전용 필드 (`cwd`, `codex_self_exe`, `codex_linux_sandbox_exe`, `main_execve_wrapper_exe`, `additional_writable_roots`, `ephemeral`, …) 와, CLI 가 “TOML 키 형태로 표현하기 어색한” 일부 의도 (예: `--dangerously-bypass-approvals-and-sandbox`) 를 주입하는 통로다 — `codex-rs/core/src/config/mod.rs:1850` 의 정의를 참조. 따라서 동일한 필드(예: `model_provider`)가 양쪽에 있으면 **`ConfigOverrides` 가 항상 마지막**.

CLI 측 머지는 `codex-rs/utils/cli/src/config_override.rs::CliConfigOverrides::parse_overrides` 가 `-c foo.bar=value` 를 `(path, toml::Value)` 쌍으로 파싱하고, `codex-rs/config/src/overrides.rs::build_cli_overrides_layer` 가 이를 점(`.`) 분리된 dotted-path 로 펴서 nested table 로 만든 뒤 `SessionFlags` 레이어로 push 한다. 값이 TOML 문법으로 파싱되지 않으면 raw string 으로 fallback (편의성 — `-c model=qwen3.5-122b` 처럼 따옴표 없이 쓸 수 있다는 뜻이지만, 부울/배열/맵 등 비-문자열 타입을 줄 때는 반드시 TOML 리터럴 형태로 쿼우팅이 필요하다).

프로젝트 레이어에는 보안 필터가 있다: `loader/mod.rs::PROJECT_LOCAL_CONFIG_DENYLIST` 에 들어간 키 (`openai_base_url`, `model_provider`, `model_providers`, `notify`, `profile`, `profiles`, `experimental_realtime_ws_base_url`, `chatgpt_base_url`) 는 프로젝트 `config.toml` 에서 읽혀도 머지 직전에 `sanitize_project_config` 가 제거하고 startup warning 으로 보고한다. 즉 “프로젝트가 자기 자격증명/엔드포인트를 변경하는 것”은 차단된다.

## 2. CODEX_HOME 해석 (`~/.xtech` 디폴트)

홈 디렉터리 결정은 `codex-rs/utils/home-dir/src/lib.rs::find_codex_home` 한 곳에 모여 있다. 흐름:

1. `CODEX_HOME` 환경변수를 읽는다. 빈 문자열은 unset 으로 간주 (`filter(|val| !val.is_empty())`).
2. 값이 있으면:
   - 존재하지 않으면 `NotFound` 로 **즉시 실패**.
   - 디렉터리가 아니면 `InvalidInput` 으로 실패.
   - canonicalize 후 `AbsolutePathBuf` 로 반환.
3. 값이 없으면 `dirs::home_dir()` 이 돌려준 `$HOME` 뒤에 `.xtech` 를 붙여 반환. 존재 여부는 검증하지 않는다 — 처음 실행 시 자동 생성되는 흐름을 막지 않기 위한 것.

**Fork 변경점**: upstream 은 `.codex` 를 push 한다. xtech 는 같은 함수의 본체와 doc-comment, 테스트 (`find_codex_home_without_env_uses_default_home_dir`) 를 모두 `.xtech` 로 바꿨다 (`codex-rs/utils/home-dir/src/lib.rs:7, 59, 130`). 따라서:

- 새로 설치한 사용자는 자동으로 `~/.xtech/` 를 보게 되고,
- upstream `~/.codex/` 가 이미 있는 사용자도 `CODEX_HOME=~/.codex xtech …` 또는 심볼릭 링크 (`ln -s ~/.codex ~/.xtech`) 로 그대로 재사용 가능.

`find_codex_home` 은 main.rs 의 거의 모든 진입점 (`run_interactive_tui`, `run_exec`, `mcp-server`, `app-server`, … — `codex-rs/cli/src/main.rs:876, 1303, 1316`) 에서 호출되며, 그 결과가 `load_config_layers_state` 의 `codex_home` 인자로 전달된다.

주의: `CODEX_HOME` 은 `find_codex_home` 한 곳에서만 읽고, 그 결과는 `load_config_layers_state` 의 매 호출마다 다시 캐싱 없이 전달된다. 따라서 한 프로세스 안에서 `CODEX_HOME` 을 도중에 바꿔도 영향 없다 — `xtech` 는 한 번 시작하면 한 home 만 본다.

## 3. `config.toml` 핵심 키

스키마는 `codex-rs/config/src/config_toml.rs::ConfigToml` 한 곳에서 정의하고 `JsonSchema` 매크로로 `codex-rs/core/config.schema.json` 자동 생성된다 (`just write-config-schema` → `codex-rs/core/src/bin/codex-write-config-schema.rs` → `codex_config::schema::write_config_schema`). `ConfigToml` 은 `#[schemars(deny_unknown_fields)]` 라 오타 키는 **에러로 떨어진다** — 디버깅 시 가장 먼저 의심할 부분.

자주 쓰는 필드 (전부 optional, 빈 값은 default 로 fallback). 전체 목록은 `ConfigToml` struct 정의 (line 93~) 와 `core/config.schema.json` 을 참고:

| 키 | 타입 | 의미 | 기본값 |
| --- | --- | --- | --- |
| `model` | string | 사용할 모델 슬러그 | provider default (`codex_utils_oss::get_default_model_for_oss_provider`) |
| `model_provider` | string | `model_providers` 맵의 키 | **`ollama`** (fork 변경 — `codex-rs/core/src/config/mod.rs:2634`) |
| `approval_policy` | enum | shell exec 승인 정책 (`unless-trusted`, `on-failure`, `never`, …) | `OnFailure` |
| `sandbox_mode` | enum | `read-only` / `workspace-write` / `danger-full-access` | `WorkspaceWrite` (Unix), Windows 별도 |
| `permissions` / `default_permissions` | table / string | 명명된 permission profile | |
| `mcp_servers` | map<string, McpServerConfig> | MCP 서버 정의 | `{}` |
| `profile` / `profiles` | string / map | 활성 profile 이름과 정의 | none |
| `model_providers` | map<string, ModelProviderInfo> | 사용자 정의 provider | builtin 만 사용 |
| `openai_base_url` / `chatgpt_base_url` | string | OpenAI / ChatGPT 게이트웨이 URL | none |
| `history` | table | `~/.xtech/history.jsonl` 동작 | `default_history()` |
| `sqlite_home`, `log_dir` | path | state DB / 로그 위치 | `${CODEX_HOME}` 하위 |
| `tools.web_search` | table | 웹 검색 설정 | none |
| `notify` | array<string> | 외부 notify 명령 (프로젝트 레이어에서는 차단됨) | none |
| `hide_agent_reasoning`, `show_raw_agent_reasoning` | bool | 추론 토큰 노출 정책 | `false` / `false` |
| `model_reasoning_effort`, `model_reasoning_summary`, `model_verbosity` | enum | reasoning / verbosity 튜닝 | none |

Fork 의 default provider 변경은 `core/src/config/mod.rs:2631-2634` 한 줄짜리 패치 (`OLLAMA_OSS_PROVIDER_ID`) 에 집중돼 있고, 빌트인 `ollama` provider 정의는 `codex-rs/model-provider-info/src/lib.rs` 에 그대로 남아 있다. 즉 “provider 자체를 새로 만든 게 아니라 기존 ollama provider 의 default 화” 라는 게 fork 의 핵심 idea.

profile 이 활성화되면 (`config_profile_key` 또는 `cfg.profile`) `ConfigProfile` (`profile_toml.rs:25`) 의 동명 필드가 base 보다 **우선** 적용된다 — `core/src/config/mod.rs:2775` `model = model.or(config_profile.model).or(cfg.model)` 식의 fallback chain. 즉 profile 은 `ConfigToml` 위에 얹히는 “경량 오버레이” 이지 별도 머지 단계가 아니다.

`ConfigProfile` 에 들어가는 키 (요약): `model`, `model_provider`, `approval_policy`, `sandbox_mode`, `service_tier`, `oss_provider`, `web_search`, `tools`, `model_instructions_file`, `include_apply_patch_tool`, `experimental_use_*`, `features`, … — 즉 “세션 단위로 자주 바뀌는 것” 위주. 반면 보안 결정에 영향을 주는 `model_providers`, `openai_base_url`, `chatgpt_base_url`, `mcp_servers` 같은 키는 profile 에 두지 않고 root level 에만 둔다.

## 4. Fork-only JSON loader (`apply_fork_config_to_env`)

fork 는 사용자가 흔히 쓰는 opencode 스타일 JSON 설정 — `{ baseURL, apiKey, model }` — 을 그대로 들여올 수 있도록 별도 로더를 둔다. 코드: `codex-rs/ollama/src/fork_config.rs`.

읽기 순서:

1. `$CODEX_FORK_CONFIG` 환경변수가 비어 있지 않으면 그 경로.
2. 아니면 `$HOME/.config/xtech/xtech.json`.

파일이 없으면 조용히 종료 (`NotFound` 만 무시, 다른 IO 에러는 `tracing::warn!` 로 보고하고 무시). JSON 파싱 실패도 동일하게 warn + 무시 — fork 동작이 망가지지 않도록 하기 위함.

스키마 (`ForkConfigJson`):

```jsonc
{
  "baseURL": "http://<gateway-host>/v1",   // alias: "base_url"
  "apiKey":  "sk-davis-...",                // alias: "api_key"
  "model":   "qwen3.5-122b"
}
```

각 필드가 **있고 trim 후 비어 있지 않으면** 다음 환경변수로 export:

| JSON 키 | export 되는 env | 소비처 |
| --- | --- | --- |
| `baseURL` | `CODEX_OSS_BASE_URL` | `codex-rs/model-provider-info/src/lib.rs` 빌트인 `ollama` provider 의 base url |
| `apiKey` | `OLLAMA_API_KEY` | 같은 곳, Bearer 토큰 |
| `model` | `CODEX_OSS_MODEL` | `codex-rs/utils/oss/src/lib.rs::get_default_model_for_oss_provider` |

`std::env::set_var` 는 multi-thread 환경에서 unsafe 라 fork 코드는 “스레드가 env 를 관측하기 전 startup” 에서만 호출한다는 가정을 갖는다 (`fork_config.rs:83` 의 `// SAFETY` 주석 참조). 호출 지점은 단 하나:

- `codex-rs/cli/src/main.rs::cli_main` 의 첫 줄 — `MultitoolCli::parse()` 보다 **앞**, 즉 clap 이 인자를 보기 전 (`main.rs:749`).
- 다른 진입점 (예: tests, app-server 의 라이브러리 사용) 에서는 호출되지 않는다 — fork JSON 은 `xtech` 바이너리를 **CLI 로 직접 실행** 했을 때만 효과가 있다는 뜻.

따라서 우선순위는 사실상 **JSON > 기존 env > 코드 default** 가 된다. 이미 `CODEX_OSS_BASE_URL` 등을 export 한 셸에서 실행해도 JSON 파일에 같은 키가 있으면 JSON 이 이긴다 — “파일이 source of truth” 로 만들기 위한 선택. 빈 문자열 / 공백 문자열은 JSON 쪽이 비활성으로 취급돼 기존 env 가 살아남는다.

`fork_model_override()` 보조 함수는 `CODEX_OSS_MODEL` 을 trim 하여 `Some(String)` / `None` 을 돌려준다 — UI 가 “fork 가 모델을 강제했는지” 표시할 때 쓴다.

이 fork JSON 은 의도적으로 `ConfigToml` 과는 **별도 채널** 이다. 즉 TOML 의 `model_providers.ollama.base_url` 을 직접 박는 대신 environment 를 거쳐가는 이유는, opencode 사용자의 기존 `~/.config/<tool>/<tool>.json` 컨벤션을 그대로 두면서도 비밀 자격증명을 git-tracked TOML 에 박지 않기 위함이다 (`fork-docs/ollama-migration.md` 의 “토큰을 코드/리포지토리에 박지 않는 이유” 섹션 참고).

## 5. 환경변수 정리표

| env | 용도 | 우선순위 / 빈 값 처리 |
| --- | --- | --- |
| `CODEX_HOME` | 설정 디렉터리 (`config.toml`, state DB, history) | unset 또는 `""` 면 `~/.xtech`. 값이 있으면 **존재·디렉터리 검증 후 canonicalize**, 실패 시 fatal. |
| `CODEX_FORK_CONFIG` | fork JSON 위치 override | trim 후 빈 문자열은 unset 으로 간주, 그러면 `~/.config/xtech/xtech.json` 사용 |
| `CODEX_OSS_BASE_URL` | ollama provider base URL | fork JSON 의 `baseURL` 가 있으면 덮어씀, 없으면 빌트인 default (`http://localhost:11434/v1`) |
| `OLLAMA_API_KEY` | ollama Bearer 토큰 | fork JSON 의 `apiKey` 가 있으면 덮어씀, 없으면 unset (인증 없이 호출) |
| `CODEX_OSS_MODEL` | OSS provider default 모델 | fork JSON 의 `model` 가 있으면 덮어씀, 없으면 `DEFAULT_OSS_MODEL` (`codex-rs/ollama/src/lib.rs:19` — fork 에서 `qwen3.5-122b`) |
| `CODEX_SQLITE_HOME` | state DB 디렉터리 | unset 이면 `${CODEX_HOME}` |
| `CODEX_SANDBOX`, `CODEX_SANDBOX_NETWORK_DISABLED` | 샌드박스 자기관측용 — **건드리지 말 것** | `AGENTS.md` 의 sandbox env 섹션 참조 |
| `RUST_LOG` | tracing 레벨 (e.g. `RUST_LOG=codex_config=debug,codex_core=info`) | 표준 `tracing-subscriber` env-filter 형식 |

빈 문자열을 unset 으로 다루는 패턴은 `codex-rs/utils/home-dir/src/lib.rs:14` 와 `codex-rs/ollama/src/fork_config.rs:39, 82` 에서 일관되게 쓰인다 — 셸 스크립트에서 `export FOO=""` 한 케이스를 “비워두기” 의도로 받아준다.

## 6. `ConfigToml ↔ Config` 매핑

빌드 단계는 `core/src/config/mod.rs::Config::build_config_with_layer_stack` (line 2138~) 에서 일어난다. 핵심 패턴:

- `ConfigOverrides` 를 destructuring 으로 모두 분해하므로 새 필드를 추가하면 컴파일러가 누락을 잡아준다 (`config/mod.rs:2174-2200`).
- profile 결정: `config_profile_key.or(cfg.profile)` 로 활성 이름을 정하고, 없으면 `ConfigProfile::default()`.
- 모든 “하나만 살아남는” 필드는 `override.or(profile_value).or(toml_value).unwrap_or_else(|| default)` 형태의 fallback chain. 예시:
  - `model_provider_id`: `override → profile.model_provider → cfg.model_provider → OLLAMA_OSS_PROVIDER_ID` (`config/mod.rs:2631`).
  - `model`: `override → profile.model → cfg.model` (`config/mod.rs:2775`).
  - `approval_policy` / `sandbox_mode`: 동일 패턴 + sandbox 와 permission_profile 동시 지정 시 `InvalidInput` 검사 (`config/mod.rs:2202-2218`).
- 상호배제 검사 3종 (`sandbox_mode↔permission_profile`, `sandbox_mode↔default_permissions`, `permission_profile↔default_permissions`) 은 `Config` 빌드 초입에서 던진다.
- 경로 필드는 `loader/mod.rs::resolve_relative_paths_in_config_toml` 에서 base_dir 기준 `AbsolutePathBuf` 로 흡수된다 — 따라서 `~/foo`, `./foo` 모두 layer 의 디렉터리 기준으로 정규화된 절대경로로 머지된다.
- serde:
  - `ConfigToml` 은 `#[serde]` 디폴트가 거의 없다 (대부분 `Option<T>`); default 값은 `unwrap_or` 단계에서 박힌다.
  - 일부 필드는 `#[serde(default = "default_xxx")]` 헬퍼 함수로 직접 채운다 — 예: `default_hide_agent_reasoning`, `default_history`, `default_allow_login_shell`.
  - `mcp_servers` 는 별도 schemars 함수와 custom deserializer 로 raw MCP 입력 형태를 받는다.

## 7. End-to-end 시나리오

다음 입력이 모두 동시에 존재한다고 하자:

- 환경: `CODEX_HOME` unset (= `~/.xtech`), `CODEX_OSS_BASE_URL=http://stale:11434/v1` (셸이 export 해 둔 stale 값).
- `~/.config/xtech/xtech.json`:
  ```json
  { "baseURL": "http://gw.example/v1", "apiKey": "sk-davis-...", "model": "qwen3.5-122b" }
  ```
- `~/.xtech/config.toml`:
  ```toml
  approval_policy = "on-failure"
  sandbox_mode = "workspace-write"
  profile = "code"
  [profiles.code]
  model = "qwen3.5-27b"
  approval_policy = "unless-trusted"
  ```
- 프로젝트의 `.codex/config.toml` (cwd 가 트러스트됨):
  ```toml
  sandbox_mode = "read-only"
  model_provider = "openai"   # 프로젝트 레이어에서 차단됨 (denylist)
  ```
- CLI: `xtech -c approval_policy='"never"' -p hello`.

순서대로 일어나는 일:

1. `cli_main` 진입 → `apply_fork_config_to_env()` 가 JSON 읽어 `CODEX_OSS_BASE_URL=http://gw.example/v1` (stale 덮어씀), `OLLAMA_API_KEY=sk-davis-…`, `CODEX_OSS_MODEL=qwen3.5-122b` 를 export.
2. `MultitoolCli::parse()` 가 `-c approval_policy="never"` 를 `raw_overrides=["approval_policy=\"never\""]` 로 캡처.
3. `find_codex_home()` → `~/.xtech` (디렉터리 없으면 그대로 — 후속 read 가 NotFound 면 빈 layer).
4. `load_config_layers_state` 가 layer 들을 만든다:
   - System (`/etc/codex/config.toml`): 없음 → 빈 table.
   - User (`~/.xtech/config.toml`): 위 TOML 그대로.
   - Project: `.codex/config.toml` 발견 + 트러스트됨 → `sanitize_project_config` 가 `model_provider` 제거, startup warning 1건 추가, 남은 `sandbox_mode = "read-only"` 만 통과.
   - SessionFlags: `{ approval_policy = "never" }` (CLI 의 `-c`).
5. `merge_toml_values` 로 합쳐 effective TOML 을 만든다 → `approval_policy="never"` (CLI 가 이김), `sandbox_mode="read-only"` (Project 가 User 를 이김), `model_provider` 키는 부재 (프로젝트가 차단당해 비어 있고 user 도 명시 안 했으므로).
6. `try_into::<ConfigToml>()` → `cfg`. `cfg.profile = "code"` 가 살아있음.
7. `Config::build_config_with_layer_stack` 진입. `ConfigOverrides` (main.rs 가 만든 것) 의 `approval_policy = Some(Never)` (CLI 의 `--ask-for-approval` 또는 위 -c 둘 중 하나). 단 이 시나리오에선 -c 만 썼으므로 `ConfigOverrides.approval_policy = None` — TOML 단계에서 이미 `"never"` 로 정해져 있다.
8. profile resolve: `cfg.profile = "code"` → `ConfigProfile { model: Some("qwen3.5-27b"), approval_policy: Some(UnlessTrusted), … }`.
9. fallback chain 실행:
   - `model = override.None.or(profile.model).or(cfg.model)` → `qwen3.5-27b`. (profile 이 fork JSON 의 `CODEX_OSS_MODEL` 보다 우선 — JSON 은 “provider default”, profile 은 “요청 model”.)
   - `approval_policy = override.None.or(profile.approval_policy).or(cfg.approval_policy)` → 주의: `cfg.approval_policy` 가 SessionFlags 머지로 이미 `Never` 다. profile 의 `UnlessTrusted` 가 그 위로 fallback chain 에서 cfg 를 이긴다 → 결과는 `UnlessTrusted`. **즉 -c 가 항상 이긴다는 직관과 다르다** — CLI 가 효과를 보려면 profile 에도 같은 키를 비우거나 profile 자체를 끄거나 `--ask-for-approval` 같은 “Override 채널” 을 써야 한다.
   - `model_provider_id = override.None.or(profile.None).or(cfg.None).unwrap_or(OLLAMA_OSS_PROVIDER_ID)` → `ollama` (fork 디폴트).
10. 결과 `Config`:
    - `model = "qwen3.5-27b"` (profile)
    - `model_provider_id = "ollama"` (fork default)
    - provider base URL → `http://gw.example/v1` (env, fork JSON 출처)
    - `approval_policy = UnlessTrusted` (profile 이 `-c` 를 이김 — 위 9. 참조)
    - `sandbox_mode = ReadOnly` (project layer 가 user 를 덮음)

이 시나리오의 교훈 두 가지:

- **CLI `-c` 는 layer precedence 안에서만 강하다** — profile fallback chain 보다 위가 아니다. 사용자가 강제로 이기고 싶으면 `ConfigOverrides` 채널 (clap 플래그 `--model`, `--ask-for-approval` 등) 을 써야 한다.
- **fork JSON 의 model 은 “OSS provider 의 default” 슬롯** 이지 “현 세션 model” 이 아니다. profile 또는 `model = …` 가 있으면 그쪽이 이긴다.

## 8. 디버깅 팁

1. **`-c` 로 한 키 빠르게 덮어쓰기**: `xtech -c model="qwen3.5-122b" -c sandbox_mode='"read-only"' …`. 값은 TOML 문법이므로 문자열은 따옴표가 필요하다 (그렇지 않으면 raw string 으로 떨어지는 fallback 이 적용됨 — `config_override.rs:65-72`).
2. **활성 값 확인**: `RUST_LOG=codex_config=debug,codex_core=debug xtech --help 2>&1 | head` 로 layer 합성 / 머지 로그를 본다. 더 깊게 보려면 `codex-rs/cli/src/lib.rs` 의 `info!` 진입점들을 살피거나, `Config` 가 빌드되는 함수에 `tracing::debug!` 를 임시 주입.
3. **JSON 보조 로더 점검**:
   - `CODEX_FORK_CONFIG=/tmp/xtech.json xtech …` 로 임시 경로 강제.
   - 파일이 없거나 파싱 실패면 `WARN xtech … failed to parse codex-fork.json; ignoring` 라인이 stderr 로 떨어진다 (`tracing::warn!` 경유 — `RUST_LOG=warn` 이상 필요).
4. **schema 와 실제 키 비교**: 알 수 없는 키 에러가 나면 `cat codex-rs/core/config.schema.json | jq '.properties | keys'` 로 허용 키 목록 확인. 스키마가 코드와 어긋났다면 `just write-config-schema` 로 재생성.
5. **CODEX_HOME 경로 의심 시**: `xtech` 가 갑자기 `~/.xtech` 가 아닌 곳을 보면 `env | grep CODEX_HOME` 부터. `find_codex_home` 은 빈 문자열이면 unset 처리하지만, 잘못된 경로면 fatal 이라 메시지에 `CODEX_HOME points to "…"` 가 그대로 나온다.
6. **profile 적용 추적**: `xtech -c profile="<name>" …` 로 외부에서 강제 → `core/src/config/mod.rs:2221-2236` 에서 lookup 실패 시 `config profile \`name\` not found` 메시지가 나온다. 정상이면 그 이후 모든 fallback chain 은 `config_profile.<field>` 를 cfg 보다 먼저 본다.
7. **프로젝트 레이어 비활성**: `cwd` 의 `.codex/config.toml` 이 분명 있는데도 적용이 안 되면 `User` 레이어의 `[projects]` 트러스트 테이블 누락 가능성이 크다 — startup warning 에 `add <key> as a trusted project` 가 출력된다 (`loader/mod.rs:710`).
8. **denylist 키 무시**: 프로젝트 `config.toml` 에 `model_provider` 등을 적었는데 적용이 안 되면 `Ignored unsupported project-local config keys in <path>: model_provider` startup warning 을 확인 (`loader/mod.rs:747-762`). 이건 의도된 보안 차단 — user 레벨 (`~/.xtech/config.toml`) 에 옮겨야 한다.
9. **머지 결과만 확인하고 싶을 때**: `load_config_as_toml_with_cli_overrides` (`core/src/config/mod.rs:1261`) 는 `ConfigToml` 단계의 머지된 값만 반환한다 — `Config` 빌드 단계의 ConfigOverrides 를 빼고 “파일 + CLI 만 합친 결과” 를 보고 싶을 때 유용. (단 doc-comment 가 deprecated 라고 명시했듯, requirements 가 적용되기 전 상태이므로 보안 결정에는 쓰지 말 것.)

## 9. 참고 파일

- `codex-rs/utils/home-dir/src/lib.rs` — `CODEX_HOME` 해석, fork default `.xtech`.
- `codex-rs/config/src/loader/mod.rs` — 레이어 합성 / project trust / legacy managed config.
- `codex-rs/config/src/config_toml.rs` — `ConfigToml` 스키마, schemars 정의.
- `codex-rs/config/src/profile_toml.rs` — `ConfigProfile` 정의.
- `codex-rs/config/src/merge.rs` / `overrides.rs` — TOML 머지와 dotted-path expansion.
- `codex-rs/core/src/config/mod.rs` — `Config::build_config_with_layer_stack`, `ConfigOverrides`, fork 의 default provider.
- `codex-rs/ollama/src/fork_config.rs` — `apply_fork_config_to_env`, `~/.config/xtech/xtech.json`.
- `codex-rs/cli/src/main.rs` — fork loader 호출 지점 (`cli_main` 첫 줄), `find_codex_home` 진입점들.
- `codex-rs/utils/cli/src/config_override.rs` — `-c key=value` 파싱.
- `codex-rs/app-server-protocol/src/protocol/v2/config.rs` — `ConfigLayerSource` 와 precedence.
- `codex-rs/core/config.schema.json` — 자동 생성 JSON 스키마 (verify 용 ground truth).
