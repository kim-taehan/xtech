# 19. Cloud Tasks 서브시스템

이 문서는 `codex cloud` (alias `cloud-tasks`) 서브커맨드 — 일명 "Codex Cloud" — 가 fork 안에서 어떤 역할을 하는지 정리한다. 결론을 먼저 적자면 **이 서브시스템은 OpenAI 가 호스팅하는 원격 ChatGPT backend (chatgpt.com/backend-api) 의 클라이언트일 뿐이며, xtech fork 가 운영하는 폐쇄망 환경에서는 점화될 일이 없고 끄는 것이 안전하다**. 그럼에도 mock 백엔드가 분리되어 있어 UI 자체를 데모 목적으로는 살릴 수 있다.

연관 audit: `fork-docs/airgap-audit-2026-05-08.md` §2.6.

## 1. Cloud Tasks 가 무엇인가

OpenAI Codex 의 "원격 코딩 작업" 기능에 대응하는 클라이언트다. 사용자가 codex CLI 안에서 task 를 만들면, 실제 LLM 실행은 사용자 머신이 아니라 **ChatGPT 서버 측에서 백그라운드로 돌아간다**. CLI 는 그 결과 (assistant 메시지 + git unified diff) 를 풀링하다가 로컬 워크트리에 패치로 적용한다. 즉 평소의 `codex` / `codex exec` (로컬 turn loop) 와는 완전히 다른 흐름이다.

지원 동작 (`codex-rs/cloud-tasks/src/cli.rs:15-27`):

- `exec` — 새 task 를 원격 환경에 제출 (best-of-N attempts 지원, 1-4)
- `status` — task 메타데이터 조회
- `list` — task 목록 조회 (env 필터 + cursor 페이지네이션, 최대 20)
- `diff` — 결과 unified diff 출력
- `apply` — 결과 diff 를 로컬 워크트리에 git apply

서브커맨드 없이 그냥 `codex cloud` 만 실행하면 ratatui 기반 TUI 가 떠서 같은 작업들을 인터랙티브로 한다 (`codex-rs/cloud-tasks/src/app.rs`, `ui.rs`).

이 기능이 일반 `codex` / `codex exec` 와 결정적으로 다른 점:

- 사용자 머신은 prompt 와 git ref 만 서버로 보낸다. 실제 LLM 호출 / tool 호출은 ChatGPT 인프라가 별도 컨테이너 안에서 수행한다.
- 결과는 git unified diff 한 덩어리 + assistant 텍스트 메시지 형태. 즉 "agent run 의 산출물을 patch 단위로 받아서 로컬에 적용" 모델이지 실시간 turn streaming 이 아니다.
- best-of-N (1-4) 옵션이 있고 sibling attempts 를 추후 비교 가능 — 일반 turn loop 는 그런 개념이 없다.
- 따라서 "원격 실행 / 배경 작업 큐" 기능이 맞다.

## 2. 크레이트 구성

cloud-tasks 는 정확히 세 크레이트로 분리되어 있다.

| 크레이트 | 경로 | 역할 |
| --- | --- | --- |
| `codex-cloud-tasks` | `codex-rs/cloud-tasks/` | TUI + CLI 진입점, 상태머신, ratatui 렌더링. `Cli`/`run_main` 만 외부 노출. `lib.rs` 약 2400 LOC, `app.rs`/`ui.rs` 가 TUI 본체. |
| `codex-cloud-tasks-client` | `codex-rs/cloud-tasks-client/` | 백엔드 추상화 (`CloudBackend` trait) + HTTP 구현 (`HttpClient`). `api.rs` 가 도메인 타입, `http.rs` 가 실제 reqwest 호출. |
| `codex-cloud-tasks-mock-client` | `codex-rs/cloud-tasks-mock-client/` | `CloudBackend` 의 in-memory 더미 구현. `MockClient` 한 개 타입으로, 약 200 LOC. |

