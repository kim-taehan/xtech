# 17. Approval policy & Guardian (auto-review)

이 문서는 xtech 가 모델의 위험 행위 (shell 실행, `apply_patch`, network egress, MCP tool call …) 를 사용자 또는 자동 리뷰어에게 어떻게 가져오는지 설명한다. 두 축이 있다:

1. **Approval policy** (`AskForApproval` enum) — "언제 사용자 승인을 받을지"의 정책.
2. **Guardian / auto-review** — 정책상 사용자에게 물어봐야 하는 요청을 *별도 LLM 세션* 으로 자동 판단해서 통과/거부하는 부가 메커니즘.

폐쇄망 fork 입장에서 두 번째 항목은 "한 번의 사용자 turn 이 LLM 호출을 N+1 번으로 만든다" 는 비용/지연 함의가 있다 — 끝에서 다시 정리한다.

## 1. `AskForApproval` enum

정의는 `codex-rs/protocol/src/protocol.rs:912` (`pub enum AskForApproval`):

| variant | wire (kebab-case) | 의미 |
| --- | --- | --- |
| `UnlessTrusted` | `untrusted` | "is_safe_command()" 로 판단된 read-only 커맨드만 자동 승인. 그 외는 전부 사용자에게 prompt. 가장 보수적. |
| `OnFailure` | `on-failure` | **deprecated**. 모든 커맨드를 sandbox 내에서 실행하고 실패하면 unsandboxed 재실행 승인을 요청. interactive 는 `OnRequest`, headless 는 `Never` 권장. |
| `OnRequest` | `on-request` | 기본값. 모델이 `with_additional_permissions` / `require_escalated` 등으로 *명시적으로* 승인을 요청할 때만 사용자에게 prompt. |
| `Granular(GranularApprovalConfig)` | `granular` | 카테고리별 (`sandbox_approval`, `rules`, `skill_approval`, `request_permissions`, `mcp_elicitations`) on/off. `false` 인 카테고리는 사용자 prompt 없이 즉시 거부 (모델 입장에서는 "denied"). |
| `Never` | `never` | 사용자에게 절대 묻지 않음. 실패는 그대로 모델에게 반환 — `codex exec` 헤드리스에 적합. |

`Default` 는 `OnRequest`.

`GranularApprovalConfig` 의 각 필드 (`codex-rs/protocol/src/protocol.rs:946`) 는 어떤 종류의 approval prompt 를 통과시킬지 결정한다:

- `sandbox_approval` — shell tool 의 `with_additional_permissions` / sandbox-escape 요청.
- `rules` — execpolicy `prompt` 룰에서 트리거된 prompt.
- `skill_approval` — skill 스크립트 실행에서 트리거된 prompt.
- `request_permissions` — `request_permissions` tool 직접 호출.
- `mcp_elicitations` — MCP elicitation prompt.

`false` 로 두면 사용자 UI 가 뜨지 않고 자동 거부된다. 즉 `Granular` 는 "어떤 카테고리는 통과시키고 어떤 카테고리는 *사람한테 묻지도 말고 즉시 거부*" 라는 세밀화된 변종이지, 카테고리별로 자동 *허용* 을 켜는 기제가 아니다 (자동 허용은 sandbox policy 와 `is_safe_command` 로 결정된다).

또 한 가지 중요한 invariant: 이 enum 은 **"자동 허용/자동 거부 이외의 회색지대를 어떻게 다룰지"** 만 결정하지, sandbox 의 권한 자체를 넓히거나 좁히지 않는다. 권한 자체는 `SandboxPolicy` (`DangerFullAccess` / `ReadOnly` / `WorkspaceWrite` / …) 가 별도로 책임지며, `AskForApproval` 은 sandbox 가 거부한 동작에 대한 "사람한테 물어볼지" 의 정책일 뿐이다.

## 2. 승인 흐름 (high level)

대략 다음과 같다:

