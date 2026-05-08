# Air-gap audit — codex-fork (2026-05-08)

대상 fork: `codex-fork` (LLM 라우팅은 이미 사내 Ollama 호환 게이트웨이로 우회됨, 자세한 사항은 `fork-docs/work-log-2026-05-08.md`).
이 문서는 **LLM 호출 외**의 잔존 외부 의존성을 점검한 결과다. 폐쇄망 (closed network) 배포 전에 차단/스텁/플래그 가드가 필요한 항목을 우선순위와 함께 정리한다.

---

## 1. 요약 / TL;DR

- **P0 — Statsig OTLP metrics**: 릴리즈 빌드의 `exec` / `tui` 양쪽이 기본값으로 `analytics_enabled = true` 이며, `OtelExporterKind::Statsig` 로 해석되어 매 세션 `https://ab.chatgpt.com/otlp/v1/metrics` 로 OTLP HTTP 메트릭을 보낸다 (`codex-rs/otel/src/config.rs:8`, `codex-rs/exec/src/lib.rs:155`, `codex-rs/tui/src/lib.rs:1025`). 폐쇄망 클라이언트에서 Codex 사용 사실/세션 단위 telemetry 가 사내가 아닌 외부 제3 도메인으로 빠진다.
- **P0 — curated plugin 동기화**: app-server 가 뜰 때마다 `https://github.com/openai/plugins.git` clone 또는 `https://api.github.com/...` 호출, 실패 시 `https://chatgpt.com/backend-api/plugins/export/curated` fallback 시도 (`codex-rs/core-plugins/src/startup_sync.rs:18-27,155`). `Feature::Plugins` 가 기본 `true` (`codex-rs/features/src/lib.rs:939-943`) 라 항상 동작. 폐쇄망에서는 매 startup 마다 long timeout 까지 hang + warn 로그.
- **P0 — featured/installed plugin 원격 동기화**: `featured_plugin_ids_for_config` 가 `chatgpt_base_url + /plugins/featured` 를 GET (`codex-rs/core-plugins/src/remote_legacy.rs:163`), startup remote sync 가 `/plugins/list` 등을 호출 (`remote_legacy.rs:131`). 기본 `chatgpt_base_url = https://chatgpt.com/backend-api/` (`codex-rs/exec/src/lib.rs:353`, `codex-rs/login/src/auth/manager.rs:93`). 사용자가 본 401 로그가 정확히 이 경로.
- **P1 — 업데이트 체크**: 릴리즈 TUI 가 `check_for_update_on_startup` 기본 `true` (`codex-rs/core/src/config/mod.rs:2876`) 로 `https://api.github.com/repos/openai/codex/releases/latest`, 또는 Homebrew 설치인 경우 `https://formulae.brew.sh/api/cask/codex.json` 백그라운드 호출 (`codex-rs/tui/src/updates.rs:65-66`). 실패해도 fatal 은 아니지만 hang+warn.
- **P1 — agent identity JWKS**: ChatGPT OAuth 로그인을 사용하는 사용자는 `https://chatgpt.com/backend-api/wham/agent-identities/jwks` 를 fetch (`codex-rs/agent-identity/src/lib.rs:36`, `manager.rs:496`). API key (Ollama) 사용자는 호출 안 함. fork 의 디폴트 흐름에서는 미점화이지만 ChatGPT 로그인 코드 경로가 살아 있어서 사용자가 잘못된 명령을 실행하면 켜진다.

---

## 2. 외부 호출 / Network egress

### 2.1 OTEL / 메트릭 (P0)

