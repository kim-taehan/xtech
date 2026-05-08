# 11 — Plugin 서브시스템

`xtech` 가 upstream `openai/codex` 에서 가장 “외부 의존”이 강한 영역 중 하나가 plugin 시스템이다. LLM 라우팅을 사내 게이트웨이로 우회한 뒤에도 (`fork-docs/ollama-migration.md`, `fork-docs/work-log-2026-05-08.md`) plugin 쪽 코드 경로는 여전히 GitHub 과 `chatgpt.com/backend-api/...` 를 향해 startup 마다 호출을 시도한다. 이 문서는 plugin 이 무엇이고, 어디서 어떻게 로드되며, 어느 단계가 외부 트래픽을 발생시키는지를 정리한다. 폐쇄망 P0 항목 매핑은 `fork-docs/airgap-audit-2026-05-08.md` (이하 “airgap-audit”) 의 §2.2 와 cross-ref 된다.

주 코드 디렉토리: `codex-rs/core-plugins/`. 보조 크레이트: `codex-rs/plugin/` (PluginId / LoadedPlugin 타입), `codex-rs/utils/plugins/` (manifest path 발견), `codex-rs/features/` (Feature flag 정의).

---

## 1. Plugin 이란?

Plugin 은 **외부에서 받아 설치되는 능력 패키지** 다. 한 개의 plugin 디렉토리 안에 다음이 묶여 들어온다.

- **skills** — `core-skills` 가 로딩하는 skill 파일들 (`skills/` 하위, 기본 디렉토리명은 `loader.rs:47` 의 `DEFAULT_SKILLS_DIR_NAME = "skills"`).
- **MCP servers** — `.mcp.json` 또는 manifest `mcpServers` 가 가리키는 JSON. `core-plugins/src/loader.rs::load_plugin_mcp_servers` 가 읽어서 `McpServerConfig` 로 변환한다 (`loader.rs:49` `DEFAULT_MCP_CONFIG_FILE = ".mcp.json"`).
- **apps** — `.app.json` 안의 connector 정의. `AppConnectorId` 로 등록된다 (`loader.rs:50`).
- **hooks** — `hooks/hooks.json` 또는 manifest 인라인. `Feature::PluginHooks` 가 켜진 경우에만 활성 (`features/src/lib.rs:944-949`, `default_enabled: false`).
- **interface metadata** — `displayName`, `defaultPrompt`, `logo`, `screenshots` 등 UI 노출용 필드. TUI / 데스크톱 앱이 plugin picker 에 표시할 때 사용.

즉 plugin 은 “skill + tool + config 를 하나의 단위로 배포·갱신하는 컨테이너” 이고, 사용자 입장에선 `~/.codex` 또는 `~/.xtech` (이 fork) 의 `plugins/cache/...` 에 깔리는 폴더 한 개다.

식별자(`PluginId`) 는 `plugin_name@marketplace_name` 형식 (`codex-rs/plugin/src/plugin_id.rs:27,46`). 예: `github@openai-curated`, `chrome@openai-bundled`. 두 marketplace 이름 상수는 `core-plugins/src/lib.rs:19-20` 에 하드코딩되어 있고, “suggest 로 보여줘도 되는” curated/bundled plugin 화이트리스트가 같은 파일 22-38 줄에 박혀 있다 (`github`, `notion`, `slack`, `gmail`, `google-calendar`, `google-drive`, `canva`, `teams`, `sharepoint`, `outlook-email`, `outlook-calendar`, `linear`, `figma`, `chrome`, `computer-use`).

```
PluginId            "github@openai-curated"
                   ┌──────────┬──────────────┐
                   │ plugin   │ marketplace  │
                   │ _name    │ _name        │
                   └──────────┴──────────────┘

Marketplace 종류
  - openai-curated   : start_curated_repo_sync 가 GitHub 에서 받아와 stage
  - openai-bundled   : 바이너리 빌드 시 동봉되는 내장 plugin
  - 사용자 git URL   : marketplace_add 로 등록하는 외부 marketplace
```

---

## 2. 로딩 흐름 — `PluginsManager` 라이프사이클

진입은 `codex-rs/core-plugins/src/manager.rs` 의 `PluginsManager` 다. app-server / TUI / exec 모두 이 매니저 인스턴스를 공유한다 (`Arc<PluginsManager>`).

