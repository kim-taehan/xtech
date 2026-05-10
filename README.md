# xtech

사내 게이트웨이를 통해 Qwen 으로 동작하는 코딩 어시스턴트. [`openai/codex`](https://github.com/openai/codex) fork 입니다.

```bash
curl -fsSL https://raw.githubusercontent.com/kim-taehan/xtech/main/dist/install.sh | bash
```

설치 옵션 (.pkg / 버전 핀 / 사내 호스팅 tarball) 과 제거·문제해결은 [`dist/INSTALL.md`](./dist/INSTALL.md) 참고.

---

## Quickstart

### 1. 설치

위 한 줄 스크립트가:

1. arch 감지 (Apple Silicon / Intel)
2. GitHub release 에서 `xtech-<arch>.tar.gz` 다운로드
3. `/usr/local/bin/xtech` 에 설치 (sudo 비밀번호 필요)
4. 설정 템플릿 `~/.config/xtech/xtech.json` 생성 (없을 때만)

### 2. 게이트웨이 설정

설치 직후 `~/.config/xtech/xtech.json` 은 빈 템플릿입니다. 세 필드를 모두 채워야 동작합니다.

```bash
$EDITOR ~/.config/xtech/xtech.json
```

```jsonc
{
  "baseURL": "http://<게이트웨이 호스트>/v1",            // 게이트웨이 root URL
  "apiKey":  "XXXXXXXXXXXXXXXXXXXX",   // 본인이 발급받은 토큰
  "model":   "qwen3.5"                          // 키 권한과 일치하는 모델 슬러그
}
```


### 3. 실행

```bash
xtech                    # TUI
xtech exec "say pong"    # 헤드리스
```

`pong` 이 돌아오면 정상.

---

## Docs

- [`dist/INSTALL.md`](./dist/INSTALL.md) — 설치 / 제거 / 문제해결 / 데이터 저장 위치
- [`fork-docs/`](./fork-docs/README.md) — 이 fork 한정 결정·작업 기록 (게이트웨이 전환, 멀티모델, 에어갭 감사 등)
- [`docs/`](./docs/) — upstream 공통 사용자 문서 (config, sandbox, exec, slash commands, agents)
- [`AGENTS.md`](./AGENTS.md) / [`CLAUDE.md`](./CLAUDE.md) — 컨트리뷰터 규칙

## 도움 요청

설치 / 실행 / 게이트웨이 이슈는 김태한 (kimtaehan11@gmail.com).

이 저장소는 [Apache-2.0 License](LICENSE) 를 따릅니다.
