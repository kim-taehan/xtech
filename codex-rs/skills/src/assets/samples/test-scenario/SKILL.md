---
name: test-scenario
description: Generate UI-to-backend test scenarios for a given screen file (React/Vue/Svelte + FastAPI/Spring/Express/etc.). Use when the user asks for a test scenario document for a specific screen.
metadata:
  short-description: Test scenario doc from a UI screen file
---

# test-scenario

UI 진입 화면 1개 → 인터랙션·검증·API 호출 식별 → 백엔드 핸들러 매칭 → 양쪽 케이스를 포괄한 시나리오 마크다운 1개 생성.

**산출물 1개, 코드 변경 0개.**

## 입력 파싱

사용자 메시지에서 진입점을 식별:
- `@경로/Login.tsx` (절대/상대 경로)
- `LoginPage` 같은 컴포넌트 이름 → `glob`/`grep`으로 정의 위치 검색
- 여러 파일 → 첫 파일이 진입점, 나머지는 의존 컴포넌트

## 0단계 — 프로젝트 컨텍스트 자동 탐지

**검색 루트** = cwd부터 위로 walk 하며 다음 마커 중 하나라도 있는 가장 가까운 조상:
- `.git`, `package.json`, `pyproject.toml`, `pom.xml`, `build.gradle`, `go.mod`, `Cargo.toml`, `Gemfile`

이게 워크스페이스 루트. 이하 모든 grep은 이 루트 기준.

**프론트엔드 스택**: 진입 파일 확장자/내용으로 결정
- `.tsx`/`.jsx` → React, `.vue` → Vue, `.svelte` → Svelte, `.html` → 정적/SSR

**백엔드 디렉토리 자동 탐지**: 검색 루트 아래에서 다음 마커가 있는 디렉토리 후보 수집 (최대 깊이 4):

| 마커 파일/디렉토리 | 스택 | grep 패턴 |
|---|---|---|
| `pyproject.toml` 또는 `requirements.txt` 에 `fastapi` | FastAPI | `@(router\|app)\.(post\|get\|put\|patch\|delete)\(`, `APIRouter\(prefix=` |
| `requirements.txt` 에 `flask` 또는 `app = Flask` | Flask | `@(app\|bp)\.route\(`, `@(app\|bp)\.(get\|post\|put\|delete)\(` |
| `manage.py` + `urls.py` | Django | `path\(`, `re_path\(`, `include\(` (urls.py 안) |
| `pom.xml` 또는 `build.gradle` | Spring Boot | `@(Get\|Post\|Put\|Patch\|Delete)Mapping`, `@RequestMapping`, `@RestController` |
| `package.json` 에 `express` | Express | `\.(post\|get\|put\|patch\|delete)\([\"']/`, `Router\(\)` |
| `package.json` 에 `@nestjs/common` | NestJS | `@(Post\|Get\|Put\|Patch\|Delete)\(`, `@Controller\(` |
| `package.json` 에 `hono` | Hono | `\.(post\|get\|put\|patch\|delete)\(` |
| `go.mod` | Go (gin/chi/echo) | `\.(POST\|GET\|PUT\|PATCH\|DELETE)\(`, `HandleFunc\(` |
| `Gemfile` 에 `rails` | Rails | `routes.rb` 안의 `resources`, `get`, `post` |

발견되지 않은 스택은 시도 자체를 생략. 후보가 0개면 시나리오 문서에 "백엔드 핸들러 미발견 — UI-only 시나리오"로 명시하고 진행.

## 1단계 — UI 인벤토리

진입 파일과 **직접 import하는 파일만** 읽는다 (2단계 깊이는 모호한 경우만).

추출:
- **입력 요소**: form, `<input>` (type/name/required/maxLength/min/max/pattern), select, textarea, checkbox, radio
- **액션 트리거**: button (onClick/type=submit), 링크, 키 핸들러
- **클라이언트 검증**:
  - React: zod/yup/joi schema, react-hook-form `rules`, 수동 if 분기
  - Vue: vee-validate, 수동 검증
  - Angular: `Validators.*`
