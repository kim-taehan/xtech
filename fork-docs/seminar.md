# 사내 코딩 어시스턴트 세미나 자료

## 목차

1. [코딩 어시스턴트 CLI 비교](#1-코딩-어시스턴트-cli-비교)
2. [이 프로젝트의 목적](#2-이-프로젝트의-목적)
3. [설치와 사용](#3-설치와-사용)
4. [추가 자료](#4-추가-자료)
5. [알려진 한계 / 후속 작업](#5-알려진-한계--후속-작업)
6. [계획 — 사내 빌트인 Skill](#6-계획--사내-빌트인-skill)

---

## 1. 코딩 어시스턴트 CLI 비교

### 1.1 비교표 — 코더 관점

| 항목 | Claude Code | Codex (OpenAI) | opencode |
|---|---|---|---|
| **개발사** | Anthropic | OpenAI | sst (오픈소스 커뮤니티) |
| **비용** | Claude Pro/Max 구독 또는 Anthropic API 종량제 | ChatGPT Plus/Pro/Business 또는 OpenAI API 종량제 | **무료** (사용자가 자기 모델 키 BYO) |
| **오픈소스** | ❌ 클로즈드 | ✅ Apache-2.0 [openai/codex](https://github.com/openai/codex) | ✅ MIT [sst/opencode](https://github.com/sst/opencode) |
| **지원 모델** | Claude 4.x 만 (Opus 4.7 / Sonnet 4.6 / Haiku 4.5) | OpenAI gpt-5.x + 제한적 OSS provider | provider-agnostic (OpenAI / Anthropic / Google / Ollama / Bedrock 등) |
| **자체 호스팅** | 불가 | 가능 (wire API 제약 — 이 fork 가 그 작업) | 설계상 가능 — baseURL 만 꽂으면 됨 |
| **구현 언어** | TypeScript / Node.js | **Rust** + TS wrapper | TypeScript (Bun runtime, ai-sdk) |
| **배포** | npm | npm / brew / GitHub release / DotSlash | curl / brew / npm |
| **샌드박스** | hook 기반 | OS 레벨 (Seatbelt / Landlock / bwrap / Windows sandbox) | hook 기반 |
| **MCP 지원** | client + server | client + server | client |
| **TUI / IDE** | TUI + VS Code / JetBrains | TUI + IDE 확장 + 데스크탑 | TUI 중심 |
| **세션 저장** | 로컬 (claude home) | 로컬 JSONL (`~/.codex/sessions/`) | 로컬 (`~/.local/share/opencode/`) |

### 1.2 코더 관점에서의 결론

- **모델 자유도**: opencode 압도적. 어떤 모델이든 baseURL+key 만 있으면 작동.
- **오픈소스 + 폐쇄망 친화도**: codex 와 opencode 둘 다 open. codex 는 Rust 단일 바이너리라 배포가 가볍고, opencode 는 ai-sdk 추상화로 provider 갈아끼우기가 자연스러움.
- **사내 폐쇄망 시나리오**: codex fork (chat-completions wire 복원 + 게이트웨이 라우팅) 가 현실적. opencode 도 후보지만 Bun 런타임 의존이라 배포 무게가 다름.
- **모델 품질**: Claude 가 코딩 평가에서 강한 평이 많지만, 사내 정책상 외부 API 못 쓰면 의미 없음.
- **OpenAI / Anthropic 외 모델 (Qwen, DeepSeek 등) 활용**: opencode 가 가장 즉시 가능, codex 는 fork 필요 (= 이 프로젝트), Claude Code 는 불가.

---

## 2. 이 프로젝트의 목적

> **"사내 코더가, 사내 LLM 게이트웨이로, codex 와 동급의 코딩 어시스턴트를 받아쓰게 한다."**

### 2.1 풀고 싶은 문제

사내에는 이미:
- 검증된 코딩 어시스턴트 형태 (codex / claude code / opencode 같은 TUI + agent 흐름) 가 있고
- 내부 GPU/추론 인프라에 호스팅된 자체 LLM 게이트웨이 (`Qwen 3.5-122b` 등) 가 있다.

그러나 두 가지를 그대로 묶을 수 없는 제약:
- **폐쇄망**: 외부 OpenAI / ChatGPT / Anthropic API 호출 불가
- **wire 호환성**: 사내 게이트웨이는 OpenAI 호환 `/v1/chat/completions` 만 지원 — `/v1/responses` 미노출
- **인증**: 사내 발급 Bearer 토큰 (모델별 권한 분리) 만 통과

→ 검증된 어시스턴트 한 개를 **fork 해서 사내 인프라에 맞게 라우팅 + 인증 + wire 변환**을 끼워넣는 것이 가장 현실적인 경로.

### 2.2 실제 사용 시나리오 — 누가, 왜 쓰는가

폐쇄망 / wire 호환성 같은 기술적 동기 외에, 사내 도입을 정당화하는 **현실적 시나리오 2가지**:

#### A. 정직원의 외부 어시스턴트 quota 보완

- 회사가 일부 정직원에게 Claude / Codex / Cursor 같은 외부 어시스턴트 구독을 제공.
- 그러나 월 token / message quota 가 빠르게 소진 (특히 코딩 헤비 사용자).
- quota 소진 후 → **xtech (사내 게이트웨이) 로 자연스럽게 전환** 해서 작업 지속.
- 외부 구독은 그대로 유지하되 **초과분만 사내로 흡수** → 외부 구독 비용 폭증 방지.

#### B. 외부직원 / 미구독 직원의 비용 절감

- 외부 직원 (협력사 / 계약직), 또는 정직원 중 외부 구독 미할당자.
- 현재 외부 어시스턴트 비용 = 0 → **코딩 어시스턴트 자체를 못 씀**.
- xtech = 사내 인프라 자원만 사용 → **추가 라이선스 비용 0 으로 어시스턴트 보급**.
- 효과: 사내 코딩 어시스턴트 보급률 상승 + 코더 생산성 균등화.

| 시나리오 | 외부 어시스턴트 사용 | xtech 도입 후 |
|---|---|---|
| A. 정직원 quota 초과 | quota 다 쓰고 나면 멈춤 | 끊김 없이 사내 게이트웨이로 전환 |
| B. 외부 어시스턴트 미보급 직원 | 어시스턴트 사용 불가 | 사내 인프라만으로 동급 UX 사용 |

### 2.3 codex 를 base 로 고른 이유

§1 의 비교를 결정 기준으로 압축:

| 후보 | 채택 | 이유 |
|---|---|---|
| Claude Code | ❌ | 클로즈드. Anthropic API 의존 강제. fork / 게이트웨이 라우팅 불가. |
| opencode | △ | 가능하지만 Bun + ai-sdk 의존. 폐쇄망 단일 바이너리 배포 무게가 큼. |
| **codex** | ✅ | Rust 단일 바이너리. Apache-2.0. wire / 인증 / 모델 모두 fork 로 손댈 수 있는 지점이 명확. 모델 품질 의존성 낮음. |

### 2.4 fork 가 한 일 — 핵심 5건

상세는 [`work-log-2026-05-08.md`](work-log-2026-05-08.md).

1. **Chat Completions wire 복원** — upstream 이 한차례 제거한 `WireApi::Chat` 분기를 다시 살림. 사내 게이트웨이가 `/v1/responses` 미지원이라 필수.
2. **디폴트 라우팅 변경** — `model_provider` 디폴트를 `openai` → `ollama` 로. `--oss` 플래그 / config.toml 없이 `xtech` 한 단어로 동작.
3. **`developer → user` role 매핑** — codex 내부의 developer-role 메시지를 chat completions 가 받는 user-role 로 매핑.
4. **외부 JSON config (`~/.config/xtech/xtech.json`)** — opencode 식 baseURL/apiKey/model 주입. 사내 인프라 정보가 코드/git 에 박히지 않게.
5. **모델 카탈로그 정리 + 브랜딩** — `models.json` 에서 GPT 항목 제거, 단일 `qwen3.5-122b` + 숨겨진 guardian 모델만. UI 의 "OpenAI Codex" → "xTech code".

### 2.5 결과 — 사내 코더가 보는 변화

```
사내 코더 PC                   회사 네트워크                       추론 서버
┌─────────────┐  /v1/chat/    ┌─────────────────┐    Qwen 3.5    ┌────────┐
│   xtech     │ ─completions─▶│   사내 게이트웨이  │ ──────────────▶│ Qwen   │
│   (CLI)     │ ◀─────────────│   (인증 / 라우팅) │ ◀──────────────│ 122b   │
└─────────────┘    SSE        └─────────────────┘                 └────────┘
       ▲
       │  ~/.config/xtech/xtech.json (baseURL / apiKey / model)
```

코더 입장에서:
- 한 줄 설치 (`curl ... | bash`).
- JSON 한 번 채우면 그 다음부터는 `xtech` 만 치면 끝.
- 외부 OpenAI / ChatGPT / Anthropic 가입 없이, 사내 인프라만으로 codex 와 동급 UX (TUI / approvals / sandbox / MCP / skills / plugins) 사용.

---

## 3. 설치와 사용

전체 가이드는 [`../dist/INSTALL.md`](../dist/INSTALL.md). 세미나용 압축판.

### 3.1 한 줄 설치 (macOS, Apple Silicon / Intel 모두)

```bash
curl -fsSL https://raw.githubusercontent.com/kim-taehan/xtech/main/dist/install.sh | bash
```

스크립트가 자동 수행:
1. 아키텍처 감지 (universal binary 라 둘 다 OK)
2. GitHub release 에서 `xtech-universal.tar.gz` 다운 (~184 MB)
3. `/usr/local/bin/xtech` 에 설치 (sudo 비밀번호 필요)
4. `~/.config/xtech/xtech.json` 빈 템플릿 생성 (이미 있으면 건드리지 않음)

### 3.2 설정 파일 작성 (한 번만)

```bash
$EDITOR ~/.config/xtech/xtech.json
```

```jsonc
{
  "baseURL": "http://<게이트웨이 호스트>/v1",
  "apiKey":  "sk-...",
  "model":   "qwen3.5-122b"
}
```

> 키마다 사용 가능한 모델이 다르다 (122b 키 → 122b 만, 27b 키 → 27b 만). `model` 슬러그가 키 권한과 안 맞으면 게이트웨이가 401 로 거부.

### 3.3 동작 확인

```bash
xtech exec "say pong"     # 헤드리스 한 턴 — pong 떨어지면 정상
xtech                     # TUI 인터랙티브 모드
```

### 3.4 그 외 자주 쓰는 흐름

| 시나리오 | 명령 / 위치 |
|---|---|
| 이전 세션 이어가기 | `xtech resume --last` 또는 `xtech resume <thread-id>` |
| 세션 기록 | `~/.xtech/sessions/YYYY/MM/DD/rollout-*.jsonl` |
| 제거 | `sudo rm /usr/local/bin/xtech` + 필요 시 `rm -rf ~/.xtech ~/.config/xtech` |
| 업데이트 | 설치 명령 한 번 더 (덮어씀, 기존 설정은 유지) |
| curl 못 쓰는 환경 | `xtech-<sha>-universal.pkg` 더블클릭 (Gatekeeper 우회 필요) |

### 3.5 트러블슈팅

| 증상 | 원인 / 해결 |
|---|---|
| `Invalid API key for model: qwen3.5-XXb` | 키 ↔ 모델 슬러그 불일치. `xtech.json` 의 `model` 을 키 권한에 맞게 수정. |
| `Connection refused` / `getaddrinfo failed` | 사내망 / VPN 미연결. 게이트웨이 IP 가 RFC1918 사설망이라 외부에선 안 보임. |
| TUI 시작 시 `failed to warm featured plugin ids cache` 401 | ChatGPT 백엔드 동기화 시도 — 동작에 영향 없음. 정식 폐쇄망 빌드에서 끌 예정. |
| `mach-o, 잘못된 아키텍처` | 거의 없음 (universal binary). 발생 시 release 자산 재다운로드. |

---

## 4. 추가 자료

### 4.1 wire API 비교 — `/v1/responses` vs `/v1/chat/completions`

이 fork 가 왜 Chat Completions 분기를 살려야 했는지 → [`responses-vs-chat.md`](responses-vs-chat.md)

요약:
- **`/v1/chat/completions`**: 사실상 표준. 거의 모든 OpenAI 호환 게이트웨이가 이걸 지원. **사내 게이트웨이도 이거만 됨.**
- **`/v1/responses`**: OpenAI 차세대 agent-친화 API. 표현력은 더 좋지만 OpenAI 본가 외엔 거의 미지원.
- **upstream codex 는 Responses 선호** → 이 fork 는 Chat 분기를 다시 살리고 role/tool 변환 끼워 넣어 사내 게이트웨이 호환 확보.

### 4.2 더 깊이 보고 싶다면

- [`arch/README.md`](arch/README.md) — 19 슬라이스로 분할한 코드베이스 분석 (코어 흐름 / 저장 / 도구 / TUI / 빌드 / 보안 / 통합)
- [`work-log-2026-05-08.md`](work-log-2026-05-08.md) — fork 가 손댄 코드의 자세한 변경 기록
- [`multi-turn-and-storage-2026-05-08.md`](multi-turn-and-storage-2026-05-08.md) — 대화 / 세션 / resume 이 디스크에 어떻게 떨어지는지
- [`airgap-audit-2026-05-08.md`](airgap-audit-2026-05-08.md) — 폐쇄망 외부 호출 점검과 차단 항목

---

## 5. 알려진 한계 / 후속 작업

| 항목 | 설명 | 상세 |
|---|---|---|
| **폐쇄망 phone-home P0 3건 미패치** | Statsig OTLP 메트릭 / curated plugin git+REST sync / featured plugin REST sync. 정식 폐쇄망 배포 전에 끄는 패치 필요. | [airgap-audit](airgap-audit-2026-05-08.md) |
| **모델 품질** | qwen3.5-122b 가 GPT-5 / Sonnet 4.6 대비 코딩 평가에서 떨어지는 영역 존재. 벤치마크보다 실제 사용 경험으로 검증 필요. | — |
| **외부 MCP 서버** | 사내 폐쇄망 가정 위반 가능. 사용 시 서버 단위로 검증 필요. | [arch/12-mcp.md](arch/12-mcp.md) |
| **TUI snapshot 미동기화** | 브랜딩 변경 (`OpenAI Codex` → `xTech code`) 후 `cargo test -p codex-tui` 가 fail. `cargo insta accept` 필요. | [arch/13-tui-structure.md](arch/13-tui-structure.md) |
| **Apple notarize 미적용** | .pkg 무서명 — 사용자가 `xattr -d com.apple.quarantine` 으로 Gatekeeper 우회. 정식 외부 배포엔 Developer ID 필요. | — |
| **Cloud Tasks** | OpenAI 호스팅 의존. 폐쇄망에선 fail-fast 이지만 정식 배포 전 `#[clap(hide)]` 권장. | [arch/19-cloud-tasks.md](arch/19-cloud-tasks.md) |

---

## 6. 계획 — 사내 빌트인 Skill

기본 LLM 채팅 외에, **사내 코더가 자주 하는 작업** 을 더 빠르게 끝낼 수 있게 **빌트인 skill 3종** 을 묶어서 출고할 계획. 사용자별 `~/.xtech/skills/` 에 두는 것 대신 바이너리에 같이 박아 일관된 사내 컨벤션 / 출력 포맷 / 프롬프트를 default 로 보장.

빌트인 skill 의 위치 / 등록 방식은 [arch/10-skills.md](arch/10-skills.md) 의 system scope 사용. 즉시 `xtech` 한 줄로 호출 가능하게 slash-command + 자연어 트리거 둘 다 지원.

### 6.1 테스트 시나리오 작성 — `@test-scenario @<화면명>`

화면 한 개를 입력으로 주면, 그 화면이 호출하는 백엔드 코드 / API 를 코드베이스에서 찾아 연결하고, **화면 + API 양쪽을 검증할 수 있는 테스트 케이스 + 문서**를 한 번에 산출.

**호출 형식**

```
@test-scenario @<화면명>
```

예: `@test-scenario @주문상세`

**동작 흐름**

```
1. 화면 식별         <화면명> 으로 UI 컴포넌트 / 라우트 / 페이지 찾기
        │
        ▼
2. 백엔드 추적       그 화면에서 호출되는 API endpoint / 함수 검색
        │           (fetch / axios / RPC / GraphQL 등)
        ▼
3. UI ↔ API 매핑     입력 필드 ↔ 요청 필드, 응답 ↔ 화면 표시
        │
        ▼
4. 테스트 케이스 생성  화면 단위 (E2E / Cypress / Playwright)
        │           + API 단위 (계약 / 통합 / 경계값)
        ▼
5. 문서화           시나리오 + 매핑표 + 테스트 코드 + 검증 포인트
```

**입출력 요약**

| 항목 | 내용 |
|---|---|
| 입력 | 화면명 (`@<화면명>`) — 사내 화면 카탈로그 또는 코드베이스에서 lookup |
| 출력 | (1) 화면-API 매핑 표, (2) E2E 테스트 코드, (3) API 테스트 코드, (4) 시나리오 / 검증 포인트 정리 문서 |
| 트리거 | `@test-scenario @<화면명>` — 자연어로도 호출 가능 (예: "주문상세 화면 테스트 시나리오") |
| 가치 | 화면 ↔ API ↔ 테스트 ↔ 문서를 따로 만들던 작업을 한 번에. 화면이 추가될 때마다 자동으로 동급 품질의 산출물 생산 |

**전제 조건**

- 화면명 → 컴포넌트 매핑 카탈로그 (사내 표준 디렉토리 구조 또는 별도 메타데이터). 없으면 코드 검색으로 best-effort.
- 사내 테스트 프레임워크 (Cypress / Playwright / Jest 등) 가 정해져 있어야 출력 코드가 즉시 사용 가능.

### 6.2 코드 분석 — `@code-analyze @<entry>`

코드 한 지점을 입력으로 받아 **의존 그래프 추적 → 복잡도 / 결합도 산출 → I/O 표면 (network / DB) 추출 → 주석 참조 → DB 적재 → 문서 + UI 표출** 까지 한 번에 도는 분석 파이프라인. 단순 LLM 답변이 아니라 **사내 코드베이스 위에 점진적으로 쌓이는 지식 자산**.

**호출 형식**

```
@code-analyze @<entry>
```

`<entry>` 는 파일 / 클래스 / 함수 / 컨트롤러 / 화면 컴포넌트 — frontend / backend 둘 다 entry 가능.

**프론트 vs 백엔드 — 분석 축이 다름**

| 축 | Frontend entry | Backend entry |
|---|---|---|
| 의존 추적 | import / 컴포넌트 트리 / 상태 관리 / API 클라이언트 호출 | 호출 그래프 / 리포지토리 / 외부 서비스 클라이언트 |
| I/O 표면 | API 호출 (fetch / axios / GraphQL), localStorage / IndexedDB | network out (HTTP / gRPC / Kafka), DB I/O (SELECT / INSERT / SP) |
| 주요 메트릭 | 컴포넌트 깊이 / props drilling / re-render 트리거 | 트랜잭션 경계 / DB 쿼리 수 / N+1 의심 |

**동작 흐름 (백엔드 가정)**

```
1. Entry 식별         <entry> 의 파일 / 함수 / 컨트롤러 결정
        │
        ▼
2. 의존 그래프 추적    static call graph 빌드 (직접 호출 + 간접 호출)
                     · 호출 그래프 깊이 N 까지
                     · 리포지토리 / 매퍼 / 외부 클라이언트
        │
        ▼
3. 복잡도 / 결합도     · cyclomatic / fan-in / fan-out / module coupling
                     · 호출 leaf 수 / 트랜잭션 경계
        │
        ▼
4. I/O 표면 발라내기   · network out: HTTP endpoint, gRPC service, Kafka topic
                     · DB I/O: SQL / SP / ORM 쿼리 추출
                     · 주석 참조: 메서드 / 필드 doc-comment 도 메타로 수집
        │
        ▼
5. (옵션) Git 증분     · 분석 대상 파일의 마지막 분석 시점 ↔ HEAD diff
                     · 변경 없는 leaf 는 스킵 (이전 결과 재사용)
                     · 변경된 leaf 는 재분석 + 그 leaf 를 참조하는 부모 노드도 invalidate
        │
        ▼
6. DB 적재             분석 결과를 사내 분석 DB 에 저장
                     · entry → 의존 노드 → 메트릭 → I/O 표면
                     · 분석 시점 / git SHA / 분석 대상 hash
        │
        ▼
7. 산출물              · 마크다운 문서 (사람용)
                       · 분석 DB 의 row (다른 도구 / skill / 외부 UI 가 재사용)

           ※ UI 화면은 이 skill 의 책임 밖.
              별도 web 프로그램 (또는 단순 Python 스크립트) 이
              **분석 DB 만 공유** 해서 표출.
```

**입출력 요약**

| 항목 | 내용 |
|---|---|
| 입력 | `<entry>` (파일 / 클래스 / 함수 / 화면 / 컨트롤러) |
| 출력 | (1) 의존 그래프, (2) 복잡도 / 결합도 메트릭, (3) network / DB I/O 목록, (4) 주석 메타, (5) 분석 DB 적재, (6) 마크다운 문서 |
| 트리거 | `@code-analyze @<entry>` |
| 가치 | 매번 LLM 한 번 호출이 아니라 **누적되는 지식 베이스**. 새 코더 온보딩 / 영향도 분석 / 리팩터 후보 발굴 / 보안 리뷰 모두 이 DB 가 기반 |
| UI / 시각화 | **이 skill 책임 밖.** 별도 web 프로그램 또는 Python 스크립트가 같은 분석 DB 를 읽어 표출 |

**Git 증분 (옵션)**

- 분석 결과를 DB 에 저장할 때 분석 대상 코드의 git SHA + 파일 hash 를 함께 기록.
- 다음 호출 시 git 으로 변경 여부 확인 → **변경 없으면 스킵, 결과 재사용**.
- 변경된 leaf 는 재분석 + **그 leaf 를 참조하는 부모 노드도 invalidate** (불완전 cache 방지).
- 효과: 큰 코드베이스에서 첫 분석은 무겁지만 그 이후엔 수 분 내 갱신.

**산출물 — 분석 DB 가 단일 진실원**

```
                ┌──────────────────────────────────┐
   xtech 의      │  분석 DB                          │
   @code-analyze ─▶│  (entry / nodes / metrics / I/O) │
   (skill)       └──────────────────────────────────┘
                        │              │
                        ▼              ▼
                 ┌────────────┐   ┌──────────────────┐
                 │  마크다운    │   │  외부 UI 프로그램  │
                 │  (skill 이   │   │  (별도 web /      │
                 │   동시 출력) │   │   Python — DB 만 │
                 │             │   │   공유)           │
                 └────────────┘   └──────────────────┘
```

skill 의 책임은 **분석 DB 까지**. UI 표출은 사내 표준 stack 으로 별도 web 프로그램 또는 단순 Python 스크립트가 만들고, **xtech 와 동일한 분석 DB 를 읽기 모드로 공유**해서 표출.

**의존하는 자원**

| 자원 | 용도 |
|---|---|
| 분석 DB MCP | 결과 적재 + 쿼리 (운영 DB 와 분리된 분석용 DB 권장) |
| Git MCP / 로컬 git | SHA / diff 로 증분 갱신 판단 |
| 코드베이스 파일 읽기 / grep (codex 빌트인) | 의존 그래프 빌드 |

**전제 조건**

- 분석용 DB 스키마 설계 — entry / 노드 / 엣지 / 메트릭 / I/O 테이블
- 사내 코딩 컨벤션 / 디렉토리 규칙 — entry 식별 정확도에 영향
- (선택) 외부 UI 프로그램 — skill 책임 밖, 별도 web 또는 Python 으로 같은 분석 DB 공유
- (선택) 정적 분석 도구 (tree-sitter / Semgrep / language-server) 보조 — LLM 만으론 큰 코드베이스의 의존 그래프 정확도 한계

**현실적 구현 단계**

1. **MVP**: entry 1개에 대해 LLM + 코드 grep 으로 의존 / I/O 추출. 결과를 마크다운으로만 출력. DB 저장 없음.
2. **DB 적재**: 결과 스키마 정의 + 사내 분석 DB 에 저장. 동일 entry 재호출 시 비교만.
3. **Git 증분**: SHA / hash 기반 캐싱.
4. **외부 UI 프로그램** (별도 트랙): web / Python 스크립트로 분석 DB 만 읽어 검색 / 그래프 시각화. xtech 와는 무관하게 진행 가능.
5. **다른 skill 과 연동**: `@test-gen` 이 분석 결과 DB 를 픽스처 소스로 사용, `@test-scenario` 가 화면-API 매핑을 여기서 가져오기 등.

### 6.3 테스트 케이스 자동 생성 — `@test-gen @<api>`

API 한 개를 입력으로 받아 **실제로 호출 가능한 실행 가능 테스트 케이스**를 생성. 단순히 코드만 보는 것이 아니라 **DB MCP / 메타데이터 / 호출 이력**까지 종합해 runtime 에서 통과하는 실제 페이로드를 만든다.

**호출 형식**

```
@test-gen @<api>
```

예: `@test-gen @POST /orders` 또는 `@test-gen @OrderController.createOrder`

**왜 단순 코드 분석으로 부족한가**

`POST /orders` 의 request body 스키마만 봐서는 *형식상 valid* 한 페이로드는 만들 수 있어도, 실제 호출 시:
- DB 에 없는 `customerId` 를 넣으면 FK 위반 → 500
- 만료된 `couponCode` 를 넣으면 비즈니스 로직에서 거부
- 동시성 / 멱등성 / 권한 검증을 거쳐야 진짜 통과

→ **실데이터 / 메타데이터 / 호출 이력까지 봐야** 실행 가능한 케이스가 나옴.

**동작 흐름**

```
1. API 식별         endpoint / 컨트롤러 / 라우트 메서드 lookup
        │
        ▼
2. 코드 수집         · 컨트롤러 / 핸들러
                    · 입력 검증 (validator / DTO / OpenAPI)
                    · 비즈니스 로직 호출 그래프
                    · 리포지토리 / DAO / SQL 쿼리
        │
        ▼
3. 메타데이터 수집    · DB 스키마 (DB MCP 로 조회)
                    · ENUM / 코드값 / 정책 테이블
                    · 권한 / 역할 매핑
        │
        ▼
4. 이력 수집         · 기존 통합 테스트 / 픽스처
                    · 로그 / 호출 이력 (가능 시 로그 MCP)
                    · 운영 환경의 valid 한 ID 샘플 (DB MCP 로 SELECT)
        │
        ▼
5. 페이로드 합성     실데이터 기반 valid 케이스 + 의도된 invalid 케이스
                    (FK 위반 / 권한 부족 / 멱등성 / 동시성 / 경계값)
        │
        ▼
6. 테스트 코드 생성  사내 테스트 프레임워크 컨벤션으로 변환
                    + 픽스처 / 셋업 / 롤백 코드 포함
```

**입출력 요약**

| 항목 | 내용 |
|---|---|
| 입력 | API endpoint (`@POST /orders`) 또는 컨트롤러 메서드 |
| 출력 | (1) valid 케이스 (실데이터 기반 happy-path), (2) invalid 케이스 (FK / 권한 / 비즈니스 룰 / 경계), (3) 셋업/픽스처 코드, (4) 검증 포인트 정리 |
| 트리거 | `@test-gen @<api>` — 자연어로도 호출 가능 |
| 가치 | "코드는 됐는데 실제 데이터로 안 돌아가는 테스트" 문제 해소. 신규 API 추가 시 이력 / 데이터 / 권한 검증을 LLM 이 일괄 정리해줌. |

**의존하는 MCP / 외부 자원**

이 skill 은 정보를 모으기 위해 다음 MCP / 도구를 호출. 사내 폐쇄망 가정하에 사내 MCP server 로 등록되어야 함.

| 자원 | 용도 |
|---|---|
| DB MCP | 스키마 조회 + valid 한 ID/상태 SELECT (테스트 픽스처용) |
| 로그 / 트레이스 MCP (있으면) | 운영 환경의 실제 호출 이력 참고 |
| 코드베이스 grep / 파일 읽기 (codex 빌트인) | 컨트롤러 / 검증기 / SQL 추적 |

**전제 조건**

- 사내 DB MCP 서버 등록 (read-only 권한 권장 — 운영 DB 노출 시 PII / 민감정보 마스킹 필요)
- 사내 테스트 프레임워크 (스프링 `@SpringBootTest` / pytest+httpx / RSpec request spec 등) 컨벤션이 정해져 있어야 즉시 사용 가능
- (선택) 호출 이력 / 트레이스 MCP — 없으면 코드 + DB 만으로 best-effort

**보안 / 운영 주의**

- DB MCP 가 운영 DB 를 가리키면 PII / 사내 비밀이 LLM 컨텍스트로 흘러감. 마스킹 / 데모 DB / 별도 테스트 DB 사용 권장.
- 생성된 테스트가 운영 환경에 직접 실행되지 않게 — destination 명시 / dry-run 가드 필요.

### 6.4 빌트인 vs 사용자 skill — 왜 빌트인인가

- **즉시 사용성**: 설치만 하면 끝 — 추가 setup 불필요
- **사내 컨벤션 강제**: 프롬프트 / 출력 포맷 / 어설션 스타일 등을 fork 가 통제 → 결과 품질 일관성
- **버전 동기화**: 바이너리 업데이트 = skill 도 같이 갱신
- **격리**: 사용자가 `~/.xtech/skills/` 에 같은 이름으로 만들면 사용자 것 우선 (override 가능, [10-skills.md](arch/10-skills.md) 의 scope rank 참고)

### 6.5 작업 단계

1. 각 skill 의 manifest (`interface.default_prompt`, `description`, `body`) 초안 작성 — 사내 사례 1-2개로 검증
2. `codex-rs/core-skills/` 의 system scope 에 bundled asset 으로 추가
3. TUI 에 slash-command 바인딩 (`/test-scenario`, `/analyze`, `/test-gen`)
4. fork-docs/arch/10-skills.md 에 빌트인 skill 섹션 추가
5. 사내 dogfooding 후 프롬프트 튜닝
