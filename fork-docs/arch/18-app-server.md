# 18. App-server / IPC layer

이 문서는 xtech 의 **App-server** — 즉 IDE / desktop 클라이언트와 codex 백엔드 사이를 잇는 JSON-RPC 게이트웨이 — 를 정리한다. 사용자가 직접 보는 것은 보통 TUI 지만, 실제로 turn 을 실행하고 thread 상태를 관리하는 주체는 별도의 `app-server` 프로세스 (혹은 in-process 인스턴스) 이고 TUI 는 그 위의 한 클라이언트일 뿐이다. v1/v2 프로토콜 분리, 네이밍 컨벤션, transport 스택, schema 자동 생성 흐름까지 한 번에 파악하는 것이 목표다.

상세 규약은 `codex-rs/app-server/README.md`, `CLAUDE.md` 의 app-server v2 API conventions 섹션, 그리고 `codex-rs/app-server-protocol/src/protocol/common.rs` 의 매크로 테이블에 있다. 이 문서는 fork 관점에서 모듈 위치와 운용 흐름을 정리한다.

## 1. App-server 가 무엇인가

- App-server 는 codex 의 **백엔드 데몬** 이다. JSON-RPC 2.0 메시지를 양방향으로 주고받으며, 클라이언트가 보내는 `thread/start`, `turn/start`, `config/read` 같은 RPC 를 처리하고 모델 응답 / tool 호출 / 파일 변경 등을 notification 으로 흘려준다 (`codex-rs/app-server/README.md` Protocol 섹션).
- 진입점은 `codex-rs/app-server/src/main.rs` 의 `AppServerArgs` — `--listen <URL>` 로 transport 를 고르고, `--session-source` / `--ws-auth` 같은 인증·식별 플래그를 받는다.
- TUI 도 다른 클라이언트와 똑같이 RPC 로 붙는다. 차이는 **별도 프로세스가 아니라 in-process** 로 띄워서 채널로 직접 연결한다는 점이다 — `codex-rs/tui/src/lib.rs` 가 `codex_app_server_client::AppServerClient::InProcess(...)` 를 만들어서 사용한다 (line 22-26, 2037, 2235 부근). exec 헤드리스 모드도 같은 facade 위에 올라간다.
- 그래서 “app-server” 는 단순히 IDE 용 백엔드가 아니라 **codex 의 모든 클라이언트가 공유하는 단일 컨트롤 플레인** 이라고 봐야 한다. fork 가 turn / thread 라이프사이클을 건드릴 때 진입점은 거의 항상 여기다.

## 2. 두 프로토콜 버전 — v1 (frozen) vs v2 (활발)

`codex-rs/app-server-protocol/src/protocol/` 아래에 두 버전이 공존한다.

- **v1** (`v1.rs`): 기존 클라이언트 호환을 위해 동결됐다. `InitializeParams`, `ClientInfo`, `InitializeCapabilities`, `ApplyPatchApprovalParams`, `ExecCommandApprovalParams`, `Profile`, `SandboxSettings`, `Tools`, `UserSavedConfig` 등 초기 핸드셰이크 + legacy approval/auth 계열 타입만 노출. CLAUDE.md 가 명시: **새 API 는 v1 에 추가하지 않는다.** v1 은 직렬화 형식이 외부 클라이언트와 묶여 있어서 필드 추가/변경이 곧 호환성 깨짐이다.
- **v2** (`v2/`): 활발히 진화하는 표면. `mod.rs` 가 25 개의 모듈 — `thread.rs`, `turn.rs`, `thread_data.rs`, `config.rs`, `mcp.rs`, `permissions.rs`, `realtime.rs`, `process.rs`, `command_exec.rs`, `fs.rs`, `apps.rs`, `plugin.rs`, `feedback.rs`, `notification.rs`, `model.rs`, `account.rs`, `device_key.rs`, `experimental_feature.rs`, `collaboration_mode.rs`, `hook.rs`, `item.rs`, `review.rs`, `windows_sandbox.rs`, `shared.rs` — 을 모두 re-export. CLAUDE.md 의 룰 그대로:
  - 모든 신규 API 는 `app-server-protocol/src/protocol/v2/` 안에 추가.
  - 기존 v1 메서드를 “확장” 하지 말고 v2 에 새 메서드를 만든다.
  - `lib.rs` 가 `pub use protocol::v2::*` 로 와일드카드 재노출하므로 클라이언트 쪽 import 는 `codex_app_server_protocol::ThreadStartParams` 처럼 단순하다 (v1 은 명시적으로 `protocol::v1::Profile` 등으로 한 줄씩 export).