```
모델이 shell tool 호출 (또는 apply_patch/MCP/network)
  -> codex-core 가 SandboxPolicy + AskForApproval + 커맨드 분류 평가
     - "safe & 정책상 자동 허용" 이면 그냥 실행
     - 자동 거부면 사람이 알 새 없이 denied 리턴
     - "사람 또는 reviewer 한테 물어봐야 함"이면 ApprovalRequest event emit
  -> EventMsg::ExecApprovalRequest / ApplyPatchApprovalRequest / ...
       (codex-rs/protocol/src/protocol.rs:1387, 1399)
  -> app-server 가 클라이언트(TUI / IDE / app-server-rpc)로 forward
  -> 클라이언트 응답 (ReviewDecision::{Approved,Denied,Abort,...}) 을
     core 로 다시 전달 -> 모델 turn 재개
```

Headless 진입점 (`codex exec`) 은 `lib.rs:404` 에서 무조건 `approval_policy = AskForApproval::Never` 를 강제한다. `codex` top-level 도 `cli/src/main.rs:1380` 에서 `--dangerously-bypass-approvals-and-sandbox` 가 켜지면 같은 값을 박아 넣는다 — 즉 "사용자 prompt UI 가 없는 컨텍스트는 무조건 Never" 라는 invariant.

TUI 는 그 외 정책을 모두 지원하며, 사용자가 prompt 에 응답하면 `ReviewDecision` 을 turn 으로 되돌려준다. 이 enum 은 `codex-rs/protocol/src/protocol.rs` 안에 정의돼 있고 `Approved`, `Denied`, `Abort`, `TimedOut`, `ApprovedForSession` 등의 변종을 가진다. 이벤트 측면에서는 `EventMsg::ExecApprovalRequest` (`protocol.rs:1387`) 와 `EventMsg::ApplyPatchApprovalRequest` (`protocol.rs:1399`) 가 가장 흔한 두 종이고, MCP / network egress / `request_permissions` 도 각자 별도의 ApprovalRequest 이벤트를 가진다.

승인 요청이 발행되는 **위치** 는 turn loop 의 tool dispatch 단계 — 모델이 `function_call` 을 내리면, core 가 (1) 도구 인자를 sanitize 하고 (2) sandbox/approval 정책을 보고 분류한 다음 (3) 자동 통과면 즉시 실행, 자동 거부면 즉시 `function_call_output` 으로 거절 사유를 모델에 반환, 회색지대면 `EventMsg::*ApprovalRequest` 를 발행하면서 동시에 코루틴을 `ReviewDecision` 수신용 채널에 park 한다. 클라이언트 응답이 도착해야 그 코루틴이 깨어나서 실행 또는 거부 경로로 간다. Guardian 이 활성화돼 있으면 이 "park & wait" 단계가 사용자 대신 별도 LLM 호출로 채워진다.

## 3. `--full-auto` 의 현재 위치

upstream 에서 `--full-auto` 는 *deprecated, 트랩 플래그* 다. xtech 도 그대로 들고 있다:

- `codex-rs/exec/src/cli.rs:38-46` — `removed_full_auto: bool`. clap 정의는 `hide = true` 로 숨겨져 있고 `dangerously_bypass_approvals_and_sandbox` 와 conflict.
- `cli.rs:99-107` — flag 가 켜져 있으면 `removed_full_auto_warning()` 가 다음 문구를 반환:

  > `warning: --full-auto is deprecated; use --sandbox workspace-write instead.`

- `codex-rs/exec/src/lib.rs:224, 281` — 경고를 stderr 에 찍은 뒤, **여전히** sandbox mode 를 `workspace-write` 로 자동 설정해서 동작은 시켜준다. 즉 호환을 위해 살아있긴 하다.

따라서 fork 사용자에게 권장할 진입점은 `--sandbox workspace-write` (와 적절한 `--ask-for-approval`) 의 조합이고, `--full-auto` 는 마이그레이션 단계의 alias 로만 봐야 한다. 결합 의미는: **샌드박스가 workspace-write 로 묶여있는 상태에서 그 안의 자동 동작을 폭넓게 허용** — sandbox escape 가 필요한 행위만 prompt 하므로, 위험은 sandbox boundary 가 1차로 막아준다.