| 항목 | 값 |
| --- | --- |
| URL | `https://ab.chatgpt.com/otlp/v1/metrics` (`codex-rs/otel/src/config.rs:8`) |
| Auth | 헤더 `statsig-api-key: client-MkRuleRQBd6qakfnDYqJVR9JuXcY57Ljly3vi5JVUIO` (하드코딩, `config.rs:10`) |
| 트리거 | `exec` / `tui` 의 `build_provider` 가 metrics_exporter 를 결정 (`codex-rs/core/src/otel_init.rs:70-77`). `analytics_enabled.unwrap_or(default_analytics_enabled)` 가 true 이면 `OtelExporterKind::Statsig` 가 OtlpHttp 로 resolve. `default_analytics_enabled` 는 `exec`=true (`codex-rs/exec/src/lib.rs:155`), `tui`=true (`codex-rs/tui/src/lib.rs:1025`), `app-server` 메인 바이너리만 false (`codex-rs/app-server/src/main.rs:72`). |
| 디버그 빌드 동작 | `cfg!(debug_assertions)` 분기에서 `OtelExporter::None` 으로 강제 (`codex-rs/otel/src/config.rs:14-22`). 따라서 cargo run 디버그에서는 안 보임 — 릴리즈 바이너리에서만 점화. |
| 오프라인 영향 | non-fatal 이지만 5–30s 단위 batch 간격으로 outbound HTTP 시도 → connection error 로그 다량. |
| 권장 조치 | (a) `~/.codex/config.toml` 에 `analytics_enabled = false` 또는 `[otel] metrics_exporter = "none"` 강제, 또는 (b) fork 에서 `STATSIG_OTLP_HTTP_ENDPOINT` 자체를 제거하고 `resolve_exporter` 가 `Statsig → None` 으로 빠지게 패치. 디폴트 설정만 패치하지 말고 **상수 자체**를 제거하면 leak 표면이 사라진다. |

### 2.2 plugin / marketplace 동기화 (P0)

#### 2.2.1 curated plugin git clone

| 항목 | 값 |
| --- | --- |
| URL | `https://github.com/openai/plugins.git` (git clone, `codex-rs/core-plugins/src/startup_sync.rs:155`), fallback `https://api.github.com/repos/openai/plugins/git/refs/heads/<branch>` 등 (`startup_sync.rs:18`), fallback `https://chatgpt.com/backend-api/plugins/export/curated` (`startup_sync.rs:21-22`). |
| 트리거 | `PluginsManager::start_curated_repo_sync` (`codex-rs/core-plugins/src/manager.rs:1651`). app-server 부팅 시 `maybe_start_plugin_startup_tasks_for_config` 에서 `Feature::Plugins` 가 켜져 있으면 항상 스폰 (`manager.rs:1392-1393`, app-server 진입은 `codex-rs/app-server/src/message_processor.rs:422`). `Feature::Plugins` 기본 true. |
| 빈도 | app-server 프로세스 당 1회 (process-static `CURATED_REPO_SYNC_STARTED` 가드, `manager.rs:79`). 매 `codex` 실행마다 process 가 새로 뜨므로 사실상 invocation 당 1회. |
| 오프라인 영향 | 30s git timeout (`startup_sync.rs:28`) + 30s HTTP timeout + 30s archive timeout 이 직렬로 시도되어, 최악의 경우 90s warn 후 fail. fatal 은 아님 (panic 안 함). 그러나 `tracing::warn!` 다량. |
| 권장 조치 | (a) `Feature::Plugins` 를 fork 에서 default_enabled = false 로 (`codex-rs/features/src/lib.rs:939-943`). (b) 또는 `start_curated_repo_sync` 진입 시 closed-network gate (`CODEX_AIRGAP=1` 같은 fork-only env). (a) 가 깔끔. |

#### 2.2.2 featured / installed plugin REST