- **상태/스토어**: useState/useReducer (React), pinia/vuex (Vue), redux/zustand/jotai, signals (Svelte/Solid)
- **라우팅/네비게이션**: `navigate(...)`, `<Link to=...>`, `router.push`, `redirect`
- **API 호출** (각 호출에서 method, URL, 페이로드, 성공/실패 분기 추출):
  - fetch / axios / ky / undici / SWR / React Query (`useQuery`, `useMutation`)
  - URL이 변수 치환이면 (`${API_BASE}/auth/login`) 정의를 따라가서 실제 prefix 확인
  - 인증 헤더 처리 방식 (token interceptor 등)

## 2단계 — 백엔드 핸들러 매칭

식별한 각 URL 경로에 대해 0단계에서 발견한 백엔드 디렉토리만 grep.

매칭 시 추가로 확인:
- **프레임워크별 prefix 결합**:
  - FastAPI: `APIRouter(prefix="...")` + 라우트 path
  - Spring: 클래스 레벨 `@RequestMapping("...")` + 메서드 레벨
  - Express: `app.use("/api/v1", router)` + 라우터 내 path
  - NestJS: `@Controller("...")` + 메서드 데코레이터 path
- **요청 스키마**:
  - Pydantic `BaseModel` (FastAPI), Java DTO/Record + `@RequestBody`, NestJS DTO + `class-validator`, Zod schema (Express)
  - 필드별 타입, 필수/옵셔널, 제약 (min/max/regex/range/email 등)
- **인증/권한**:
  - FastAPI: `Depends(get_current_user)`, OAuth2 scheme
  - Spring: `@PreAuthorize`, security config, JWT filter
  - Express/Nest: middleware, `@UseGuards`
- **응답**: 상태 코드별 스키마 (성공 2xx, 명시적 raise/throw → 4xx/5xx)
- **외부 의존성 호출**: DB/타 API/캐시/큐 → 5xx 시나리오 단서

핸들러 미발견은 추측 금지, "**핸들러 미발견**"으로 표기.

## 3단계 — 시나리오 도출

해당 사항만 채운다 (없으면 섹션 자체 생략):

| 카테고리 | 도출 기준 |
|---|---|
| Happy Path | 모든 필드 valid → 정상 응답 → 다음 화면/상태 전이 |
| 클라이언트 검증 | 각 필드 미입력/형식 오류/길이 초과 (UI에서 차단됨, 서버 호출 없음) |
| 서버 검증 실패 (4xx) | 클라 검증 통과하지만 서버 raise/throw에 도달 (중복, 잘못된 자격증명 등) |
| 인증/권한 (401/403) | 토큰 누락·만료, 권한 부족 |
| 5xx / 네트워크 | 서버 에러, 타임아웃, 끊김 → UI 에러 핸들러 동작 |
| 경계값 | 길이 경계, 공백 trim, unicode/이모지, 특수문자 |
| 상태 전이 | 성공 시 라우팅, 실패 시 머무름, 토스트/다이얼로그 |
| 멱등성/재시도 | 같은 요청 두 번 → 중복 제출 방지, 서버 중복 처리 |

각 케이스 필수 필드:
- **ID**: `<SLUG>-<CATEGORY>-<NNN>` (예: `LOGIN-PAGE-VALID-001`)
- **사전 조건**: 로그인 상태, 시드 데이터
- **입력값**: 구체값
- **기대 UI**: 버튼 disabled, 에러 메시지 정확한 텍스트, 라우팅
- **기대 API 호출**: 메서드/경로/페이로드 또는 "호출 없음"
- **기대 응답**: 상태 코드, 본문 핵심 필드
- **부수효과**: DB 변경, 로그, 외부 호출

## 산출물 위치 결정

