# 10. Skills subsystem

이 문서는 xtech 의 **Skills** 서브시스템을 정리한다. Skill 은 사용자/조직이 정의한 prompt 조각을 system / developer / user 메시지에 동적으로 끼워 넣어 모델 동작을 특정 작업에 맞춰 “onboarding” 하는 메커니즘이다. 비슷한 시스템인 **Plugins** 는 별도 문서 (`11-plugins.md`) 에서 다루며, 이 둘이 만나는 경계 (plugin 이 자체 skills 를 export 하는 경우) 만 여기서 cross-ref 한다.

주요 코드:

- `codex-rs/skills/` (crate `codex-skills`) — 임베디드 system skills 의 install/cache (`include_dir!` 로 묶어둔 샘플을 `~/.xtech/skills/.system` 에 풀어두는 일).
- `codex-rs/core-skills/` (crate `codex-core-skills`) — discovery (`loader.rs`), 캐시/머지 (`manager.rs`), prompt 합성 (`injection.rs`, `render.rs`), config 룰 (`config_rules.rs`).
- `codex-rs/core/src/skills.rs` — core 가 위 crate 를 re-export 하면서 turn 단위로 dependency env / implicit invocation 같은 부가 기능을 얹는 layer.
- `codex-rs/core/src/context/skill_instructions.rs`, `available_skills_instructions.rs` — 합성된 텍스트가 실제로 어느 role 의 contextual fragment 로 들어가는지 결정.

## 1. Skill 이란?

Skill 은 **YAML frontmatter 가 있는 `SKILL.md` 한 개 + 부속 파일들** 로 구성된 디스크상의 디렉터리다. frontmatter 는 모델이 “이 skill 이 무엇을 하는지” 판단할 수 있도록 짧은 `name` / `description` 을 제공하고, body 는 모델이 실제로 호출했을 때 펼쳐 읽을 instructions 를 담는다 — 우리는 이걸 **prompt-skill** 이라 부른다.

Codex 가 skill 을 인식하면 두 단계로 모델에게 노출된다:

1. **Available skills**: 매 턴 `developer` role 메시지로 “현재 세션에서 사용 가능한 skill 목록 (이름 + description + 경로)” 가 주입된다 (`codex-rs/core/src/context/available_skills_instructions.rs`, `<SKILLS_INSTRUCTIONS>` 태그).
2. **Skill body 주입**: 사용자가 `$skill-name` mention 또는 explicit selection 으로 skill 을 명시하면, 그 skill 의 `SKILL.md` 본문이 `<skill>...<name>...<path>...본문</skill>` 형태로 `user` role 메시지에 추가된다 (`skill_instructions.rs`).

즉 “system prompt 에 동적으로 들어가는 사용자 정의 instructions” 라는 개념적 요약은 맞지만, 정확히는 **developer role 에 metadata 카탈로그 + user role 에 본문** 으로 나뉜다.

## 2. 로딩 위치 (skill roots)

`codex-core-skills::loader::skill_roots_with_home_dir` 가 진입점이며, **여러 root 디렉터리를 모아 각각을 BFS 로 훑어** `SKILL.md` 를 발견한다. root 는 다음 출처에서 모인다 (`codex-rs/core-skills/src/loader.rs:248`).

| 출처 | 경로 | scope |
| --- | --- | --- |
| User config layer (legacy) | `${CODEX_HOME}/skills/` (= `~/.xtech/skills/`) | `User` |
| User config layer (표준) | `${HOME}/.agents/skills/` | `User` |
| Embedded system skills | `${CODEX_HOME}/skills/.system/` | `System` |
| Project layer | `<project>/.codex/skills/` (config 가 있는 모든 ancestor) | `Repo` |
| Repo `.agents/skills` | cwd 부터 project root 까지 모든 디렉터리의 `<dir>/.agents/skills/` | `Repo` |
| System config layer | `/etc/codex/skills/` | `Admin` |
| Plugin-bundled | 각 plugin manifest 의 `skills` 필드가 가리키는 디렉터리 | `User` (plugin namespace 부착) |

Plugin 이 export 하는 skill 은 `codex-utils-plugins::PluginSkillRoot` 로 들어와 `SkillRoot { plugin_id: Some(...), scope: User }` 가 되고, `loader::namespaced_skill_name` 이 이름 앞에 plugin namespace 를 prefix 로 붙인다 (`namespace:skill-name`).