| 항목 | 값 |
| --- | --- |
| URL | `{chatgpt_base_url}/plugins/featured` (`codex-rs/core-plugins/src/remote_legacy.rs:163`), `{chatgpt_base_url}/plugins/list` (`remote_legacy.rs:131`), `{chatgpt_base_url}/plugins/{id}/{enable|uninstall}` (`remote_legacy.rs:286-296`). 추가로 `crate::remote::fetch_remote_installed_plugins` 가 `manager.rs:1709` 에서 호출됨. |
| 기본 chatgpt_base_url | `https://chatgpt.com/backend-api/` (`codex-rs/exec/src/lib.rs:353`, `codex-rs/login/src/auth/manager.rs:93`). fork 가 override 하지 않음. |
| 트리거 | `featured_plugin_ids_for_config` 는 `maybe_start_plugin_startup_tasks_for_config` 에서 cache warmup 으로 spawn (`manager.rs:1474-1485` — 사용자가 본 `failed to warm featured plugin ids cache` 로그가 정확히 여기). `sync_plugins_from_remote` 는 `start_startup_remote_plugin_sync_once` 에서 spawn (`codex-rs/core-plugins/src/startup_remote_sync.rs:27`). |
| 인증 | `auth.uses_codex_backend()` 가 true 일 때만 status/list/mutation 이 실행 (`remote_legacy.rs:124-128`). `featured` 는 무인증으로도 GET 시도. Ollama API key 사용자는 status 는 silent skip 되지만 **featured 는 무조건 HTTP 시도**. |
| 오프라인 영향 | 10s timeout 후 warn. fatal 아님. featured 는 401 반환을 받아도 warn-and-continue. |
| 권장 조치 | (a) `chatgpt_base_url` 디폴트를 사내값 또는 빈 문자열로 (`codex-rs/login/src/auth/manager.rs:93`, `codex-rs/exec/src/lib.rs:353`). 빈 문자열 시 build_url 단계에서 fail-fast 하므로 in-memory short-circuit 필요. (b) `RemotePluginServiceConfig` 사용처마다 `if cfg.chatgpt_base_url.is_empty() { return Ok(empty); }` 가드. (c) 또는 plugin 기능 자체를 비활성화 (위 2.2.1 권장과 함께 진행). |

### 2.3 업데이트 체크 (P1)

| 항목 | 값 |
| --- | --- |
| URL | `https://api.github.com/repos/openai/codex/releases/latest` (`codex-rs/tui/src/updates.rs:66`), Homebrew 설치 감지 시 `https://formulae.brew.sh/api/cask/codex.json` (`updates.rs:65`), npm/bun 설치 감지 시 `https://registry.npmjs.org/@openai%2fcodex` (`codex-rs/tui/src/npm_registry.rs:5`). |
| 트리거 | `get_upgrade_version` 호출 시점, TUI 에서만 (`updates.rs:1` 의 `#![cfg(not(debug_assertions))]` 로 디버그에서는 비활성화). 마지막 체크 후 20시간 경과 시 백그라운드 fetch (`updates.rs:33`). `check_for_update_on_startup` 기본 true (`codex-rs/core/src/config/mod.rs:2876`). |
| 오프라인 영향 | TUI 에서만 영향. 백그라운드 task 라 메인 흐름 block 없음. 다만 매 20h 마다 connect error warn. |
| 권장 조치 | `check_for_update_on_startup` 디폴트를 false 로 (`codex-rs/core/src/config/mod.rs:2876`). 한 줄 수정으로 깔끔. |

### 2.4 agent identity JWKS / OAuth refresh (P1)

| 항목 | 값 |
| --- | --- |
| URL | JWKS: `{chatgpt_base_url}/wham/agent-identities/jwks` → 기본 `https://chatgpt.com/backend-api/wham/agent-identities/jwks` (`codex-rs/agent-identity/src/lib.rs:36,317`, `codex-rs/login/src/auth/manager.rs:496`). Refresh: `https://auth.openai.com/oauth/token` (`manager.rs:94`). Revoke: `https://auth.openai.com/oauth/revoke` (`manager.rs:95`). |
| 트리거 | Agent identity / ChatGPT OAuth 로그인 사용자가 토큰 refresh 또는 JWT 검증을 할 때. fork 의 기본 인증은 API key (Ollama) 라 미점화. 단 사용자가 잘못해서 `codex login` (chatgpt mode) 실행하면 점화. |
| 오프라인 영향 | API key 모드에서는 무관. ChatGPT 로그인 시도 시 fatal (login 자체가 OAuth 콜백이라 closed-network 에서는 동작 자체 불가능). |
| 권장 조치 | `codex login` (ChatGPT mode), `codex login --use-device-code` 를 fork 에서 hide / disable (`codex-rs/cli/src/main.rs:967-1011`). API key login (`codex login --with-api-key`) 만 노출하면 충분. |

### 2.5 cloud requirements / wham (P1)