`v1.rs` 상단에는 `core` 의 sandbox / approval enum 을 직접 재사용한 흔적이 남아있고, v2 의 `shared.rs` 는 같은 enum 을 **camelCase 로 재선언** 한 뒤 `v2_enum_from_core!` 매크로로 core ↔ v2 변환 (`to_core()`, `From<Core>`) 을 자동 생성한다. 즉 v2 는 단순한 사본이 아니라 **wire 표현을 코어 표현과 분리한 안티-결합 레이어** 다 — 코어가 kebab-case 를 쓰든 snake_case 를 쓰든 wire 는 항상 camelCase 로 통일된다.

이 분리 덕분에 fork 작업에서 코어 enum 에 variant 를 추가해도 v2 열거형을 따로 관리해 IDE 클라이언트의 호환성을 망치지 않을 수 있다 — 단, 새 variant 를 wire 로 노출하려면 `shared.rs` 의 v2 enum 에도 같이 추가하고 `v2_enum_from_core!` 매크로 라인을 갱신해야 한다.

## 3. Naming / wire format 컨벤션

`CLAUDE.md` 의 “App-server v2 API conventions” 를 그대로 따른다. fork 작업 시 새 RPC 를 추가할 때 반드시 지킬 것.

- 타입 이름:
  - 요청 = `*Params` (예: `ThreadStartParams`)
  - 응답 = `*Response` (예: `ThreadStartResponse`)
  - 알림 = `*Notification` (예: `ThreadStartedNotification`)
- RPC 메서드 문자열은 `<resource>/<method>` 이고 **resource 는 항상 단수**.
  - 예: `thread/read`, `thread/list`, `app/list`, `fs/readFile`, `mcpServer/tool/call` (실제 매크로 테이블이 `common.rs:440-700` 부근).
  - 따라서 `threads/...`, `apps/list` 처럼 복수형은 안 된다.
- Wire format 은 **camelCase**: 모든 타입에 `#[serde(rename_all = "camelCase")]` 를 박는다. 유일한 예외는 `config/*` 계열 RPC — `config.toml` 키와 1:1 매칭돼야 해서 snake_case 그대로 둔다.
- ID 는 boundary 에서 항상 `String`. 시스템 내부에 `ThreadId` newtype 이 있어도 wire 로 나갈 땐 string.
- 타임스탬프는 **i64 Unix seconds**, 필드명은 항상 `*_at` (camelCase 직렬화 시 `*At`).

## 4. TS export — `#[ts(export_to = "v2/")]`

`ts-rs` 매크로로 Rust 타입을 TypeScript 정의로 자동 변환한다. v2 타입은 모두

```rust
#[derive(Serialize, Deserialize, JsonSchema, TS, ExperimentalApi)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadStartParams { ... }
```

처럼 `export_to = "v2/"` 를 달아야 한다. 이렇게 하면 generated TypeScript 가 `codex-rs/app-server-protocol/schema/typescript/v2/` 아래로 떨어져서 SDK 측 import path 가 v1/v2 로 깔끔히 나뉜다 (`shared.rs` 의 `v2_enum_from_core!` 매크로도 자동으로 `export_to = "v2/"` 를 박는다).

마찬가지로 JSON schema 출력은 `schema/json/v2/` 디렉터리로 분리된다. `lib.rs` 가 `generate_ts`, `generate_json`, `generate_internal_json_schema` 같은 export helper 를 노출한다.

## 5. Optional / discriminated union 컨벤션

CLAUDE.md 룰 + 실제 `thread.rs` 패턴을 종합:

- **`*Params` 의 optional 필드** 는 `Option<T>` + `#[ts(optional = nullable)]`. 예 (`thread.rs` line 95-149):
  ```rust
  #[ts(optional = nullable)]
  pub model: Option<String>,
  ```
  `#[ts(optional = nullable)]` 은 `*Params` 입력에만 사용한다 — `*Response` 의 optional 에는 쓰지 않는다.
