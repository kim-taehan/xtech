# xtech 설치 가이드

사내 게이트웨이를 통해 Qwen 3.5-122B 로 작동하는 코딩 어시스턴트입니다.

## 권장 — 한 줄 설치

터미널에서:

```bash
curl -fsSL https://raw.githubusercontent.com/kim-taehan/xtech/main/dist/install.sh | bash
```

스크립트가:
1. arch 감지 (Apple Silicon / Intel)
2. GitHub release 에서 `xtech-<arch>.tar.gz` 다운로드
3. `/usr/local/bin/xtech` 에 설치 (sudo 비밀번호 필요)
4. 설정 템플릿 `~/.config/xtech/xtech.json` 생성 (없을 때만)

## 게이트웨이 설정 입력

설치 직후 `~/.config/xtech/xtech.json` 은 빈 템플릿으로 깔립니다. 세 필드를 모두 채워야 동작합니다.

```bash
$EDITOR ~/.config/xtech/xtech.json
```

```jsonc
{
  "baseURL": "http://<게이트웨이 호스트>/v1",         // 사내 게이트웨이 root URL
  "apiKey":  "sk-davis-122b-XXXXXXXXXXXXXXXXXXXX",  // 본인이 발급받은 토큰
  "model":   "qwen3.5-122b"                          // 키 권한과 일치하는 모델 슬러그
}
```

> 키마다 사용 가능한 모델이 다릅니다 (122b 키는 122b 만, 27b 키는 27b 만). 공유 금지.

## 실행

```bash
xtech                    # TUI
xtech exec "say pong"    # 헤드리스
```

`pong` 이 돌아오면 정상.

---

## 옵션 — 설치 변형

```bash
# 특정 release 버전 핀
curl -fsSL <url>/install.sh | XTECH_VERSION=v0.1.0 bash

# 다른 디렉토리에 설치 (sudo 권한 없을 때)
curl -fsSL <url>/install.sh | XTECH_INSTALL_DIR="$HOME/.local/bin" bash

# tarball URL 직접 지정 (GitHub 외부 호스팅용)
curl -fsSL <url>/install.sh | bash -s -- \
  --url https://files.internal/xtech-arm64.tar.gz
```

## 옵션 — .pkg installer (curl 못 쓸 때)

GitHub release 의 `xtech-<ver>-arm64.pkg` 를 받아 더블클릭 설치, 또는:

```bash
xattr -d com.apple.quarantine ~/Downloads/xtech-*.pkg
sudo installer -pkg ~/Downloads/xtech-*.pkg -target /
```

---

## 문제 해결

**`mach-o, 잘못된 아키텍처`**
Intel Mac 인데 arm64 빌드를 받은 경우. `XTECH_VERSION` 으로 `x86_64` 자산이 있는 release 를 핀하세요.

**`Invalid API key for model: qwen3.5-XXb`**
키와 `model` 필드 불일치. 키 발급자에게 물어 본인 키가 어떤 모델 권한을 가졌는지 확인 후 `model` 값을 맞추세요.

**`Connection refused` 또는 `getaddrinfo failed`**
회사 네트워크 (VPN/내부망) 에 연결돼 있는지 확인. 게이트웨이 `http://<gateway-host>` 은 사내망에서만 보입니다.

**TUI 시작 시 `failed to warm featured plugin ids cache` 같은 401 경고**
ChatGPT 백엔드 동기화 시도 — 동작에 영향 없음. 무시해도 됩니다 (정식 폐쇄망 빌드에서는 끌 예정).

## 제거

```bash
sudo rm /usr/local/bin/xtech
rm -rf ~/.xtech   # 설정/세션/메모리 모두 삭제
```

세션만 지우려면 `~/.xtech/sessions/` 만.

## 데이터 저장 위치

| 경로 | 내용 | 비고 |
|---|---|---|
| `~/.config/xtech/xtech.json` | 게이트웨이 설정 | 키 포함 |
| `~/.xtech/sessions/YYYY/MM/DD/rollout-*.jsonl` | 대화 기록 (스레드당 1) | append-only |
| `~/.xtech/state_5.sqlite` | 세션 인덱스 | 자동 재구성 |
| `~/.xtech/shell_snapshots/*` | 셸 환경 캐시 (3일 TTL) | env 포함 |

세션 이어가기: `xtech resume --last` 또는 `xtech resume <thread-id>`.

## 도움 요청

설치 / 실행 / 게이트웨이 이슈는 김태한 (kimtaehan11@gmail.com).