TUI 측에는 별도 "full-auto 모드 토글" 이 없다 — 모드 선택은 이제 `--ask-for-approval` 와 `--sandbox` 두 축으로만 이루어지고, 자동화를 더 강하게 가고 싶으면 `AskForApproval::OnRequest` + `approvals_reviewer = auto_review` (Guardian) 조합이 사실상 그 역할을 한다.

## 4. Guardian (auto-review) 메커니즘

코드 위치: `codex-rs/core/src/guardian/`. `mod.rs` 가 진입점이고, `review.rs` 가 실제 흐름, `review_session.rs` 가 별도 LLM 세션을 띄워주는 매니저, `prompt.rs` 가 transcript 압축 + JSON schema, `approval_request.rs` 가 입력 정규화.

핵심 상수 (`mod.rs:43-55`):

```rust
const GUARDIAN_PREFERRED_MODEL: &str = "codex-auto-review";
pub(crate) const GUARDIAN_REVIEW_TIMEOUT: Duration = Duration::from_secs(90);
pub(crate) const GUARDIAN_REVIEWER_NAME: &str = "guardian";
pub(crate) const MAX_CONSECUTIVE_GUARDIAN_DENIALS_PER_TURN: u32 = 3;
pub(crate) const MAX_TOTAL_GUARDIAN_DENIALS_PER_TURN: u32 = 10;
```

활성화 조건은 `review.rs:144` 의 `routes_approval_to_guardian`:

```rust
matches!(
    turn.approval_policy.value(),
    AskForApproval::OnRequest | AskForApproval::Granular(_)
) && turn.config.approvals_reviewer == ApprovalsReviewer::AutoReview
```

즉 두 조건이 모두 참일 때만 사용자 prompt 가 guardian session 으로 우회된다:

1. 정책이 `OnRequest` 또는 `Granular`. (`Never` / `UnlessTrusted` 는 우회하지 않는다.)
2. `config.approvals_reviewer = "auto_review"` (또는 별칭 `"guardian_subagent"`). 기본값은 `"user"` 이므로 명시적으로 켜야 한다.

`ApprovalsReviewer` 정의는 `codex-rs/protocol/src/config_types.rs:88` — `User` (default) / `AutoReview` 두 값이고, 후자는 wire 상 `guardian_subagent` 와 `auto_review` 둘 다 받아준다 (legacy 호환).

## 5. `GuardianAssessment` 구조

guardian session 의 출력 contract (`mod.rs:60`):

```rust
pub(crate) struct GuardianAssessment {
    pub risk_level: GuardianRiskLevel,                  // low | medium | high | critical
    pub user_authorization: GuardianUserAuthorization,  // unknown | low | medium | high
    pub outcome: GuardianAssessmentOutcome,             // allow | deny
    pub rationale: String,
}
```

각 enum 정의는 `codex-rs/protocol/src/approvals.rs:87-109`. 모두 `lowercase` serde rename 으로 wire 에 직렬화되며 TS export 된다.

- `risk_level` — 모델이 매긴 거친 위험도. 사용자에게 표시되는 warning 메시지에 등장.
- `user_authorization` — 현재까지의 transcript 가 그 행위를 *얼마나 직접적으로* 허락하고 있는가. 가령 "테스트 돌려" 라고 한 user 가 `rm -rf` 를 정당화하지는 않는다.
- `outcome` — 최종 결정. `Allow` 면 `ReviewDecision::Approved`, `Deny` 면 `ReviewDecision::Denied`.
- `rationale` — 사람이 읽을 수 있는 이유. 거부 시에는 `GuardianRejection` 으로 보존됐다가 모델에게 `tool result` 의 일부로 다시 주입된다.

거부 객체 (`mod.rs:67`):

```rust
pub(crate) struct GuardianRejection {
    pub rationale: String,
    pub source: GuardianAssessmentDecisionSource, // 현재 Agent 만 존재
}
```

거부가 모델로 돌아갈 때는 `review.rs:44` 의 고정 instruction 이 함께 붙는다 — "우회/간접 실행/정책 회피 금지, 더 안전한 대안 또는 사용자 직접 승인 없이는 같은 목표를 다시 시도하지 말 것".

