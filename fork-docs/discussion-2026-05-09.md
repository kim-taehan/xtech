# 작업 / 논의 로그 — 2026-05-09

이 파일은 2026-05-09 부터의 작업/대화 기록을 시간 순으로 누적한다. 코드 변경 자체는 git commit 으로 기록되고, 여기엔 **결정 / 거부된 옵션 / 컨텍스트 / 다음 액션** 만 남긴다.

---

## 흐름 한 줄 요약

`OpenAI Codex` 브랜딩을 `xTech code` 로 갈아끼우고, 코드베이스를 처음 fork 받은 입장에서 한 번에 머릿속에 넣을 수 있도록 `fork-docs/arch/` 아래 19개 슬라이스 + README (TOC) 로 정리.

---

## 진행 사항

### 1. 브랜딩: OpenAI Codex → xTech code
- 5곳 갈아끼움 — TUI 메인 배너, status 카드, onboarding welcome, exec 헤드리스 배너, 그리고 history_cell 의 raw_lines.
- **알려진 후속 작업**: TUI insta snapshot baseline 미동기화. 다음 빌드 후 `cargo insta accept -p codex-tui` 로 갱신 필요. (참조: arch/13-tui-structure.md §9)

### 2. 코드베이스 분석 — 19 슬라이스 작성
처음 fork 받은 입장에서 코드베이스 통째 머리에 넣기엔 너무 크다는 사용자 의견을 반영해, 8 그룹 × 19 슬라이스로 분할 분석. 모두 백그라운드 에이전트 병렬 작성.

| 그룹 | 슬라이스 | 파일 |
|---|---|---|
| 코어 흐름 | turn lifecycle / wire protocol / streaming / error handling | 02, 03, 04, 05 |
| 저장 / 상태 | thread-manager / models-manager | 07, 08 |
| 능력 | tools / skills / plugins / mcp | 09, 10, 11, 12 |
| UI | tui-structure / tui-event-loop | 13, 14 |
| 운영 | config / build-deploy | 06, 15 |
| 보안 | sandboxing / approval-guardian | 16, 17 |
| 통합 | app-server / cloud-tasks | 18, 19 |
| 전체 | overview | 01 |

전체 ~4,500 LoC, 평균 ~230 LoC 각. README (00-인덱스) 가 TOC + cross-link + 권장 읽기 순서 (01 → 02 → 03 → 06 → 09).

### 3. 거부된 옵션 — orphan 단일 commit
upstream openai/codex 의 contributors 가 GitHub Contributors 에 그대로 표시되는 문제 발견. 해결책으로 **orphan branch + 단일 commit + force-push** 를 제시했으나 사용자 결정: 현재는 그대로 (open-source 라 라이선스상 무관).
대안 B (모든 커밋의 author 를 본인으로 rewrite) 도 제시했으나 마찬가지로 보류.

### 4. 토큰 / 사내 IP scrub
`fork-docs/ollama-migration.md` 등에 박혀있던 `sk-davis-27b-22222` 와 사내 IP `10.250.121.100` 를 placeholder 로 교체 후 push. 토큰은 사용자 확인 결과 이미 비활성. history 에는 남아있으나 latest 에선 안 보임. (orphan rewrite 보류로 history-level scrub 은 안 함)

### 5. 배포 채널 — kim-taehan/xtech (PUBLIC)
`v0.1.0` release 자산 `xtech-universal.tar.gz` (184MB, arm64 + x86_64) 업로드 완료.
설치 한 줄: `curl -fsSL https://raw.githubusercontent.com/kim-taehan/xtech/main/dist/install.sh | bash`

---

## 메모 / 결정 / 보류

- **xtech.json 다중 모델 + per-model 게이트웨이 override** — 사용자가 `~/.config/xtech/xtech.json` 의 `models` 배열로 여러 모델을 등록하고 각 모델별 `baseURL`/`apiKey` override 를 줄 수 있게 확장 (사용자 작업, 별도 commit).
- **TUI snapshot 동기화** — 브랜딩 변경 후 미적용. 빌드 후 처리 필요.
- **폐쇄망 P0 3건** — 배포 직전에 처리 (analytics_enabled=false / Feature::Plugins=false / chatgpt-base-url 가드).
- **NUDGE_MODEL_SLUG dead path** — `tui/src/chatwidget.rs:459` 의 `gpt-5.4-mini` 가 카탈로그에 없으니 silent no-op. 추후 정리.
- **Cloud Tasks** — fork 디폴트 인증으로는 fail-fast 라 폐쇄망 자연 차단. 정식 배포 전 `#[clap(hide = true)]` 권장.

## 다음 액션 후보

1. xtech.json 다중 모델 기능 commit + 사용 가이드 (INSTALL.md / arch 보강)
2. TUI snapshot 동기화 (`cargo insta accept -p codex-tui`)
3. 폐쇄망 P0 패치 (배포 일정 정해지면)
4. arch 문서를 보면서 본인이 손볼 영역 결정 → 해당 슬라이스 보강 또는 새 슬라이스 추가