`codex-cloud-tasks` 는 `codex-cloud-tasks-client` 의 `CloudBackend` trait 만 의존하고, 실제 구현체 (HTTP / Mock) 는 런타임에 `Arc<dyn CloudBackend>` 로 주입된다 (`codex-rs/cloud-tasks/src/lib.rs:39, 56-60, 63`). 즉 백엔드 선택이 `init_backend` 한 곳에 격리되어 있다 — 폐쇄망 패치 포인트로 깔끔하다.

`Cargo.toml` 에는 mock client 가 dev-dep 가 아니라 일반 dep 로 들어있는데, 같은 파일에 `# TODO: codex-cloud-tasks-mock-client should be in dev-dependencies` 주석이 있다 (`codex-rs/cloud-tasks/Cargo.toml:20-21`). 즉 release 빌드에도 mock 코드가 끌려 들어온다.

## 3. 외부 통신

기본 base URL 은 **하드코딩된 `https://chatgpt.com/backend-api`** 이다 (`codex-rs/cloud-tasks/src/lib.rs:50`, 그리고 동일 문자열이 lib.rs 안에서만 8 군데 반복 — :842, :857, :1083, :1467, :1655, :1832, :2315). `CODEX_CLOUD_TASKS_BASE_URL` 환경변수로 덮어쓸 수는 있다.

실제 HTTP 호출은 `codex-cloud-tasks-client::HttpClient` 가 직접 하지 않고 **`codex-backend-client::Client`** 에 위임한다 (`codex-rs/cloud-tasks-client/src/http.rs:18-33`). backend-client 가 base URL 을 보고 두 가지 path style 중 하나로 분기한다 (`codex-rs/backend-client/src/client.rs:106-111`):

- `*/backend-api` → `/wham/...` 경로 (ChatGPT 운영 환경)
- 그 외 → `/api/codex/...` 경로 (구 Codex API 호환)

호출되는 엔드포인트 (backend-client/src/client.rs):

| 동작 | wham 경로 | codex-api 경로 |
| --- | --- | --- |
| list | `/wham/tasks/list` | `/api/codex/tasks/list` |
| details | `/wham/tasks/{id}` | `/api/codex/tasks/{id}` |
| sibling turns | `/wham/tasks/{id}/turns/{tid}/sibling_turns` | `/api/codex/tasks/{id}/turns/{tid}/sibling_turns` |
| create | `/wham/tasks` | `/api/codex/tasks` |
| environments | `/wham/environments`, `/wham/environments/by-repo/...` | `/api/codex/environments...` (`codex-rs/cloud-tasks/src/env_detect.rs:36-74`) |

인증은 ChatGPT OAuth 전용이다 — `init_backend` 가 `auth.uses_codex_backend()` 가 false 이면 `eprintln!("Not signed in...")` 후 `std::process::exit(1)` 한다 (`codex-rs/cloud-tasks/src/lib.rs:90-95`). `uses_codex_backend()` 는 ChatGPT / ChatgptAuthTokens / AgentIdentity 모드에서만 true 이고 (`codex-rs/login/src/auth/manager.rs:292-297`), **API key (= fork 의 Ollama / 사내 게이트웨이 경로) 사용자에게는 항상 false** — 따라서 fork 의 디폴트 인증으로는 `codex cloud` 가 그 자리에서 fail-fast 한다.

`env_detect.rs` 는 추가로 `git remote -v` 를 실행해 GitHub origin 을 파싱한 뒤 `by-repo/github/{owner}/{repo}` 엔드포인트를 친다 (`codex-rs/cloud-tasks/src/env_detect.rs:30-67`). 즉 사내 git host 만 등록된 상태에선 어차피 의미 있는 매칭이 안 된다.

## 4. 데이터 흐름