각 root 는 `MAX_SCAN_DEPTH = 6`, `MAX_SKILLS_DIRS_PER_ROOT = 2000` 으로 제한된 BFS 로 스캔되며, dot-prefixed 디렉터리는 건너뛴다. system scope 만 symlink 추적을 하지 않는다 (system skills 는 codex 자신이 쓴 것이므로).

### Embedded (system) skills

`codex-rs/skills/src/lib.rs` 가 `include_dir!("$CARGO_MANIFEST_DIR/src/assets/samples")` 로 바이너리에 묶어둔 샘플들 — 현재 `imagegen`, `openai-docs`, `plugin-creator`, `skill-creator`, `skill-installer`, `test-scenario` — 을 `${CODEX_HOME}/skills/.system/` 에 한 번만 풀어둔다. fingerprint marker (`.codex-system-skills.marker`) 가 일치하면 재설치를 건너뛰고, `skills.bundled.enabled = false` 면 `uninstall_system_skills` 로 그 디렉터리를 제거한다 (`SkillsManager::new_with_restriction_product`).

### Project-local (xtech 의 CLAUDE.md 동등물)

upstream 의 `AGENTS.md` / 우리 `CLAUDE.md` 와 달리 skills 는 “단일 파일” 이 아니라 **디렉터리 트리** 이므로, project-local skill 은 `<repo>/.codex/skills/<skill-name>/SKILL.md` 또는 `<repo>/.agents/skills/<skill-name>/SKILL.md` 에 둔다. 후자가 권장 위치 (`AGENTS_DIR_NAME = ".agents"`).

## 3. Manifest 포맷

한 skill 디렉터리는 다음과 같이 생겼다:

```
my-skill/
├── SKILL.md           # 필수. YAML frontmatter + Markdown body
├── agents/
│   └── openai.yaml    # optional. interface / dependencies / policy
├── assets/            # icon / 템플릿 등 (interface.icon_* 가 참조)
├── scripts/           # SKILL.md 가 실행 가이드로 쓰는 스크립트
└── references/        # SKILL.md 가 가리키는 보조 문서
```

`SKILL.md` 의 frontmatter (loader.rs:38, `SkillFrontmatter`):

```yaml
---
name: skill-creator                    # optional, 없으면 디렉터리 이름
description: Guide for creating ...    # MAX 1024 chars
metadata:
  short-description: Create or update a skill   # optional, MAX 1024
---
```

선택적 `agents/openai.yaml` 은 (loader.rs:54, `SkillMetadataFile`):

```yaml
interface:
  display_name: "Image Gen"            # MAX 64
  short_description: "..."             # MAX 1024
  icon_small: "./assets/imagegen-small.svg"   # 반드시 ./assets/ 하위
  icon_large: "./assets/imagegen.png"
  brand_color: "#FF6B00"               # #RRGGBB 형식만
  default_prompt: "Use $imagegen ..."  # MAX 1024 chars
dependencies:
  tools:
    - type: env_var
      value: OPENAI_API_KEY
      description: Required for image generation
policy:
  allow_implicit_invocation: true
  products: [codex]                    # 비어있으면 모든 product 에 노출
```

길이 / 형식 제약 (`codex-rs/core-skills/src/loader.rs:106-120`):

| 필드 | 제한 |
| --- | --- |
| `name` | 64 chars (MAX_NAME_LEN) |
| `description` | 1024 chars (MAX_DESCRIPTION_LEN) |
| `metadata.short-description` | 1024 chars |
| `interface.display_name` | 64 chars |
| `interface.short_description` | 1024 chars |
| `interface.default_prompt` | **1024 chars** (`MAX_DEFAULT_PROMPT_LEN`) |
| `dependencies.tools.*` | 64 (type/transport) / 1024 (그 외) |

### Skills vs Plugins 의 default_prompt

플러그인 매니페스트 (`codex-rs/core-plugins/src/manifest.rs:9-10`) 도 `interface.defaultPrompt` 를 갖지만, 거기는 **최대 3개 / 각 128자** 라는 더 엄격한 제한을 쓴다. 우리가 테스트 중 본 “maximum of 3 prompts is supported” 경고는 **plugin** 매니페스트 쪽에서 떨어진 것이고, skill 쪽 1024-자 초과 경고는 `core-skills::loader::resolve_str` 의 `tracing::warn!("ignoring {field}: exceeds maximum length of {max_len} characters")` (loader.rs:923) 가 발생원이다. 두 시스템의 구체적 위치를 헷갈리지 않게 매핑해 둔다:

- `core-skills::loader::resolve_str` → skill 의 `interface.default_prompt` 1024 초과 시 silently drop + warn.
- `core-plugins::manifest::resolve_default_prompts` (manifest.rs:301) → plugin 의 `interface.defaultPrompt` 가 4개째일 때, 또는 한 prompt 가 128자를 넘을 때 drop + warn.

## 4. 로딩 우선순위와 충돌 해결

`load_skills_from_roots` 는 모든 root 의 모든 skill 을 먼저 모은 뒤 **dedupe** 하고 정렬한다 (`loader.rs:159-228`).

1. **Dedupe**: 같은 `path_to_skills_md` 가 두 번 들어오면 (ex. 동일 디렉터리가 user / repo 양쪽에 보이는 경우) 첫 등장만 유지. canonicalize 결과 기준이라 symlink 도 중복 제거된다.
2. **Sort**: scope rank → `Repo (0) < User (1) < System (2) < Admin (3)` 순으로, 같은 scope 안에선 이름 알파벳 순. **즉 같은 이름의 skill 이 repo 에도 있고 user 에도 있다면 두 entry 가 둘 다 살아남으며**, 모델에 노출되는 카탈로그에 두 줄로 나타난다. 이름 충돌은 `mention_counts.rs` 의 `build_skill_name_counts` 가 카운트해, 사용자 입력에서 plain `$name` mention 을 받았을 때 **count != 1 이면 ambiguous 로 무시** 하고 explicit `[$name](path)` 형태만 받는다 (`injection.rs::select_skills_from_mentions:378-390`).
3. **Disable 룰**: `~/.xtech/config.toml` (또는 SessionFlags) 의 `[[skills.config]]` 항목으로 path 또는 name selector 를 써서 enabled=false 시킬 수 있다 (`config_rules.rs`). 이 룰은 layer precedence 순으로 처리되고 같은 selector 의 뒤 entry 가 앞을 덮는다.
4. **Product restriction**: `policy.products` 가 명시돼 있고 현재 product 와 매치하지 않으면 `filter_skill_load_outcome_for_product` 가 통째로 제거.

## 5. System prompt 합성

`SkillsManager::skills_for_cwd` 가 `SkillLoadOutcome` 을 만들어 `cache_by_cwd` 또는 `cache_by_config` 에 캐시한 뒤, turn 시작 시점에 두 군데서 활용된다.

### 5-a. Available skills 카탈로그 (developer role)

`codex-core-skills::render::build_available_skills` 가 (`SkillScope` 무관하게) `allow_implicit_invocation = true` 인 skill 을 모아 “name | description | path” 표 형태로 렌더한다. 토큰 예산은 `default_skill_metadata_budget` — context window 의 **2%** 또는 8000자 (window 정보가 없을 때) — 이고, 초과 시 description 을 잘라내거나 (`render_lines_with_description_budget`) 끝의 항목을 통째로 drop 한다. drop 이 발생하면 “Exceeded skills context budget. ...” 경고가 fragment 마지막에 붙는다.

이 결과는 `AvailableSkillsInstructions` 로 감싸 `<SKILLS_INSTRUCTIONS> ... </SKILLS_INSTRUCTIONS>` 태그로 **developer role** 메시지에 들어간다 (`available_skills_instructions.rs:24` `const ROLE: &'static str = "developer"`).

### 5-b. 명시적으로 호출된 skill 본문 (user role)

사용자가 `$skill-name` 을 텍스트에 쓰거나 UI 에서 skill 선택 (`UserInput::Skill { name, path }`) 을 한 경우, `injection::collect_explicit_skill_mentions` 가 후보를 추리고 `build_skill_injections` 가 각 skill 의 `SKILL.md` 전체를 읽어 다음 형태로 user 메시지에 끼워 넣는다 (`skill_instructions.rs`):

```
<skill>
<name>skill-creator</name>
<path>/Users/me/.xtech/skills/.system/skill-creator/SKILL.md</path>
... SKILL.md 본문 ...
</skill>
```

읽기에 실패하면 (파일 누락 등) `SkillInjections.warnings` 에 메시지가 추가되고 OTel `codex.skill.injected` counter 가 `status="error"` 로 올라간다.

## 6. 예외 / 무시 케이스 — warning 이 어디서 나오는지