- **`#[serde(skip_serializing_if = "Option::is_none")]` 를 v2 payload 에 절대 사용 금지.** 유일한 예외는 빈 params 요청(`Option<()>` + `#[ts(type = "undefined")]`).
- **Optional collection 은 `Option<Vec<...>>` + `#[ts(optional = nullable)]`** — `#[serde(default)] pub xs: Vec<...>` 가 아님.
- **bool default-false 필드** 는 `Option<bool>` 이 아니라:
  ```rust
  #[serde(default, skip_serializing_if = "std::ops::Not::not")]
  pub defer_loading: bool,
  ```
  (실 사용처: `shared.rs::DynamicToolSpec`).
- **Discriminated union** 은 serde 와 ts-rs 양쪽에 같은 태그를 박아야 한다:
  ```rust
  #[serde(tag = "type", rename_all = "camelCase")]
  #[ts(tag = "type", rename_all = "camelCase")]
  ```
  한쪽만 박으면 schema/타입 정의가 어긋나서 SDK 가 깨진다.

### `ClientRequestSerializationScope`

`common.rs` 의 매크로 테이블은 각 RPC variant 에 **serialization scope** 도 같이 박아둔다 (`Global("...")`, `Thread { thread_id }`, `ThreadPath { path }`, `CommandExecProcess { process_id }`, `Process { process_handle }`, `FuzzyFileSearchSession { session_id }`, `FsWatch { watch_id }`, `McpOauth { server_name }`). 이 scope 는 message_processor 에서:

- 같은 thread 에 대한 in-flight RPC 를 직렬화 (race 방지) 하고,
- 특정 리소스가 사라지면 (`process/kill` 등) 해당 scope 의 대기 중인 요청을 일괄 정리하고,
- backpressure 큐가 넘칠 때 어떤 그룹을 우선 드롭할지 판정

하는 용도로 쓰인다. 신규 RPC 를 추가할 때 잘못된 scope 를 박으면 thread 간 turn 이 직렬화돼 throughput 이 떨어지거나, 반대로 race 가 생길 수 있으니 기존 비슷한 RPC 의 scope 를 그대로 차용하는 게 안전하다.

### v2 Params 작성 예시 (`ThreadStartParams`)

`v2/thread.rs` 의 `ThreadStartParams` 가 모든 컨벤션을 한 번에 보여준다.

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default,
         JsonSchema, TS, ExperimentalApi)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadStartParams {
    #[ts(optional = nullable)]
    pub model: Option<String>,
    #[ts(optional = nullable)]
    pub model_provider: Option<String>,
    // double-Option: "필드 미지정" vs "명시적 null" 구분이 필요한 케이스
    #[serde(default,
            deserialize_with = "...::deserialize_double_option",
            serialize_with = "...::serialize_double_option",
            skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub service_tier: Option<Option<ServiceTier>>,
    #[experimental(nested)]
    #[ts(optional = nullable)]
    pub approval_policy: Option<AskForApproval>,
    #[experimental("thread/start.permissions")]
    #[ts(optional = nullable)]
    pub permissions: Option<PermissionProfileSelectionParams>,
    #[experimental("thread/start.experimentalRawEvents")]
    #[serde(default)]
    pub experimental_raw_events: bool,
    // ...
}
```

체크포인트:
- 모든 derive 가 한 줄 (`Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS, ExperimentalApi`) — 빠뜨리면 schema 생성이 실패한다.
- camelCase + `export_to = "v2/"`.
- 보통 optional 은 `Option<T>` + `#[ts(optional = nullable)]`, “미지정 vs null” 둘 다 의미가 있을 땐 double-`Option` + `serde_helpers::*_double_option`.
- 메서드 단위 게이팅은 `#[experimental("thread/start.fieldName")]`, 중첩 enum 의 일부 variant 만 experimental 일 땐 `#[experimental(nested)]`.
- 디폴트 false bool 은 `#[serde(default)] pub flag: bool` (skip-serialize-if-not 패턴은 본문 §5 참조).

응답 측 `ThreadStartResponse` 는 `#[ts(optional = nullable)]` 을 쓰지 않고 일반 `Option<T>` 그대로 둔다 — wire 는 항상 `null` 또는 값을 명시한다.

## 6. Pagination — cursor 기반

신규 list 엔드포인트는 cursor pagination 으로 통일한다.

- 요청: `cursor: Option<String>` + `limit: Option<u32>` (둘 다 `#[ts(optional = nullable)]`).
- 응답: `data: Vec<...>` + `next_cursor: Option<String>`.
- 실제 사용 예: `thread/list`, `thread/turns/list`, `experimentalFeature/list`, `mcpServerStatus/list`. README 의 API Overview (line 149, 152, 202, 225) 에서 확인 가능.
- offset/page 기반 (`page=2&page_size=20`) 은 사용하지 않는다 — 새 RPC 를 만들 때 cursor 로 시작할 것.