`maybe_start_plugin_startup_tasks_for_config` (`manager.rs:1386-1487`) 가 라이프사이클의 **부트스트랩** 이다. 호출자는 `codex-rs/app-server/src/message_processor.rs:416-427`. 흐름은 다음과 같다.

1. `config.plugins_enabled` (= `Feature::Plugins` 가 켜졌는지) 확인. 꺼져 있으면 모든 startup task skip — 폐쇄망 운영의 단일 차단점이 여기다.
2. **Curated repo sync** — `start_curated_repo_sync` (`manager.rs:1651`) 가 별도 OS 스레드를 띄워 `sync_openai_plugins_repo` 를 호출. process 단위 가드 `CURATED_REPO_SYNC_STARTED` (`manager.rs:79`) 로 1회만 실행. 자세한 외부 호출은 §4 참조.
3. **Marketplace auto-upgrade** — `upgrade_configured_marketplaces_for_config` 가 사용자가 등록한 git marketplace 들에 대해 `git fetch/pull` 시도 (`marketplace_upgrade/git.rs:137`). 사용자가 외부 git URL 을 등록하지 않았다면 no-op.
4. **Startup remote plugin sync** — `start_startup_remote_plugin_sync_once` (`startup_remote_sync.rs:16`). curated snapshot 이 준비된 후 `sync_plugins_from_remote` 를 호출 → 내부적으로 `remote_legacy.rs::fetch_remote_plugin_status` 가 `{chatgpt_base_url}/plugins/list` 를 GET (§5).
5. **Remote installed plugins cache refresh** — `Feature::RemotePlugin` 이 켜져 있을 때만. fork 디폴트로는 `default_enabled: false` (`features/src/lib.rs:974-979`) 라 미점화.
6. **Featured plugin ids cache warmup** — `featured_plugin_ids_for_config` (`manager.rs:1474-1485`). 사용자가 본 `failed to warm featured plugin ids cache` 경고가 정확히 이 task 의 출처. §5.

부트스트랩 이후 plugin 사용은 다음 함수들로 펼쳐진다 (모두 `manager.rs`).

- `load_plugins_from_layer_stack` — config layer stack 을 보고 활성 plugin 을 디스크에서 읽음.
- `load_plugin_skills` / `load_plugin_mcp_servers` / `load_plugin_apps` — 로드된 plugin 으로부터 skill / MCP / app 추출.
- `install_plugin` (`PluginInstallRequest`) — marketplace 에서 plugin 을 fetch 해 `plugins/cache/...` 에 풀고 `PluginStore` 에 등록.
- `clear_cache` — 파일 변경 후 in-memory 캐시 무효화.

```
[app-server boot]                      plugins_enabled?
       │                                    │
       ▼                                    │ no
maybe_start_plugin_startup_tasks_for_config ┴──► (early return — 폐쇄망 권장 경로)
       │ yes
       ├─► start_curated_repo_sync          (OS thread, §4)
       ├─► upgrade_configured_marketplaces  (사용자 git marketplace, no-op 가능)
       ├─► start_startup_remote_plugin_sync_once
       │      └─► /plugins/list  (auth=codex backend 에서만)            §5.2
       ├─► remote_installed_plugins_cache_refresh   (Feature::RemotePlugin gate)
       └─► featured_plugin_ids_for_config           (3h TTL warmup)     §5.1
              └─► /plugins/featured  ── 401 경고가 여기서 발생
```

`Feature::Plugins` flag (`features/src/lib.rs:163-164, 938-943`) 가 default `true` 인 한 위 1–6 단계가 매 codex 실행마다 시도된다. fork 의 P0 항목이 정확히 이 default 를 false 로 뒤집는 한 줄 패치다 (airgap-audit §9).

---

## 3. Manifest 포맷 — `.codex-plugin/plugin.json`

manifest path 발견은 `codex-rs/utils/plugins/src/plugin_namespace.rs:8-16` 에서 한다.

```rust
const DISCOVERABLE_PLUGIN_MANIFEST_PATHS: &[&str] =
    &[".codex-plugin/plugin.json", ".claude-plugin/plugin.json"];
```