| 항목 | 값 |
| --- | --- |
| URL | `{chatgpt_base_url}/wham/config/requirements` (`codex-rs/backend-client/src/client.rs:403`, 호출 표면 `codex-rs/cloud-requirements/src/lib.rs`). |
| 트리거 | `cloud_requirements_eligible_auth` 가 true 인 경우만 — `auth.uses_codex_backend()` AND plan_type 이 Business / Enterprise (`codex-rs/cloud-requirements/src/lib.rs:182-188`). API key (Ollama) 이용자에서는 미점화. |
| 오프라인 영향 | 해당 plan 사용자에서만 fatal (fail-closed 로 설계됨, `cloud-requirements/src/lib.rs:7-9`). fork 운영에서는 무관. |
| 권장 조치 | 현재 fork 운영에선 트리거 안 되므로 P1. enterprise 사용자가 닿을 위험을 차단하려면 `cloud_requirements_eligible_auth` 를 강제 false 로 패치. |

### 2.6 cloud tasks (P2)

| 항목 | 값 |
| --- | --- |
| URL | `https://chatgpt.com/backend-api` (`codex-rs/cloud-tasks/src/lib.rs:50,842,857,1083,1467,1655,1832,2315`). |
| 트리거 | `codex cloud` (alias `cloud-tasks`) 서브커맨드 실행 시에만 (`codex-rs/cli/src/main.rs:160-161, 1039`). 사용자가 명시적으로 호출. |
| 오프라인 영향 | 해당 서브커맨드 자체가 closed network 에선 동작 불가능. fail-fast 로 끝남. |
| 권장 조치 | `codex cloud` 서브커맨드를 fork 에서 hide / 제거. 단순 cosmetic 측면에서 P2. |

### 2.7 desktop app installer (P2)

| 항목 | 값 |
| --- | --- |
| URL | `https://persistent.oaistatic.com/codex-app-prod/Codex.dmg` (arm64) / `Codex-latest-x64.dmg` (`codex-rs/cli/src/desktop_app/mac.rs:8-10`). Windows 는 Microsoft Store URL (`codex-rs/cli/src/desktop_app/windows.rs:7-8`). |
| 트리거 | `codex app` 서브커맨드 실행 시에만 (`codex-rs/cli/src/main.rs:905`, `codex-rs/cli/src/app_cmd.rs:19`). |
| 오프라인 영향 | 해당 서브커맨드 호출 시 fail. CLI 일반 사용에는 영향 없음. |
| 권장 조치 | hide. P2. |

### 2.8 cookies / auth probe (P2)

`codex-rs/codex-client/src/chatgpt_cloudflare_cookies.rs:130 …` 의 chatgpt.com URL 들은 cookie host filter (host == chatgpt.com) 일 때만 작동. 호스트가 사내 게이트웨이면 노출 안 됨.

---

## 3. 외부 바이너리 의존

| 바이너리 | 사용 위치 | 설치 가정 | 폐쇄망 영향 |
| --- | --- | --- | --- |
| `git` | curated plugin clone (`codex-rs/core-plugins/src/startup_sync.rs:138-159`), marketplace add/upgrade (`codex-rs/core-plugins/src/marketplace_upgrade/git.rs:137`, `marketplace_add/install.rs:114`, `loader.rs:1187`), apply-patch baseline (`codex-rs/git-utils/src/apply.rs:127-152,332`), branch (`branch.rs:129,138`), info (`info.rs:273`), turn diff (`codex-rs/core/src/turn_diff_tracker.rs:207`), cloud-tasks env detect (`codex-rs/cloud-tasks/src/env_detect.rs:173,191`). | 시스템에 설치되어 있다고 가정. **fork 가 번들하지 않음**. | git 자체는 closed-network 에서도 동작. 단 plugin clone 은 외부 GitHub 을 향하므로 (3.A) 와 함께 차단. |
| `bwrap` (Linux sandbox) | `codex-rs/linux-sandbox/src/bwrap.rs`, dotslash 로 `codex-bwrap` 동봉 (`codex-rs/release/dotslash/codex-bwrap.lock`). | Linux 릴리즈 빌드에서 bundled (commit 22326e26 `release: bundle bwrap with Linux codex DotSlash artifact`). | 영향 없음. |
| `sandbox-exec` (macOS Seatbelt) | `codex-rs/sandboxing/src/seatbelt.rs`. | macOS system binary, 항상 존재. | 영향 없음. |
| `rg` (ripgrep) | `codex-rs/linux-sandbox/src/bwrap.rs:818,2638` (linux sandbox 내부 helper). | 시스템에 설치되어 있다고 가정. | 일반 동작에는 critical 하지 않음. |
| `npm` / `bun` | TUI update prompt 가 사용자에게 명령 안내만 (`codex-rs/tui/src/update_action.rs`). | **CLI 가 직접 spawn 하지 않음**. | 영향 없음. |