## 7. Experimental API gating

새 기능을 안정 클라이언트에 노출하기 전에 격리할 수 있는 메커니즘. `codex-rs/app-server-protocol/src/experimental_api.rs` + `codex-experimental-api-macros` 크레이트 조합.

- 메서드/필드 단위 어노테이션:
  ```rust
  #[experimental("thread/start.permissions")]
  pub permissions: Option<PermissionProfileSelectionParams>,
  ```
- 타입에 `derive(ExperimentalApi)` 를 달면 `inventory::collect!` 로 등록된 `ExperimentalField` 목록을 통해 런타임에 “이 요청 안에 experimental 필드가 들어왔는가?” 를 판정할 수 있다 (`experimental_fields()`).
- 메서드 자체가 “대부분 안정인데 일부 필드만 experimental” 인 경우 `common.rs` 의 매크로 테이블에서 `inspect_params: true` 를 줘 부분 게이팅을 시킨다 — `ThreadStart` 가 그 예.
- 클라이언트는 `initialize.params.capabilities.experimentalApi = true` 로 opt-in 해야 experimental 필드를 보낼 수 있다. opt-in 없이 보내면 `<reason> requires experimentalApi capability` 에러로 거절 (`experimental_required_message`).
- README 의 “Experimental API Opt-in” 섹션이 클라이언트 측 가이드.

## 8. Transport 스택

`codex-rs/app-server-transport/src/transport/mod.rs::AppServerTransport` 가 enum 으로 4 가지 모드를 정의한다.

| Listen URL | Variant | 용도 |
| --- | --- | --- |
| `stdio://` (default) | `Stdio` | newline-delimited JSONL — VS Code 확장, exec 등 단일 프로세스 호스팅 |
| `unix://` 또는 `unix://PATH` | `UnixSocket` | 로컬 control-plane. 기본 경로는 `$CODEX_HOME/app-server-control/app-server-control.sock`. 위에서 websocket Upgrade 를 태운다 |
| `ws://IP:PORT` | `WebSocket` | 실험적 원격 transport. 같은 리스너가 `/readyz`, `/healthz` 도 응답 |
| `off` | `Off` | 로컬 transport 비활성화 (remote control 만 쓰는 경우) |

각 transport 는 `start_stdio_connection`, `start_control_socket_acceptor`, `start_websocket_acceptor` 로 구동되며, 모두 공통 채널 (`mpsc::Sender<TransportEvent>`) 에 `ConnectionOpened` / `IncomingMessage` / `ConnectionClosed` 이벤트를 흘린다. `ConnectionOrigin` enum (`Stdio`, `InProcess`, `WebSocket`, `RemoteControl`) 으로 출처를 구분해 device-key API 같이 “로컬에서만 허용” 인 호출을 게이팅한다 (`allows_device_key_requests`).

### Backpressure & overload

- 채널은 모두 bounded (`CHANNEL_CAPACITY = 128`, websocket outbound 는 32 KiB).
- 인입 큐가 가득 차면 새 request 는 JSON-RPC 에러 코드 `-32001` (`OVERLOADED_ERROR_CODE`) + `"Server overloaded; retry later."` 로 거절 (`mod.rs::enqueue_incoming_message`). 클라이언트는 지수 백오프 + jitter 로 재시도해야 한다 (README backpressure 섹션).

### `codex-stdio-to-uds` 보조 크레이트

`codex-rs/stdio-to-uds/` 는 stdio 만 이해하는 외부 클라이언트 (예: 제3자 MCP 호스트) 를 UDS 위로 끌어올리기 위한 어댑터다. `mcp_servers.example = {command="codex-stdio-to-uds", args=["/tmp/mcp.sock"]}` 처럼 설정하면 stdio↔UDS 양방향 프록시가 붙는다. Windows 의 stdlib UDS 미지원을 우회하려고 `codex-uds` (`uds_windows` 백엔드) 를 쓰는 게 포인트.

### Auth (`--ws-auth`)

WebSocket transport 는 비-loopback 주소에 노출될 수 있어서 `--ws-auth` 로 명시적 인증을 켜야 한다 (`app-server-transport/src/transport/auth.rs::WebsocketAuthPolicy`):