두 경로를 순서대로 시도. 첫 번째가 존재하면 그것, 없으면 `.claude-plugin/plugin.json` (claude-code 호환) 을 fallback. 파싱은 `core-plugins/src/manifest.rs::load_plugin_manifest` (`manifest.rs:140`) 가 `RawPluginManifest` 로 deserialize 한다.

핵심 필드 (`manifest.rs:13-35`):

| 필드 | 의미 |
| --- | --- |
| `name` | plugin display name. 비어 있으면 디렉토리 이름 사용 (`manifest.rs:156-161`). |
| `version` | 옵션. 없으면 `store.rs:14` 의 `DEFAULT_PLUGIN_VERSION = "local"`. |
| `description`, `keywords` | 검색·표시용. |
| `skills` | `./skills` 같은 상대 경로 문자열. plugin root 아래로만 resolve. |
| `mcpServers` | `./.mcp.json` 등. JSON 또는 인라인 server map. |
| `apps` | `./.app.json`. connector 정의. |
| `hooks` | path / inline hooks 둘 다 지원 (`RawPluginManifestHooks`). `Feature::PluginHooks` 가 꺼져 있으면 무시. |
| `interface` | UI 메타. 아래 §3.1. |

### 3.1 `interface` 서브트리

`PluginManifestInterface` (`manifest.rs:61-77`) 가 핵심. 자주 쓰이는 필드:

- `displayName`, `shortDescription`, `longDescription`, `developerName`, `category`, `capabilities[]`
- `websiteURL`, `privacyPolicyURL`, `termsOfServiceURL` (모두 alias 로 camel + ALL CAPS 두 형태 지원, `manifest.rs:95-102`)
- `defaultPrompt` — string 또는 string list. TUI 가 plugin 카드의 “suggested prompt” 로 표시. 최대 3개, 각 128자 cap (`manifest.rs:9-10`).
- `brandColor`, `composerIcon`, `logo`, `screenshots[]` — 이미지 / 색상 자원 path. plugin root 밖으로 escape 하면 reject.

interface 필드가 모두 비면 `interface = None` 으로 떨어뜨려 빈 객체로 직렬화하지 않는다 (`manifest.rs:218-233`).

---

## 4. 마켓플레이스 / 동기화 — curated repo

`start_curated_repo_sync` 의 본체는 `core-plugins/src/startup_sync.rs::sync_openai_plugins_repo` (`startup_sync.rs:66`). 3-tier fallback 전략이다.

1. **git clone** (`startup_sync.rs:138-173`) — `git ls-remote https://github.com/openai/plugins.git HEAD` 로 remote sha 확인 후, 변경되었으면 `git clone --depth 1 https://github.com/openai/plugins.git` 을 임시 디렉토리로. 30초 timeout (`CURATED_PLUGINS_GIT_TIMEOUT`, `startup_sync.rs:28`).
2. **GitHub API HTTP** (`startup_sync.rs:175-199`) — git binary 가 없거나 실패하면 `https://api.github.com/repos/openai/plugins` → default branch → `git/ref/heads/<branch>` → `zipball/<sha>` 순서로 fetch. 30초 timeout (`CURATED_PLUGINS_HTTP_TIMEOUT`).
3. **chatgpt.com export archive** (`startup_sync.rs:201-222`) — 위 둘 다 실패 + 로컬 snapshot 도 없을 때만 시도. `https://chatgpt.com/backend-api/plugins/export/curated` 를 GET 하면 `{ download_url }` 응답 → 그 URL 에서 zip 다운로드. 로컬 snapshot 이 이미 있으면 archive fallback 은 skip 한다 (`startup_sync.rs:102-110`) — “lagging backup path” 라는 주석.

**폐쇄망 동작**: 세 단계 모두 외부 도메인 (`github.com`, `api.github.com`, `chatgpt.com`) 을 향한다. 폐쇄망 머신에선 단계마다 30s timeout → fail → fallback 으로 진행되며 최악의 경우 직렬 90s warn 후 종료. fatal 은 아니지만 매 codex 실행마다 stderr 가 시끄러워진다.