| 상황 | 동작 | 발생 위치 |
| --- | --- | --- |
| frontmatter 누락 / 깨짐 | skill 자체가 outcome.errors 에 들어가고 user 에게 노출 (단 system scope 는 silent skip) | `loader.rs::parse_skill_file:610` |
| `name` 이 64자 초과 | 해당 skill 전체가 error 로 떨어짐 (load 실패) | `validate_len`, `loader.rs:898` |
| `interface.default_prompt` 1024 초과 | 그 필드만 drop + warn `ignoring interface.default_prompt: exceeds maximum length of 1024 characters` | `resolve_str`, `loader.rs:915-927` |
| `interface.icon_*` 가 절대경로 / `..` 포함 / `assets/` 하위가 아님 | 아이콘만 drop + warn | `resolve_asset_path`, `loader.rs:846-892` |
| `interface.brand_color` 가 `#RRGGBB` 가 아님 | 색만 drop + warn | `resolve_color_str`, `loader.rs:941-955` |
| BFS 스캔이 2000 디렉터리 초과 | 그 시점에서 truncate + warn `skills scan truncated after 2000 directories ...` | `loader.rs:590-596` |
| 카탈로그가 토큰 예산 초과 | description 축약 또는 항목 drop + 카탈로그 끝에 warning 문자열 | `render.rs::build_available_skills_from_lines:213-243` |
| Plugin manifest 의 `defaultPrompt` 가 4개째 | 그 prompt drop + warn `maximum of 3 prompts is supported` | `core-plugins::manifest.rs:313-317` (skills 와는 별도) |

“실제 테스트에서 본 warning” 들의 발생 위치는 위 표의 `loader.rs` 또는 `manifest.rs` 둘 중 하나다 — `tracing` 로깅이 `RUST_LOG=codex_core_skills::loader=warn,codex_core_plugins::manifest=warn` 로 모두 나온다.

## 7. 사용자가 skill 추가하기

가장 단순한 흐름 — “이 사용자에게만 보이는 prompt-skill 하나 추가” :

```bash
mkdir -p ~/.agents/skills/my-style
cat > ~/.agents/skills/my-style/SKILL.md <<'EOF'
---
name: my-style
description: Apply our team's commit message style
metadata:
  short-description: Team commit style
---

# My Style

When asked to write commit messages, follow these rules:
- Subject line ≤ 50 chars, imperative mood.
- Body wraps at 72 chars.
- ...
EOF
```

이대로 두면 다음 세션 시작 시 `SkillsManager` 가 발견하고 카탈로그에 `my-style` 항목이 표시된다. 사용자가 “$my-style 으로 커밋 메시지 정리해줘” 라고 입력하면 `injection::collect_explicit_skill_mentions` 가 본문을 user 메시지에 주입한다.

interface 메타나 dependency 가 필요하면 `~/.agents/skills/my-style/agents/openai.yaml` 을 추가한다:

```yaml
interface:
  display_name: "My Style"
  short_description: "Team commit style"
  default_prompt: "Use $my-style for commit messages."
policy:
  allow_implicit_invocation: true
```

프로젝트 단위로 공유하려면 같은 디렉터리 구조를 `<repo>/.agents/skills/<name>/` 또는 `<repo>/.codex/skills/<name>/` 에 두고 git 에 커밋한다. trust 가 통과된 프로젝트만 repo 스코프 skill 이 활성화된다 (config layer 자체가 trust 게이팅을 따르므로).

비활성화는 `~/.xtech/config.toml`:

```toml
[[skills.config]]
name = "imagegen"
enabled = false

[[skills.config]]
path = "/path/to/some/repo/.agents/skills/legacy/SKILL.md"
enabled = false
```

embedded system skills 를 통째로 끄려면:

```toml
[skills.bundled]
enabled = false
```

이 경우 `SkillsManager::new_with_restriction_product` 가 `~/.xtech/skills/.system/` 디렉터리를 지운다.

## 8. 관련 문서

- `11-plugins.md` — plugin 매니페스트의 `skills` 필드가 어떻게 PluginSkillRoot 로 변환되어 위 loader 의 입력이 되는지, 그리고 plugin 전용 `defaultPrompt` 제약 (3개 / 128자) 의 별도 경로.
- `06-config.md` §`skills.config` / `skills.bundled` 절 — disable 룰의 layer precedence 동작.
- 이 fork 에서 추가된 “remote chat-completions Qwen 게이트웨이” 는 skill 동작에 직접 영향 없음 — skill body 는 마지막 user 메시지에 그대로 들어가고 그 뒤로의 wire format 은 `03-wire-protocol.md` 를 따른다.