타임아웃은 거부와 다르게 처리된다 (`review.rs:52`): `ReviewDecision::TimedOut` 을 돌려주고, 모델에게는 "안전하지 않다고 단정하지는 말되 한 번 재시도하거나 사용자에게 물어보라" 는 가이드를 준다.

흐름의 요약 (`run_guardian_review`, `review.rs:233`):

```
GuardianAssessmentEvent { status: InProgress }       // UI 진행 표시
  -> run_guardian_review_session (별도 model + 별도 prompt)
     -> Completed(assessment) | Timeout | Cancelled | PromptBuild/Session/Parse error
  -> 결과를 GuardianWarning event 로 사용자에게 노출
  -> GuardianAssessmentEvent { status: Approved/Denied/TimedOut/Aborted } 송출
  -> ReviewDecision 반환 (turn 재개에 사용)
```

prompt build/session/parse 에러는 **fail closed** — `risk_level=High`, `outcome=Deny` 의 합성 assessment 로 처리된다 (`review.rs:439`). 단 이 합성 거부는 *circuit breaker 의 denial 카운터를 증가시키지 않는다* (`review.rs:447` 의 `count_denial_for_circuit_breaker = false`). 게이트웨이 장애가 사용자 turn 을 갑자기 interrupt 시키지 않게 하기 위한 안전장치다 — guardian *모델의 의도적 거부* 만 카운터에 들어간다.

`review_session.rs` 의 sandbox / 권한 설정도 fail-closed 의 일부다: guardian 자신은 `approval_policy = never`, read-only sandbox 로 고정 운영되어 *guardian 이 또 다른 approval 을 트리거할 수 없다*. nested approval 을 만들면 그 자체가 무한 재귀의 원인이 되기 때문에 코드 차원에서 차단돼 있다.

### 5.1 사용자가 거부를 뒤집는 경우

`mod.rs:48` 에 다음 상수가 있다:

```rust
pub(crate) const AUTO_REVIEW_DENIED_ACTION_APPROVAL_DEVELOPER_PREFIX: &str =
    "The user has manually approved a specific action that was previously `Rejected`.";
```

guardian 이 거부한 동작을 사용자가 *수동으로* 다시 승인했을 때, 그 승인 결과가 모델 컨텍스트에 들어갈 때 위 prefix 가 붙은 developer 메시지가 같이 주입된다. 모델이 "이전에 거부됐던 동작이지만 사용자가 명시적으로 풀어줬다" 는 사실을 인식해야 같은 동작에 대한 추가 회피 시도를 멈출 수 있기 때문 — 그렇지 않으면 모델이 거절 instruction (§5 의 `GUARDIAN_REJECTION_INSTRUCTIONS`) 만 보고 "사용자 명시 승인" 채널을 못 알아차린다.

## 6. Circuit breaker

guardian 이 한 turn 안에서 무한히 거부를 반복하면 모델이 우회하려고 변형을 짜내며 같은 self-loop 에 빠질 수 있다. 이를 끊기 위한 카운터가 `mod.rs:73-121` 의 `GuardianRejectionCircuitBreaker`:

- turn 당 두 카운터 — `consecutive_denials`, `total_denials` — 를 유지.
- `record_denial` 에서 다음 중 하나라도 임계 도달이면 `InterruptTurn` 액션을 돌려준다:
  - `consecutive_denials >= MAX_CONSECUTIVE_GUARDIAN_DENIALS_PER_TURN` (3)
  - `total_denials >= MAX_TOTAL_GUARDIAN_DENIALS_PER_TURN` (10)
- 한 번 trigger 되면 같은 turn 에서는 `interrupt_triggered = true` 로 latch 되어 추가 trigger 안 함.
- `record_non_denial` 은 consecutive 만 0 으로 리셋 (total 은 누적 유지).

Interrupt 가 트리거되면 (`review.rs:188`):