- `--ws-auth capability-token --ws-token-file /abs/path` — 파일에서 raw token 을 읽어 SHA-256 verifier 와 비교. 운영 권장 형태.
- `--ws-auth capability-token --ws-token-sha256 HEX` — 해시만 인자로 받고 raw token 은 별도 secret store. 프로세스 listing 에 해시가 노출돼도 인증이 깨지지는 않는다.
- `--ws-auth signed-bearer-token --ws-shared-secret-file /abs/path` — HMAC-signed JWT/JWS bearer. `--ws-issuer`, `--ws-audience`, `--ws-max-clock-skew-seconds` 옵션과 같이 쓴다.

클라이언트는 websocket handshake 의 `Authorization: Bearer <token>` 헤더로 자격증명을 제시하고, 인증은 JSON-RPC `initialize` **이전에** 끝난다. loopback 리스너는 SSH port-forwarding 워크플로우를 위해 비인증을 허용한다 (그래서 startup banner 가 "binds localhost only" 노트를 찍는다).

### Remote control / enrollment

`app-server-transport/src/transport/remote_control/` 는 ChatGPT 클라우드 측의 controller 와 app-server 를 페어링하기 위한 별도 transport 다.

- `start_remote_control` 이 controller 측 websocket 으로 outbound 연결을 만들고, `ServerEvent` 스트림을 양방향으로 멀티플렉싱한다 (`segment.rs`, `protocol.rs`).
- enrollment 는 `enroll.rs` — device key 로 챌린지를 서명해 “이 app-server 가 이 사용자의 디바이스” 임을 증명한다 (README 의 `device/key/*` API 와 짝).
- 상태 변화는 `RemoteControlStatusChangedNotification` (status: `disabled` / `connecting` / `connected` / `errored`) 으로 클라이언트에 push.

## 9. Schema 자동생성 — `just write-app-server-schema`

protocol 변경 시 반드시 돌려야 하는 고정된 명령. 루트 `justfile`:

```
write-app-server-schema *args:
    cargo run -p codex-app-server-protocol --bin write_schema_fixtures -- "$@"
```

내부적으로 `app-server-protocol/src/bin/write_schema_fixtures.rs` 가 `schema_fixtures::write_schema_fixtures` 를 호출해 `schema/json/`, `schema/typescript/` 아래에 `*.json` / `*.ts` 픽스처를 갱신한다. 별도로 `app-server-protocol/src/bin/export.rs` 가 `codex app-server generate-ts --out DIR`, `... generate-json-schema --out DIR` 의 런타임 export 백엔드를 담당한다.

`--experimental` 플래그를 붙이면 experimental 필드까지 포함된 픽스처가 생성된다 (CLAUDE.md). 변경 후에는 `cargo test -p codex-app-server-protocol` 로 fixture diff 테스트가 통과하는지 확인한다.

### Initialize / capabilities 핸드셰이크

각 transport connection 은 단 한 번 `initialize` 요청으로 시작해야 한다 (README Initialization 섹션). 그 전에 다른 RPC 를 보내면 `"Not initialized"`, 두 번째 `initialize` 는 `"Already initialized"` 로 거절된다.

- `params.clientInfo.{name, title, version}` 으로 클라이언트가 자기 식별을 한다. `name` 은 OpenAI Compliance Logs Platform 의 식별자로 쓰이므로 신규 통합 시 등록이 필요하다.
- `params.capabilities` 에 `experimentalApi: true` 를 켜야 §7 의 experimental 필드를 보낼 수 있다.
- `params.capabilities.optOutNotificationMethods` 로 특정 notification 을 connection 단위로 끌 수 있다 (정확 매칭, 와일드카드 없음).
- 응답에는 `userAgent`, `codexHome`, `platformFamily`, `platformOs` 가 들어와서 클라이언트가 서버의 실행 환경을 인식할 수 있다.
- 핸드셰이크가 끝난 뒤 클라이언트는 `initialized` notification 을 다시 보내야 정상 운영 모드에 진입한다.

## 10. TUI 와의 연결 — in-process flow

TUI 에서 본 흐름은 다음과 같다 (`codex-rs/tui/src/lib.rs`).