순서대로 시도해서 첫번째로 가능한 곳 사용:
1. **`docs/scenarios/`** 가 이미 있으면 → 그곳
2. **`docs/`** 가 있으면 → `docs/scenarios/` 디렉토리 새로 생성
3. **`tests/scenarios/`** 가 이미 있으면 → 그곳
4. 위 모두 없으면 → 검색 루트에 `docs/scenarios/` 새로 만듦

파일명: `<slug>.md`. slug 변환 규칙 (결정론적):
- 확장자 제거: `.tsx`, `.jsx`, `.vue`, `.svelte`, `.html`, `.ts`, `.js`
- `.page` / `.screen` / `.view` / `.component` 접미사 제거
- PascalCase → kebab-case (`LoginPage` → `login-page`)
- camelCase → kebab-case (`loginPage` → `login-page`)
- 공백/언더스코어 → 하이픈

같은 slug 파일이 이미 있으면 **덮어쓴다** (백업 안 만듦). 다른 파일은 일절 건드리지 않음.

## 출력 템플릿

```markdown
# <화면 이름> 테스트 시나리오

## 메타
- 진입 파일: `<루트 기준 상대경로>`
- 분석 일시: <YYYY-MM-DD>
- 프론트엔드 스택: <React 19 / Vue 3 / ...>
- 탐지된 백엔드: <FastAPI @ ./api, Spring @ ./server>
- 의존 컴포넌트: …
- 호출 API:
  - `POST /auth/login` → `<백엔드경로>/auth_router.py:42` (`AuthRouter.login`)
  - `GET /me` → ...

## UI 인벤토리
(1단계 결과 — 폼 필드, 검증, API 호출 목록을 간결한 표/리스트로)

## 백엔드 핸들러 요약
### POST /auth/login
- 파일: `api/routes/auth_router.py:42`
- 요청: `LoginRequest { email: EmailStr, password: str(min_length=8) }`
- 응답: 200 `TokenResponse`, 401 `{detail: "invalid credentials"}`, 422 validation
- 인증: 불필요
- raise 지점:
  - L:55 — 사용자 미존재 → 401
  - L:60 — 비밀번호 불일치 → 401

(API별 반복)

## 시나리오

### 1. Happy Path
| ID | 시나리오 | 입력 | 기대 결과 |
|---|---|---|---|
| LOGIN-PAGE-VALID-001 | 정상 로그인 | email=`a@b.com`, password=`Pa$$w0rd` | POST /auth/login → 200 → /dashboard |

### 2. 클라이언트 검증
…

### 3. 서버 검증 실패 (4xx)
…

(나머지 카테고리)

## 커버되지 않은 영역 / 가정
- 분석 중 확신할 수 없었던 부분
- 핸들러를 못 찾은 API
- 추가 정보 필요 항목
- (있으면) 워크스페이스 외부 디렉토리에 있어 접근 못 한 백엔드
```

## 진행 규칙

- **클라이언트 검증과 서버 검증 명확히 분리**. 같은 규칙이 양쪽에 있으면 서버 케이스로 두되 UI에서도 차단됨을 표기.
- **파일 경로 + 라인 번호** 로 근거 표기 (`auth_router.py:42`).
- 모노레포(Module Federation, Lerna, Turborepo, Nx 등)면 진입 컴포넌트가 다른 패키지에 있을 수 있음. import는 1단계만 추적, 그 이상은 모호한 경우만.
- 사용자에게 중간 질문 금지. 추정/가정은 모두 "커버되지 않은 영역" 섹션에 명시.
- 코드 수정 금지. **`<output_dir>/<slug>.md` 한 개만 작성/덮어쓰기**.
- 0단계에서 백엔드를 못 찾아도 UI-only 시나리오로 진행 (미발견을 메타에 기록).

## 종료 출력

성공 시 마지막 한 줄만:
```
Wrote: <output_dir>/<slug>.md
```

실패 시 (진입 파일을 못 찾는 등) 사유 1~2줄로 간단히.