성공 시 결과는 `~/.xtech/.tmp/plugins/` 에 stage 되고 sha 는 `~/.xtech/.tmp/plugins.sha` (`startup_sync.rs:25-26`). manifest 는 `.agents/plugins/marketplace.json` 위치에 있어야 함 (`startup_sync.rs:371-379`).

---

## 5. Featured plugin / installed plugin REST

curated repo 와 별개로 ChatGPT 백엔드의 plugin metadata API 를 두 군데서 호출한다.

### 5.1 `/plugins/featured`

`core-plugins/src/remote_legacy.rs::fetch_remote_featured_plugin_ids` (`remote_legacy.rs:157-195`).

- URL: `{chatgpt_base_url}/plugins/featured?platform=codex`. 기본 `chatgpt_base_url = https://chatgpt.com/backend-api/` (`exec/src/lib.rs:353`, `login/src/auth/manager.rs:93`).
- 인증: 있으면 codex backend auth header 첨부, 없어도 GET 시도 (`remote_legacy.rs:173-176`).
- 트리거: §2 의 step 6 — `maybe_start_plugin_startup_tasks_for_config` 의 마지막 `tokio::spawn`.
- TTL: `FEATURED_PLUGIN_IDS_CACHE_TTL = 3h` (`manager.rs:80-81`). 첫 startup 에 fetch, 이후 3시간 동안 in-memory 재사용.
- **이 fork 운영에서 본 401 경고의 출처** — Ollama API key 인증 모드는 `auth.uses_codex_backend()` 가 false 라 list/mutation 은 silent skip 되지만 `featured` 는 unauth 로 GET 을 시도하다 ChatGPT 가 401 반환 → `failed to warm featured plugin ids cache` warn.

### 5.2 `/plugins/list`, `/plugins/{id}/{enable|uninstall}`

- `fetch_remote_plugin_status` (`remote_legacy.rs:119-155`) → `GET {base}/plugins/list`.
- `enable_remote_plugin` / `uninstall_remote_plugin` (`remote_legacy.rs:197-213`) → `POST {base}/plugins/{id}/{action}`.
- 모두 `auth.uses_codex_backend()` 가 true 일 때만 실제 호출. fork 의 API key 모드에서는 silent skip.

### 5.3 `/plugins/installed` (RemotePlugin 실험 경로)

`crate::remote::fetch_remote_installed_plugins` (manager.rs:1709) — `Feature::RemotePlugin` 이 켜진 경우만. 디폴트 false 이므로 fork 에서는 미점화.

---

## 6. 로컬 plugin 디렉토리 레이아웃

| 경로 | 내용 |
| --- | --- |
| `~/.xtech/.tmp/plugins/` | curated marketplace snapshot. `git clone` / zipball 압축 해제 결과. `.agents/plugins/marketplace.json` 가 manifest. (`startup_sync.rs:25,55`) |
| `~/.xtech/.tmp/plugins.sha` | 위 snapshot 의 git HEAD sha. 캐시 invalidation 용. (`startup_sync.rs:26`) |
| `~/.xtech/plugins/cache/<marketplace>/<plugin_name>/<version>/` | 설치된 plugin 의 실제 파일들. `PluginStore::plugin_root` (`store.rs:51-58`). |
| `~/.xtech/plugins/data/<plugin_name>-<marketplace>/` | plugin 이 런타임에 쓸 수 있는 user data 영역. (`store.rs:61-66`) |
| `~/.xtech/.tmp/app-server-remote-plugin-sync-v1` | startup remote sync 가 1회 성공했음을 표시하는 marker. 다음 실행 때 prerequisite 체크에 쓰임. (`startup_remote_sync.rs:13`) |

`PluginStore` 는 `codex_home + plugins/cache` 를 root 로 쓰고 (`store.rs:15-16,37-44`), `active_plugin_version` 이 디렉토리명 정렬로 “가장 최신 버전” 을 결정한다 (`store.rs:68-90`). 사용자가 직접 만진 plugin 도 같은 위치를 쓰면 된다.

---

## 7. 폐쇄망 운영 — 끄는 위치와 상실 항목

**끄는 위치는 한 줄**: `codex-rs/features/src/lib.rs:938-943` 의 `Feature::Plugins` spec.

```rust
FeatureSpec {
    id: Feature::Plugins,
    key: "plugins",
    stage: Stage::Stable,
    default_enabled: true,   // ← 여기를 false 로
},
```