1. `GuardianWarning` event 로 사용자에게 "너무 많이 거부돼서 turn 을 중단합니다" 메시지 송출.
2. 백그라운드 task 가 `session.abort_turn_if_active(turn_id, TurnAbortReason::Interrupted)` 호출.

따라서 guardian 거부가 무한 루프를 만들 가능성은 코드 차원에서 차단돼 있다.

## 7. 모델 선택 — 왜 `codex-auto-review` 를 그대로 두었나

guardian session 은 가능한 한 *전용 small/cheap* 모델을 쓰도록 설계돼 있다. `review.rs:642` 에서:

1. 현재 사용 가능한 모델 목록 (`models_manager.list_models(Offline)`) 에서 `slug == "codex-auto-review"` 인 preset 을 찾는다.
2. 있으면 그 모델 + `Low` reasoning effort (지원하면) 사용.
3. 없으면 turn 의 현재 모델 / reasoning 을 그대로 fallback.

`codex-auto-review` preset 은 `codex-rs/models-manager/models.json:109` 에 정의돼 있고 `"visibility": "hide"` 로 picker 에서는 보이지 않는다. **fork 에서도 이 엔트리는 그대로 유지** — 제거하면 fallback 경로 (메인 모델 재사용) 로 떨어져서 비용이 1.x → 2x 로 올라가고, base_instructions 에 박혀있는 guardian-전용 평가 페르소나를 잃게 된다.

대체 가능성: 단순히 `models.json` 의 `slug` 만 바꾸려면 `GUARDIAN_PREFERRED_MODEL` 상수가 코드에 박혀있어 같이 고쳐야 한다. 더 자연스럽게는 **xtech 의 chat-completions 게이트웨이 쪽에서 `codex-auto-review` 라는 이름의 routing 만 잘 해주는** 것이 맞다 — 게이트웨이가 같은 베이스 모델을 더 작은 컨텍스트/낮은 reasoning 으로 라우팅한다고 가정하면, 코드 수정 없이도 비용을 깎을 수 있다. (현 fork 가 ollama/qwen 게이트웨이만 보고 있다면, 그 게이트웨이가 `codex-auto-review` slug 를 알맞은 작은 모델에 매핑하도록 하는 쪽이 깨끗하다.)

## 8. 타임아웃

`GUARDIAN_REVIEW_TIMEOUT = Duration::from_secs(90)` (`mod.rs:44`). 이 값은 외부에서 설정 가능하지 않다 — turn 한 건이 90 초를 넘기면 무조건 timeout 으로 처리된다. timeout 은:

- denial 카운터를 증가시키지 *않는다* (`record_guardian_non_denial`).
- 사용자에게는 `GuardianWarning` + `GuardianAssessmentEvent { status: TimedOut }` 두 이벤트로 표시.
- 모델에게는 "재시도 한 번 또는 user prompt 요청 가능" 이라는 instruction 이 함께 반환.

폐쇄망 + 작은 로컬 모델 환경에서 90 초가 짧을 가능성에 유의 — 실측 후 필요하면 상수를 늘리는 fork 패치가 필요하다.

이상적으로는 timeout 도 `config.toml` 키로 노출하는 게 깨끗하지만, 현재 코드 베이스에선 `Duration::from_secs(90)` 가 const 로 박혀 있고 별도 override 경로가 없다. fork 패치가 필요하다면 `mod.rs` 의 상수 한 줄 변경 + 관련 테스트 (`tests.rs`) 의 timeout 어서션 검토면 충분하다.

## 9. fork (xtech) 가 신경 써야 할 점

실용 관점 정리:

- **추가 LLM 호출**. `approvals_reviewer = auto_review` 가 켜진 turn 에서 사용자 prompt 가 발생할 때마다 guardian session 이 한 번 더 LLM 을 친다. polling 호출이 아니라 turn 의 critical path 에 있으므로 모델 응답 지연이 그대로 사용자 체감 지연이 된다.
- **prompt 압축 비용**. `prompt.rs` 가 transcript 를 별도 토큰 budget (`GUARDIAN_MAX_MESSAGE_TRANSCRIPT_TOKENS = 10_000`, `GUARDIAN_MAX_TOOL_TRANSCRIPT_TOKENS = 10_000`) 으로 잘라서 보낸다. 작은 로컬 모델은 그래도 무거울 수 있으므로 turn 길이가 길어지면 검토 필요.
- **세션 재사용**. `review.rs:597` 주석대로 guardian 은 가능하면 idle 상태의 trunk session 을 재사용하고, busy 면 last-committed rollout 에서 ephemeral fork 한다. 즉 prompt-cache key 가 안정되도록 짜여 있어 같은 게이트웨이가 캐싱을 지원하면 비용이 잘 떨어진다.
- **fail-closed**. 게이트웨이 장애나 응답 파싱 실패 시 모두 `Deny` 로 처리되므로, 게이트웨이 가용성이 낮으면 turn 진행이 막힐 수 있다. 헤드리스 (`codex exec`) 는 어차피 `AskForApproval::Never` 라 영향을 받지 않지만, TUI 상호작용이 자동화에 묶여있는 시나리오라면 모니터링 포인트.
- **default 는 user**. `ApprovalsReviewer::User` 가 default 이므로 fork 가 사용자에게 강제로 auto-review 를 켜는 일은 없다 — 명시적으로 `~/.xtech/config.toml` 또는 `-c approvals_reviewer=auto_review` 로 켜야 한다. 따라서 *비용을 걱정한다면 default 를 그대로 두면 된다*.
- **로깅 / analytics**. `track_guardian_review` 호출이 `analytics_events_client.track_guardian_review` 로 모든 결정을 보낸다 (`review.rs:161`). xtech 가 analytics endpoint 를 별도 라우팅하지 않는다면 이 호출이 어디로도 가지 않는 dead-letter 가 되므로, 디버깅이 필요하면 `GuardianAssessmentEvent` 자체를 클라이언트 쪽 로그로 잡는 게 빠르다.
- **netproxy 상속**. guardian session config 빌드 시 `live_network_config = network_proxy.proxy().current_cfg()` 를 그대로 복제해서 부모 turn 의 managed-network 허용 목록을 상속한다 (`review.rs:618`). 폐쇄망 fork 입장에서는 guardian 도 같은 (제한된) egress 룰 안에서 동작한다는 뜻 — guardian 만 별도 egress 가 필요한 일은 발생하지 않는다.

## 10. 관련 파일 빠른 색인

- `codex-rs/protocol/src/protocol.rs:912` — `AskForApproval`.
- `codex-rs/protocol/src/protocol.rs:946` — `GranularApprovalConfig`.
- `codex-rs/protocol/src/approvals.rs:85-170` — `GuardianRiskLevel`, `GuardianUserAuthorization`, `GuardianAssessmentOutcome`, `GuardianAssessmentStatus`, `GuardianAssessmentDecisionSource`, `GuardianAssessmentAction`, `GuardianAssessmentEvent`.
- `codex-rs/protocol/src/config_types.rs:88` — `ApprovalsReviewer`.
- `codex-rs/core/src/guardian/mod.rs` — 상수, `GuardianAssessment`, `GuardianRejection`, circuit breaker.
- `codex-rs/core/src/guardian/review.rs` — `routes_approval_to_guardian`, `run_guardian_review`, error mapping, fail-closed 합성.
- `codex-rs/core/src/guardian/review_session.rs` — guardian 전용 세션 빌드/실행, trunk reuse / ephemeral fork.
- `codex-rs/core/src/guardian/prompt.rs` — transcript 압축, JSON output schema, parser.
- `codex-rs/core/src/guardian/approval_request.rs` — `GuardianApprovalRequest` (입력 정규화).
- `codex-rs/models-manager/models.json:109` — `codex-auto-review` preset (hidden).
- `codex-rs/exec/src/cli.rs:38-107` — deprecated `--full-auto` 트랩 + warning.
- `codex-rs/exec/src/lib.rs:224, 281, 404` — `--full-auto` → `workspace-write` 매핑, 헤드리스에서 `AskForApproval::Never` 강제.