요약: **fork 가 가정하는 외부 바이너리는 git, sandbox-exec, bwrap, rg**. 모두 폐쇄망에서도 사용 가능. 단 `git clone` 의 대상 URL (3.A의 GitHub plugins repo) 만 차단 대상.

---

## 4. install / login / auth 흐름

### 4.1 ChatGPT 로그인 (`codex login`, browser flow)

- 진입: `codex-rs/cli/src/login.rs:134 run_login_with_chatgpt`.
- 의존 URL: OAuth issuer `https://auth.openai.com` (`codex-rs/login/src/server.rs:51`), token endpoint `https://auth.openai.com/oauth/token`, revoke `https://auth.openai.com/oauth/revoke` (`codex-rs/login/src/auth/manager.rs:94-95`), JWT audience 검증 `https://api.openai.com/auth` (`codex-rs/login/src/token_data.rs:75-77`), platform.openai.com 페이지 (`codex-rs/login/src/server.rs:854`).
- 폐쇄망 영향: **fatal** (OAuth 콜백 자체가 동작 불가).
- 권장 조치: `codex login` 의 ChatGPT/device-code 분기 둘 다 hide 또는 명시적 차단. fork 의 인증은 `codex login --with-api-key` 또는 `~/.codex/codex-fork.json` 의 `apiKey` 만으로 충분.

### 4.2 Device code flow (`codex login --use-device-code`)

- 진입: `codex-rs/cli/src/login.rs:267 run_login_with_device_code`.
- 동일하게 `auth.openai.com` 의 device authorization endpoint 사용. 폐쇄망 fatal.

### 4.3 API key login (`codex login --with-api-key`)

- 로컬 디스크에 키 저장만 함. 외부 호출 없음. **폐쇄망 OK**.
- fork 가 사용하는 권장 경로.

### 4.4 access token login (`codex login --with-access-token`)

- 마찬가지로 로컬 저장만. 단, 이후 token refresh 시 `auth.openai.com/oauth/token` 호출 가능 (4.1 참고).

### 4.5 enforce_login_restrictions

- `codex-rs/login/src/auth/manager.rs:617`. 순수 로컬 파일 검증. 외부 호출 없음.

---

## 5. 모델 카탈로그 / 스킬 / 플러그인 동기화

### 5.1 모델 카탈로그 (`/models`)

- 호출: `OpenAiModelsEndpoint::list_models` (`codex-rs/model-provider/src/models_endpoint.rs:81-109`).
- URL: `{provider.base_url}/models` — fork 의 ollama provider 는 사내 게이트웨이를 가리키므로 **외부로 빠지지 않음**.
- 빈도: `RefreshStrategy::Online` / `OnlineIfUncached` 시. cache TTL 5분 (`codex-rs/models-manager/src/manager.rs:23`). disk cache `models_cache.json` (`manager.rs:22`).
- Offline 동작: `RefreshStrategy::Offline` / cache hit 시 네트워크 안 탐. cache miss + closed gateway 면 5s timeout (`models_endpoint.rs:31`) 후 warn. fatal 아님.
- **결론**: 사내 게이트웨이 자체가 살아 있으면 OK. 외부 leak 없음.

### 5.2 원격 skills (`/hazelnuts`)

- 호출: `codex-rs/core-skills/src/remote.rs:99 list_remote_skills`.
- URL: `{chatgpt_base_url}/hazelnuts` → 기본 `https://chatgpt.com/backend-api/hazelnuts`.
- 트리거: 코드 주석에 따르면 *"intentionally kept around for future wiring, but it is not used yet by any active product surface"* (`core-skills/src/remote.rs:14-15`). 현 시점 dead code.
- **결론**: 현재 leak 없음. 미래 PR 머지 시 활성화 가능성 있어 P2 로 모니터링.