이 한 줄을 false 로 바꾸면 `config.features.enabled(Feature::Plugins)` 가 false 가 되고, `config.plugins_config_input().plugins_enabled` 가 false 가 되므로 `maybe_start_plugin_startup_tasks_for_config` 의 outer `if config.plugins_enabled` 블록 (`manager.rs:1392`) 전체가 skip 된다. §2 의 step 1–6 모두 차단.

추가로 features 가 켜져도 chatgpt_base_url 이 비어 있으면 leak 을 막을 수 있도록 fork-only 가드를 `remote_legacy.rs::fetch_remote_featured_plugin_ids` 에 넣는 보수적 옵션이 airgap-audit §9 에 정리되어 있다.

**끄면 상실되는 것**:

- TUI 의 `/plugins` slash command — `chatwidget.rs:9433` 의 `set_plugins_command_enabled` 가 false 로 들어가 메뉴에서 사라진다 (`tui/src/chatwidget/slash_dispatch.rs:882`).
- plugin picker / install UI (TUI 와 데스크톱 앱) — `tui/src/chatwidget.rs:9807,10637`, `tui/src/app/event_dispatch.rs:1223`, `tui/src/app/background_requests.rs:387` 가 모두 early-return.
- 마켓플레이스에서 받아오는 default skill / MCP / app connector 묶음. **단 사용자 자체 skill (`~/.xtech/skills/`) 과 사용자 자체 MCP (`config.toml::mcp_servers`) 는 영향 없음** — 이들은 `core-skills` 와 `codex-mcp` 가 직접 로드한다.
- App connector (Github, Notion, Slack, Gmail 등 — `core-plugins/src/lib.rs:22-38` 의 `TOOL_SUGGEST_DISCOVERABLE_PLUGIN_ALLOWLIST`). 폐쇄망에서는 어차피 인증·토큰 흐름이 동작 안 하므로 손실이 아니다.

요약: **상실 항목 ≈ 외부 SaaS 와 묶인 connector + curated 카탈로그 UI**. 사내 skill / MCP / config 는 그대로다.

---

## 8. fork 의 미해결 항목 (P0)

`fork-docs/airgap-audit-2026-05-08.md` §2.2 에 잡힌 P0 두 건이 plugin 영역의 기술 부채다.

- **#2 — curated plugin 동기화 (airgap-audit §2.2.1)**: `start_curated_repo_sync` 가 매 startup `https://github.com/openai/plugins.git` 을 시도. **현재 default 가 켜져 있음**. 권장 패치는 위 §7 의 한 줄 (`features/src/lib.rs:942` `default_enabled: false`).
- **#3 — featured / installed plugin REST 동기화 (airgap-audit §2.2.2)**: `featured_plugin_ids_for_config` 가 `chatgpt.com/backend-api/plugins/featured` 를 GET 하다 401 경고를 남김. 위 한 줄로 함께 해소되지만, 보수적으로 `remote_legacy.rs::fetch_remote_featured_plugin_ids` 진입에서 `if config.chatgpt_base_url.is_empty() { return Ok(vec![]); }` 가드 추가가 추천됨.

두 항목 모두 “디폴트가 외부 호출을 켠 상태로 남아 있고, fork 가 아직 한 줄 뒤집기를 안 했다”가 핵심이다. 패치 적용 시 §7 의 상실 항목만 trade-off 로 받으면 plugin 쪽 외부 트래픽은 0 으로 떨어진다. airgap-audit §9 의 검증 절차 (RUST_LOG=info + grep `chatgpt.com|api.github.com`) 가 그대로 적용된다.

---

## Cross-references

- `fork-docs/airgap-audit-2026-05-08.md` §2.2 (이 문서의 P0 매핑) / §9 (검증 명령).
- `fork-docs/arch/01-overview.md` §1.1 (core-plugins 가 core 에서 떨어져 나온 분리 산물이라는 맥락).
- `fork-docs/arch/06-config.md` (Feature flag 가 config layer 에 어떻게 노출되는지).
- `fork-docs/work-log-2026-05-08.md` (LLM 라우팅 fork 작업과 plugin 끄기 작업의 우선순위 관계).