1. `main` 이 CLI 인자를 파싱한 뒤, **원격 app-server 에 붙을지 / 자기 안에서 띄울지** 를 결정한다.
2. 원격 (`--app-server-url ws://…`) 인 경우: `RemoteAppServerClient::connect(RemoteAppServerConnectArgs { ... })` (line 384) — 일반 websocket transport 위에서 JSON-RPC 클라이언트가 된다.
3. 로컬 (default) 인 경우: `codex_app_server::in_process::start(...)` 를 호출해 같은 프로세스 안에서 app-server runtime 을 띄우고, `InProcessClientHandle` 을 받아서 `AppServerClient::InProcess(...)` 로 감싼다 (line 2037, 2235). 이 핸들 위에서 typed request (`ThreadStartParams`, `TurnStartParams`, …) 를 호출하면 그대로 message_processor 까지 흘러간다.
4. `app-server-client` (`codex-rs/app-server-client/src/lib.rs`) 가 두 모드를 추상화한 facade 다. 워커 태스크가 가운데 끼어 caller 쪽 `mpsc` 와 server 쪽 `mpsc` 를 잇고, queue overload 를 `InProcessServerEvent::Lagged` 로 surfacing 해 backpressure 를 시각화할 수 있게 해준다.
5. exec 헤드리스 모드도 같은 facade 를 쓴다 — 즉 fork 가 transport / config / hooks 어디를 건드려도 단일 진입점 `codex-app-server-client` 가 갈아끼우면 모든 surface 에 일관되게 반영된다.

테스트 진입점은 `codex-rs/app-server-test-client/` — fixtures 와 함께 풀 RPC 시퀀스를 돌릴 수 있는 클라이언트다. 새 v2 메서드를 추가했을 때 통합 테스트를 여기다 붙이면 된다.

## 11. 주요 파일 빠른 인덱스

| 위치 | 역할 |
| --- | --- |
| `codex-rs/app-server/src/main.rs` | 바이너리 entrypoint (`AppServerArgs`, `--listen`, `--ws-auth`) |
| `codex-rs/app-server/src/lib.rs` | runtime 엔트리, `run_main_with_transport_options` |
| `codex-rs/app-server/src/message_processor.rs` | RPC dispatch core |
| `codex-rs/app-server/src/in_process.rs` | TUI/exec 가 쓰는 in-process 부팅 |
| `codex-rs/app-server-protocol/src/lib.rs` | v1 + v2 type re-export |
| `codex-rs/app-server-protocol/src/protocol/common.rs` | `ClientRequest` 매크로 테이블 (RPC 메서드 ↔ Params/Response 매핑) |
| `codex-rs/app-server-protocol/src/protocol/v2/*.rs` | resource 별 신규 API |
| `codex-rs/app-server-protocol/src/experimental_api.rs` | `ExperimentalApi` trait + `ExperimentalField` inventory |
| `codex-rs/app-server-protocol/schema/{json,typescript}/v2/` | 자동 생성 schema 산출물 |
| `codex-rs/app-server-transport/src/transport/mod.rs` | `AppServerTransport` enum, `TransportEvent`, overload 처리 |
| `codex-rs/app-server-transport/src/transport/{stdio,unix_socket,websocket}.rs` | 각 transport 구현 |
| `codex-rs/app-server-transport/src/transport/remote_control/` | controller 페어링 / enrollment |
| `codex-rs/app-server-client/src/lib.rs` | TUI/exec 가 쓰는 facade (`AppServerClient`, `InProcessClientHandle`) |
| `codex-rs/app-server-client/src/remote.rs` | `RemoteAppServerClient` (ws 클라이언트) |
| `codex-rs/app-server-test-client/` | 통합 테스트용 클라이언트 |
| `codex-rs/stdio-to-uds/` | stdio → UNIX domain socket 어댑터 (보조) |

새로운 RPC 를 추가할 때의 일반적인 절차는:

1. `app-server-protocol/src/protocol/v2/<resource>.rs` 에 `*Params` / `*Response` / `*Notification` 추가 (camelCase, ts export, optional 룰 준수).
2. `common.rs` 의 매크로 테이블에 `<Variant> => "<resource>/<method>" { ... }` 한 줄 추가 (serialization scope 명시).
3. `app-server/src/message_processor.rs` 에서 dispatch 추가, 실제 핸들러는 적절한 `request_processors/*.rs` 모듈로.
4. `just write-app-server-schema` 실행 후 fixture diff 커밋.
5. `cargo test -p codex-app-server-protocol` + `cargo test -p codex-app-server` 로 검증.
6. 필요하면 `app-server-test-client` 에 통합 테스트 추가.