### 5.3 plugin 동기화

위 §2.2 참조. **현재 가장 시끄러운 leak**.

### 5.4 marketplace upgrade

- `codex-rs/core-plugins/src/marketplace_upgrade/git.rs:137` 의 `git fetch/pull` — 설정된 marketplace `source` URL 을 사용. 사용자가 외부 `https://github.com/...` marketplace 를 추가하지 않는 한 미점화. fork-default config 에서는 안전.

---

## 6. Telemetry / 크래시 리포트 / OTEL

### 6.1 OTEL Statsig (P0)

§2.1 참조. **가장 큰 leak**.

### 6.2 Sentry (feedback upload)

- `codex-rs/feedback/src/lib.rs:31`: 하드코딩된 DSN `https://ae32ed50620d7a7792c1ce5df38b3e3e@o33249.ingest.us.sentry.io/4510195390611458`.
- 트리거: `feedback/upload` JSON-RPC 메서드 (`codex-rs/app-server/src/request_processors/feedback_processor.rs:196`). 사용자가 명시적으로 *Submit feedback* UI 를 누르거나 `codex feedback submit` 호출 시.
- 폐쇄망 영향: 사용자 명시 액션이라 unintentional leak 위험은 낮음. 다만 호출되면 thread snapshot, rollout, log 첨부 등 민감 정보 전송.
- 권장 조치: `SENTRY_DSN` 상수를 빈 문자열로 패치하거나 `upload_feedback` 진입에서 fork-only short-circuit (`return Err("disabled in this build")`).

### 6.3 analytics events (P2 — 현 fork 에서는 미점화)

- `codex-rs/analytics/src/client.rs:370`: `{chatgpt_base_url}/codex/analytics-events/events`.
- `auth.uses_codex_backend()` 가 false 면 `send_track_events` 가 silent return (`client.rs:365-367`). API key 모드에서는 미점화.
- 권장 조치: 그대로 두어도 안전. 단 `chatgpt_base_url` 자체를 fork 에서 정리하면 함께 무력화됨.

### 6.4 codex_otel.log_only events

- `codex-rs/model-provider/src/models_endpoint.rs:135` 등에서 `target: "codex_otel.log_only"` event emit. `codex_export_filter` (`codex-rs/core/src/otel_init.rs:97-99`) 가 codex_otel target 만 export 하므로 위 6.1 의 Statsig exporter 가 이 이벤트들도 함께 외부로 보낸다. 6.1 차단으로 자동 해결.

---

## 7. Auto-update / 버전체크

§2.3 (P1) 참조. 추가 사항:

- `codex-rs/tui/src/update_prompt.rs:207`: 업데이트 안내 화면이 `https://github.com/openai/codex/releases/latest` 링크를 표시. **표시만**, 자동 fetch 아님 — cosmetic.
- `codex-rs/cli/src/main.rs:637`: 자동 업데이트 실패 시 `https://developers.openai.com/codex/cli/` 안내 메시지. cosmetic.
- DotSlash artifact (`codex-rs/release/dotslash/codex-*.lock`) 는 **빌드/릴리즈 타임** artifact 다운로드용이라 런타임 영향 없음. 단 사용자가 codex 를 dotslash 래퍼로 직접 받았다면 첫 실행 시 dotslash 가 GitHub releases 에서 동봉 binary 를 가져온다 — 폐쇄망에서는 사전 mirror 필요.

---

## 8. Documentation / NUX / 하드코딩 URL

### 자동 fetch / 자동 open 되지 않는 cosmetic URL (P2)

- `codex-rs/sandboxing/src/bwrap.rs:20`: `https://developers.openai.com/codex/concepts/sandboxing#prerequisites` — 에러 메시지 안내.
- `codex-rs/cli/src/main.rs:439`: `https://developers.openai.com/codex/config-advanced/#metrics` — `--help` 도움말.
- `codex-rs/codex-mcp/src/connection_manager.rs:687`: GitHub MCP 안내 메시지.
- `codex-rs/ollama/src/client.rs:22`: `https://github.com/ollama/ollama` 설치 안내 메시지.
- `codex-rs/model-provider-info/src/lib.rs:42`: `OLLAMA_CHAT_PROVIDER_REMOVED_ERROR` 가 GitHub discussions URL 안내.