1. 사용자가 `codex cloud exec --env <id> "prompt"` 실행 → `run_exec_command` (`lib.rs:157-180`).
2. `init_backend` 가 ChatGPT OAuth 토큰을 로드하고 `HttpClient` 를 만든다.
3. `resolve_environment_id` 가 `/wham/environments` 를 쳐서 env 가 실재하는지 검증.
4. `resolve_git_ref` 가 로컬 `git` 브랜치를 조회 (`codex-git-utils`).
5. `CloudBackend::create_task(env_id, prompt, git_ref, qa_mode, best_of_n)` → `POST /wham/tasks` 로 prompt 전송. 응답은 task id 한 개뿐.
6. 실제 LLM turn 은 **ChatGPT 서버 측에서 비동기로 실행**된다 (best-of-N 이면 N 개 attempt 가 병렬). 결과는 task 의 turn record 에 누적.
7. CLI 는 task URL (`{base_url 의 호스트}/codex/tasks/{id}`) 만 출력하고 종료. 결과를 보려면 `status` / `diff` / `apply` 를 다시 호출하거나 TUI 를 띄운다.
8. `apply` 는 backend 에서 unified diff 를 받아 `codex-git-utils::apply_git_patch` 로 로컬 워크트리에 적용하고 결과를 `ApplyOutcome` (status: Success/Partial/Error, conflict_paths) 로 반환 (`codex-rs/cloud-tasks-client/src/http.rs:99-113`, `apply_git_patch`).

요점: **task spec/실행/결과 모두 OpenAI 서버 측에 저장된다.** 로컬 디스크에 캐시되는 건 디버그용 `append_error_log` 라인 정도 (`codex-rs/cloud-tasks/src/util.rs`).

## 5. CLI 진입점

`codex-rs/cli/src/main.rs:159-161`:

```rust
/// [EXPERIMENTAL] Browse tasks from Codex Cloud and apply changes locally.
#[clap(name = "cloud", alias = "cloud-tasks")]
Cloud(CloudTasksCli),
```

dispatch 는 `main.rs:1039-1051` 에서 `codex_cloud_tasks::run_main` 으로 넘긴다. fork 가 새로 도입한 `--remote` 모드와는 호환되지 않아 `reject_remote_mode_for_subcommand("cloud", ...)` 로 차단해 둔다 — 즉 fork 의 원격 게이트웨이 모드를 켜고 동시에 `codex cloud` 를 쓰는 조합은 이미 막혀 있다.

서브커맨드 트리는 `codex-rs/cloud-tasks/src/cli.rs` 의 `Command` enum: `Exec | Status | List | Apply | Diff`. 인자 없이 실행하면 (`Command::None`) TUI 모드로 떨어진다 (`lib.rs:756 init_backend("codex_cloud_tasks_tui")`).

xtech 명으로는 `xtech cloud ...` 또는 `xtech cloud-tasks ...` (alias) 로 노출된다.

## 6. 이 fork 에서의 의미 — 폐쇄망 가설 검증

가설: **폐쇄망에선 끄는 게 맞다.** 검증 결과 — 그렇다.

근거:

1. base URL 이 `chatgpt.com/backend-api` 로 하드코딩되어 있어 사내 게이트웨이 도메인으로 자동 라우팅되지 않는다 (`CODEX_CLOUD_TASKS_BASE_URL` 로 명시 override 해야 하는데, 사내에 `/wham/tasks/list` 같은 호환 엔드포인트가 없다).
2. `init_backend` 가 ChatGPT OAuth 토큰을 강제 요구한다 (§3). fork 의 표준 인증인 OPENAI_API_KEY (Ollama 경유) 로는 즉시 `exit(1)`.
3. 환경 자동탐지가 GitHub origin 기반 — 사내 git host 에서는 매칭 자체가 무의미.
4. 결과 fetch / apply 도 ChatGPT 서버에 task record 가 존재해야만 의미 있음.

권장 조치 (airgap-audit §2.6 와 동일 방향):