이들은 모두 사용자에게 표시되는 텍스트일 뿐 자동 open / fetch 하지 않음. 수정 우선순위 낮음.

### `models.json` availability_nux (P2)

- `codex-rs/models-manager/models.json` 은 fork 에서 단일 `qwen3.5-122b` 항목으로 정리됨 (work-log 2.6 참조). availability_nux 메시지가 외부 URL 을 포함할 수 있으나 자동 open 안 함.

---

## 9. 권장 조치 / 우선순위 매트릭스

| 우선순위 | 항목 | 조치 (한 줄 수정 가능한 것 위주) |
| --- | --- | --- |
| **P0** | Statsig OTLP metrics leak (§2.1) | `codex-rs/exec/src/lib.rs:155` `DEFAULT_ANALYTICS_ENABLED = false` 로, `codex-rs/tui/src/lib.rs:1025` `default_analytics_enabled` 인자도 false. 추가로 `codex-rs/otel/src/config.rs:8-10` 의 `STATSIG_OTLP_HTTP_ENDPOINT`/`STATSIG_API_KEY` 상수를 빈 문자열로 두어 향후 실수 방지. |
| **P0** | curated plugin git/HTTP/archive 동기화 (§2.2.1) | `codex-rs/features/src/lib.rs:942` `default_enabled: false`. PluginsManager 의 startup task 가 features gate 를 따르므로 한 줄로 차단. |
| **P0** | featured/installed plugin REST sync (§2.2.2) | 위 plugins feature off 로 함께 해소. 또한 보수적으로 `core-plugins/src/remote_legacy.rs::fetch_remote_featured_plugin_ids` 진입에서 `if config.chatgpt_base_url.is_empty() || ...` early-return 패치. |
| **P1** | TUI update check (§2.3) | `codex-rs/core/src/config/mod.rs:2876` `check_for_update_on_startup.unwrap_or(false)` 로 변경. |
| **P1** | ChatGPT / device-code login 진입 차단 (§4.1, §4.2) | `codex-rs/cli/src/main.rs` 의 LoginSubcommand 분기 (line 977 부근) 에서 ChatGPT browser/device-code 두 분기를 `bail!("disabled in fork")` 로 막거나, 서브커맨드 자체를 hide. |
| **P1** | feedback upload (Sentry) (§6.2) | `codex-rs/feedback/src/lib.rs:31` `SENTRY_DSN = ""` 로, `upload_feedback` 진입에서 빈 DSN 일 때 silent skip. |
| **P1** | agent identity JWKS (§2.4) | ChatGPT login 차단 (P1 위 항목) 으로 자연 해소. 추가 가드 불필요. |
| **P2** | `codex cloud` 서브커맨드 (§2.6) | `codex-rs/cli/src/main.rs:160` 에 `#[clap(hide = true)]` 추가. |
| **P2** | `codex app` 서브커맨드 (§2.7) | `codex-rs/cli/src/main.rs:905` 분기 hide 또는 제거. |
| **P2** | cosmetic 안내 URL (§8) | 우선순위 낮음. 사내 mirror 가 결정되면 한꺼번에 치환. |

### 최소 변경으로 가는 path

위 P0 세 항목만 막으면 air-gap 환경에서 `codex exec`/`codex tui` 의 외부 호출은 사실상 게이트웨이 (`/v1/chat/completions`) 한 개로 수렴한다. P1 까지 처리하면 cosmetic warning 도 사라진다. P2 는 hide 만으로 충분.

### 검증 방법

- 폐쇄망 머신에서 `RUST_LOG=info codex exec "ping"` 실행 후 stderr/log 에 `chatgpt.com`, `api.github.com`, `formulae.brew.sh`, `ab.chatgpt.com`, `auth.openai.com`, `oaistatic` 토큰이 나타나지 않는지 grep.
- `tcpdump -i any 'host not <internal-gateway>'` 로 발신지 IP 확인.
- (선택) fork build 에 `RUSTFLAGS="--cfg airgap"` 등 빌드 플래그를 도입하면 P0/P1 변경을 conditional 하게 둘 수 있다.