- 단기: `codex-rs/cli/src/main.rs:160` 의 `#[clap(name = "cloud", ...)]` 위에 `#[clap(hide = true)]` 추가해 `--help` 에서 가린다. 동작은 그대로 둔다 (어차피 fail-fast).
- 중기: subcommand 자체를 `#[cfg(feature = "cloud-tasks")]` 뒤로 빼고, fork 빌드의 default-features 에서 제거. 빌드에서 `codex-cloud-tasks*` 세 크레이트가 빠져 바이너리 크기도 줄어든다.
- 장기 (선택): 사내 task 큐를 진짜로 운용하고 싶다면 `CloudBackend` trait 의 사내 구현 한 개만 새로 작성하고 `init_backend` 의 분기에 끼우면 된다 — 표면이 trait 한 개라 비용이 작다.

## 7. mock client 의 역할

`codex-cloud-tasks-mock-client::MockClient` (`codex-rs/cloud-tasks-mock-client/src/mock.rs`) 는 `CloudBackend` 를 in-memory 로 구현한 더미다. 하드코딩된 task `T-1000`/`T-1001`/`T-1002` 와 짧은 fake unified diff 만 돌려준다.

활성화 조건 (`codex-rs/cloud-tasks/src/lib.rs:43-60`):

```rust
#[cfg(debug_assertions)]
let use_mock = matches!(
    std::env::var("CODEX_CLOUD_TASKS_MODE").ok().as_deref(),
    Some("mock") | Some("MOCK")
);
```

- `debug_assertions` 가 켜진 빌드 (= `cargo build` 디폴트 = debug profile) 에서만, 그리고
- `CODEX_CLOUD_TASKS_MODE=mock` 이 환경변수로 설정됐을 때만 점화된다.

따라서 release 빌드에서는 mock 으로 빠질 수 없다 — 폐쇄망 대체용으로는 **그대로는 못 쓴다**. 의도는 명백히 "TUI 개발자가 chatgpt.com 없이 UI 를 굴리기 위한 fixture" 이고 실제 lib.rs 의 단위 테스트도 `MockClient` 를 직접 인스턴스화해서 사용한다 (`lib.rs:2135-2337`).

폐쇄망 데모 용도로 정말 살리고 싶다면 두 가지 길이 있다:

1. `#[cfg(debug_assertions)]` 을 떼고 `CODEX_CLOUD_TASKS_MODE=mock` 만으로 release 에서도 켜지게 한다 — 한 줄 수정. 단, 표시되는 task 가 모두 가짜라는 사실을 사용자에게 알릴 UX 가 필요.
2. `MockClient` 를 베이스로 사내 stub 백엔드를 작성. trait surface 가 8개 메서드뿐이라 비용은 크지 않다.

대다수 운영 시나리오에선 위 §6 의 "subcommand 자체를 hide / feature gate" 가 더 적절하다.

참고로 mock 은 단순 fixture 이상의 일은 안 한다 — 호출이 영속화되지 않고 (`apply_task` 가 그냥 `applied: true` 만 돌려준다, 실제 git apply 는 안 함), `create_task` 는 `task_local_{timestamp}` 라는 가짜 id 만 만든다. 즉 mock 으로 demo 를 보여줘도 사용자가 받는 diff 는 hardcoded 3 줄짜리이고 실제 워크트리 변경도 없다는 걸 인지해야 한다.

## 8. 핵심 파일 빠른 참조

- 진입점: `codex-rs/cli/src/main.rs:159-161, 1039-1051`
- CLI 정의: `codex-rs/cloud-tasks/src/cli.rs`
- Backend bootstrap: `codex-rs/cloud-tasks/src/lib.rs:43-107` (`init_backend`)
- TUI 메인 루프: `codex-rs/cloud-tasks/src/app.rs`, `ui.rs`
- Trait + 도메인 타입: `codex-rs/cloud-tasks-client/src/api.rs`
- HTTP 구현: `codex-rs/cloud-tasks-client/src/http.rs`
- Backend HTTP path style: `codex-rs/backend-client/src/client.rs:99-160, 319-420`
- Mock 구현: `codex-rs/cloud-tasks-mock-client/src/mock.rs`
- 환경 자동탐지: `codex-rs/cloud-tasks/src/env_detect.rs`
- Audit 항목: `fork-docs/airgap-audit-2026-05-08.md` §2.6
