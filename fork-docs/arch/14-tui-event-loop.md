# 14. TUI event loop & 입력 처리

이 문서는 `xtech` TUI (`codex-rs/tui/`) 의 **동적 / 시간 축** 을 정리한다. `13-tui-structure.md` (정적 모듈 / 위젯 트리) 와 짝을 이루며, 한 키 입력이 어떤 채널을 따라 흘러 화면에 다시 반영되기까지의 경로를 모두 따라간다.

## 1. 진입점과 메인 루프

### 1.1 부팅 단계
`run_ratatui_app` (`codex-rs/tui/src/lib.rs:1085`) 이 TUI 부팅을 책임진다.

1. `tui::init()` (`codex-rs/tui/src/tui.rs`, 약 250 line 영역) 가 stdout 에 alt-screen 진입 직전 상태를 만들고 `ratatui::Terminal` 을 반환한다. 그 안에서 `set_modes()` (`tui.rs:107-119`) 가 `enable_raw_mode()` + `EnableBracketedPaste` + `EnableFocusChange` 를 한 번에 켠다.
2. `Tui::new(terminal)` (`tui.rs:394-428`) 으로 래퍼를 만든다. 이 시점에 `FrameRequester` (broadcast 채널 기반 그리기 스케줄러) / `EventBroker` (crossterm 입력 브로커) / `terminal_focused` / `enhanced_keys_supported` 캐시가 함께 만들어진다.
3. `TerminalRestoreGuard` (`lib.rs:1120`) 와 패닉 훅 (`lib.rs:1111-1115`) 을 설치해 어떤 경로로 빠져나가도 raw mode 가 풀리도록 한다.
4. `App::run(...)` (`app.rs:602-1080`) 으로 본 루프에 진입한다.

### 1.2 본 루프
실제 메인 루프는 `App::run` 안의 `loop { let control = select! { ... } }` (`app.rs:991-1050`). 4 개 입력 소스를 `tokio::select!` 로 다중화한다.

루프 진입 직전에 한 번 일어나는 일이 더 있다.

- `unbounded_channel::<AppEvent>()` 으로 in-process 메시지 버스를 깐다 (`app.rs:622`). 이 채널은 메인 task 만이 receiver 이고 위젯 / 백그라운드 task / `AppEventSender` 클론들이 모두 sender. 즉 위젯이 “app 에 알림” 을 보내는 길은 정확히 이 한 채널이다.
- `app_server.bootstrap(&config).await` (`app.rs:665`) 가 `account/read` + `model/list` RPC 두 발을 쳐서 default model / available models / auth 메타를 받아온다. 이 결과로 `ModelCatalog`, `SessionTelemetry`, `WorkspaceCommandRunner` 가 wired 된다.
- `tui.event_stream()` (`tui.rs:534-550`) 에서 받은 `Pin<Box<dyn Stream<Item = TuiEvent>>>` 를 `tui_events` 변수에 보관해 `select!` 의 한 leg 으로 쓴다.

```text
                   +----------------------+
   AppEvent  --->  | app_event_rx.recv()  |  --> handle_event       (event_dispatch.rs)
   채널            +----------------------+
                   +----------------------+
   active thread   | active.recv()        |  --> handle_active_thread_event
   (서브 thread)   +----------------------+      (thread_routing.rs)
                   +----------------------+
   TuiEvent stream | tui_events.next()    |  --> handle_tui_event   (app.rs:1082)
                   +----------------------+      Key/Paste/Draw/Resize
                   +----------------------+
   AppServer 이벤트| app_server.next_event()| -> handle_app_server_event
                   +----------------------+      (app_server_events.rs)
```

`select!` 가 끝나면 반환된 `AppRunControl` 이 `Continue / Exit(reason)` 중 하나로 재합성된다. `Exit` 이면 `app_server.shutdown().await` 후 `terminal.clear()` 로 마무리하고 `AppExitInfo` 를 돌려준다 (`app.rs:1052-1079`).

본 루프가 처리하는 분기의 **gate 조건** 도 주의할 가치가 있다.

- `active.recv()` 분기는 `App::should_handle_active_thread_events(...)` (`thread_routing.rs:1304`) 가 true 일 때만 활성. 즉 startup 직후 첫 `SessionConfigured` 가 도착하기 전에는 sub-thread 이벤트를 일부러 흘려 보낸다 — UI 가 아직 thread state 를 못 만든 상태에서 turn 이벤트가 들어오면 race 가 나기 때문.
- `app_server.next_event()` 분기는 `listen_for_app_server_events` 플래그로 보호된다 (`app.rs:1029-1038`). 스트림이 한 번 끝나면 (`None`) 다시 폴링하지 않고 경고만 찍는다.
- 매 round 끝에 `should_stop_waiting_for_initial_session(...)` 으로 `waiting_for_initial_session_configured` 플래그를 풀어 `active` 분기를 “정상” 모드로 전환한다 (`app.rs:1040-1045`).

## 2. Crossterm 이벤트 → `TuiEvent` 매핑

`TuiEvent` enum (`tui.rs:354-366`) 은 `Key(KeyEvent) / Paste(String) / Resize / Draw` 네 변종만 가진다. crossterm 의 `Event::Mouse` 등은 일찌감치 버려지고, `Draw` 는 입력 이벤트가 아니라 `FrameRequester` 가 보내는 **렌더 트리거** 다.

매핑은 `TuiEventStream::map_crossterm_event` (`tui/event_stream.rs:237-260`) 에서 이뤄진다.

- `Event::Key(...)` → `TuiEvent::Key(...)`. Unix 면 그 직전에 `SUSPEND_KEY` (Ctrl-Z) 를 가로채서 `suspend_context.suspend(...)` 를 호출하고 (`tui.rs:job_control` 참조), 복귀 후 `TuiEvent::Draw` 를 한 번 흘려 화면을 다시 그리게 한다.
- `Event::Resize(_, _)` → `TuiEvent::Resize`. 별도 열로 두는 이유는 `app.rs:1088-1096` 의 resize-reflow 사전 처리 분기 때문이다.
- `Event::Paste(s)` → `TuiEvent::Paste(s)`. 본 루프 단에서 CRLF→LF 정규화 (`app.rs:1110`) 후 `chat_widget.handle_paste(pasted)` 로 들어간다.
- `Event::FocusGained / FocusLost` → `terminal_focused` 원자 갱신 + 팔레트 재질의 (`requery_default_colors`). FocusLost 는 `TuiEvent` 로 변환되지 않고, 대신 데스크탑 알림 게이트 (`Tui::notify`, `tui.rs:506-532`) 의 입력으로 쓰인다.
- 그 외 (Mouse 등) → `None` 이라 루프가 한 번 더 돈다 (`event_stream.rs:179-221`).

`KeyEvent` 자체에는 modifier (`KeyModifiers::CONTROL` 등) / `KeyCode` / kind (`Press`/`Release`/`Repeat`, kitty enhancement 가 잡혔을 때만 의미가 있다) / state 가 다 들어 있다. `enhanced_keys_supported` (`tui.rs:401-402`) 캐시는 부팅 시 한 번만 결정되며, 본 루프 / 위젯 / approval overlay 가 같은 키를 다르게 해석할 때 (예: macOS Option+Left 의 word-motion fallback, `app/input.rs:97-136`) 분기 조건으로 자주 쓰인다.

`TuiEventStream::poll_next` (`event_stream.rs:265-291`) 는 매 호출마다 draw / crossterm 폴링 순서를 round-robin 으로 뒤집어 둘 중 하나가 starvation 되지 않게 한다. 단일 입력 소스를 공유해야 해서 `EventBroker` (`event_stream.rs:51-115`) 가 mutex 로 감싸 보호하고, `pause_events()` 가 호출되면 underlying `crossterm::event::EventStream` 을 **드롭** 해서 stdin 을 완전히 놓아준다 — vim / less 같은 외부 프로그램으로 핸드오프할 때를 위한 디자인이다 (`event_stream.rs:9-18` 주석에 근거가 적혀 있다).

## 3. App-server 와의 양방향 통신

TUI 는 `codex-core` 를 직접 호출하지 않고 **항상 `AppServerSession`** (`app_server_session.rs:175`) 을 통해 talk 한다. 내부 `AppServerClient` 는 `InProcessAppServerClient` (같은 프로세스에서 spawn 된 app-server tokio task) 또는 `RemoteAppServerClient` (WebSocket) 가 될 수 있고 (`lib.rs:22-27`), TUI 는 이 추상화 위에서만 동작한다. 자세한 wire 는 `03-wire-protocol.md`.

### 3.1 outbound — TUI → app-server

1. 사용자가 키를 누르면 `ChatWidget` 이 `AppEventSender::send(...)` (`app_event_sender.rs:35-44`) 로 `AppEvent` 를 enqueue.
2. 위 메인 루프의 `app_event_rx.recv()` 분기가 그것을 `App::handle_event` (`app/event_dispatch.rs:12`) 로 디스패치.
3. 거기서 분기마다 `app_server.<rpc>(...).await` 가 일어나거나, `AppEvent::CodexOp(AppCommand)` / `AppEvent::SubmitThreadOp { thread_id, op }` 형태가 thread_routing 으로 넘어가 `app_server.submit_op(...)` (또는 thread 별 핸들) 를 통해 JSON-RPC 으로 나간다.
4. 자주 쓰는 outbound 는 `AppEventSender` 가 헬퍼로 감싼다: `interrupt()`, `compact()`, `exec_approval(thread_id, id, decision)`, `patch_approval(...)`, `request_permissions_response(...)`, `resolve_elicitation(...)` (`app_event_sender.rs:46-131`).

### 3.2 inbound — app-server → TUI

`AppServerSession::next_event()` (`app_server_session.rs:325-327`) 가 broadcast 채널에서 `AppServerEvent` 를 받아 `select!` 한 자리로 흘린다. 핸들러는 `App::handle_app_server_event` (`app/app_server_events.rs:30-58`) 이고 다음 4 종으로 갈라진다.

- `Lagged { skipped }` — 컨슈머 지연. MCP startup expected 목록을 config 기준으로 재동기화 후 `chat_widget.finish_mcp_startup_after_lag()` 로 lock-up 방지.
- `ServerNotification(...)` — `ServerRequestResolved` / `McpServerStatusUpdated` / `AccountRateLimitsUpdated` / `AccountUpdated` / `ExternalAgentConfigImportCompleted` 등을 `chat_widget` 의 도메인 메서드로 라우팅 (`app_server_events.rs:60+`).
- `ServerRequest(...)` — server-initiated request (예: exec approval, patch approval, MCP elicitation). 곧 4 절의 approval 흐름과 합류한다.
- `Disconnected { message }` — 화면에 에러를 띄우고 `AppEvent::FatalExitRequest` 로 본 루프에 종료 요청.

## 4. 화면 상태 머신

전역 상태 머신은 한 곳에 있지 않고 **두 층** 으로 나뉜다.

### 4.1 App 레벨 — overlay 게이트
`App::overlay: Option<Overlay>` (`app.rs` 상단 임포트, 사용처 `app.rs:1098-1100` / `app/input.rs`). overlay 가 `Some` 이면 키 / Draw / Resize 가 `handle_backtrack_overlay_event` 로 우회되고 평소의 `chat_widget` 경로는 막힌다. 트랜스크립트 페이저, backtrack, alt-screen 모드가 여기에 산다.

### 4.2 ChatWidget 레벨 — turn 단계
`ChatWidget` (`chatwidget.rs:744`) 이 turn 진행 단계를 다음 필드로 표현한다.

| 필드 | 의미 |
|---|---|
| `task_running: bool` (`chatwidget.rs:1172`) | model turn 또는 MCP startup 진행 중. `update_task_running_state()` (`:1706-1717`) 가 `bottom_pane.set_task_running(...)` 으로 바닥 영역의 spinner / 입력 잠금을 토글. |
| `agent_turn_running` | 모델 응답 수신 중인지. composing 과 awaiting model 을 구분. |
| `stream_controller: Option<StreamController>` (`:787`) | 활성 스트림이 있으면 `Some`. `streaming::controller` 가 token 청크를 받아 `active_cell` 에 incremental 적용. |
| `plan_stream_controller: Option<PlanStreamController>` (`:789`) | proposed plan 스트림은 별도 컨트롤러로 분리 (plan 화면이 끼면 흐름이 다르므로). |
| `bottom_pane.active_view` | composer / approval overlay / list selection / file search popup 등 **하위 모드** 가 여기서 결정된다. approval 이 enqueue 되면 `BottomPane` 이 composer 를 잠시 가리고 `ApprovalOverlay` 를 우선 표시. |

전이는 사실상 다음 순서로 일어난다.

```
composing  ──submit──▶  awaiting model  ──first delta──▶  streaming
                                              ▲                 │
                                              │                 ▼
                                        approval pending ◀── server request
                                              │
                                              └──── decision ──▶ awaiting model / streaming 으로 복귀
                                                                또는 abort 시 composing
```

각 전이의 트리거는 모두 `AppServerEvent` (`ServerNotification` / `ServerRequest`) 또는 사용자의 `KeyEvent` 다. 명시적인 `enum State` 필드는 없고 위 boolean / `Option<...>` 들의 **곱** 으로 상태가 정의된다는 점에 주의.

## 5. Approval 입력 흐름

서버가 `ExecApprovalRequest` / `ApplyPatchApprovalRequest` / `RequestPermissions` / `McpElicitation` 을 보내면 — `ServerRequest` 로 들어와 `chat_widget.on_exec_approval_request(...)` (`chatwidget.rs:3429`) / `on_apply_patch_approval_request(...)` (`:3437`) 로 라우팅된다. 이 함수들은 사용자 입력을 받기 전에 큐에 쌓을지, 즉시 처리할지 정한다 (`push_exec_approval` vs `handle_exec_approval_now`, `:4506-4541`). 실제 모달은 `ApprovalRequest` enum (`bottom_pane::ApprovalRequest`) 으로 변환되어 `BottomPane::push_approval_request(request, &features)` 로 전달.

화면 단에서는 `ApprovalOverlay` (`bottom_pane/approval_overlay.rs`) 가 보여진다. `BottomPaneView::handle_key_event` (`approval_overlay.rs:551-559`) 에서:

1. `try_handle_shortcut(key)` 로 각 옵션의 단축키 (`y` / `n` / `a` 등 — 옵션마다 `shortcuts: Vec<KeyBinding>` 가 등록되어 있다, `:540-547`) 와 매칭되면 `apply_selection(idx)` 를 즉시 호출.
2. 미매칭이면 `ListSelectionView::handle_key_event` 로 위/아래 화살표 + Enter 흐름을 거친다.

`apply_selection(actual_idx)` (`approval_overlay.rs:300-345`) 는 현재 `ApprovalRequest` 의 종류와 선택된 `ApprovalDecision` 을 매칭해 다음 메서드로 갈라진다.

- `Exec` → `handle_exec_decision(id, command, decision)` (`:347-367`)
   `app_event_tx.exec_approval(thread_id, id, decision)` 호출 → `AppEventSender::exec_approval` (`app_event_sender.rs:82-92`) 가 `AppEvent::SubmitThreadOp { op: AppCommand::exec_approval(id, /*turn_id*/ None, decision) }` 으로 enqueue → 메인 루프 → app-server 로 RPC. 동시에 thread label 이 없으면 user actor 로 history cell (`new_approval_decision_cell`) 을 추가해 즉시 화면에 반영.
- `ApplyPatch` → `handle_patch_decision` → `app_event_tx.patch_approval(...)`
- `Permissions` → `handle_permissions_decision` → `request_permissions_response(...)`
- `McpElicitation` → `handle_elicitation_decision` → `resolve_elicitation(...)`

선택 후 `current_complete = true`, `advance_queue()` 가 다음 큐를 꺼내거나 overlay 를 닫는다. Ctrl-C 는 `BottomPaneView::on_ctrl_c` (`:561-564`) 가 `cancel_current_request()` 로 받아 같은 outbound 채널에 `Abort` 결정을 흘린다.

요약하면 “y 누름” 의 콜 트리는

```
KeyEvent('y')
  → TuiEventStream::map_crossterm_event   (tui/event_stream.rs)
  → App::handle_tui_event                 (app.rs:1082)
  → ChatWidget::handle_key (overlay 비어 있을 때)
  → BottomPane → ApprovalOverlay::handle_key_event
  → try_handle_shortcut(&KeyEvent) 매칭
  → apply_selection(idx)
  → handle_exec_decision/handle_patch_decision/...
  → AppEventSender::exec_approval / patch_approval / ...
  → AppEvent::SubmitThreadOp { thread_id, op }
  → App::handle_event (event_dispatch.rs)
  → app_server.submit_op(...)  → JSON-RPC out
```

이고, 결정 결과 (실행 / 패치 적용 / 거부) 는 다음 `ServerNotification` (예: `ExecCommandBegin`, `PatchApplyBegin`) 으로 다시 inbound 되어 `chat_widget` 의 active cell 에 반영된다.

## 6. Async / runtime 패턴

- 메인 tokio runtime 은 `run_main` 가 spawn 한 단일 multi-thread runtime. **메인 루프는 그 안의 한 task** 일 뿐 별도 스레드가 아니다. 따라서 `select!` 의 모든 분기가 같은 task 에서 직렬 처리된다 → 잠금 충돌이 거의 없다.
- 백그라운드 태스크는 명확히 분리된다.
  - `FrameScheduler::run` (`tui/frame_requester.rs:96-127`): 자기 자신을 `tokio::spawn` 하고 deadline coalescing 을 담당. 메인 루프는 broadcast subscriber 로만 동작.
  - `AppServerClient` 내부의 transport task (in-proc 모드에서는 app-server 전체를 spawn).
  - 파일 검색, 클립보드 paste burst tick, plan stream tick 등 자잘한 timer 들은 `frame_requester.schedule_frame_in(dur)` 으로 다시 메인 루프에 합류시킨다.
- **블로킹 작업** (외부 에디터 실행, vim spawn) 은 `Tui::with_restored` (`tui.rs:472-504`) 가 `pause_events()` 로 stdin 을 놓고 `mode.restore()` → `f().await` → `set_modes()` → `flush_terminal_input_buffer()` → `resume_events()` 순서로 처리한다. 이 절차는 crossterm `EventStream` 의 stdin steal 이슈 (`event_stream.rs:9-18` 주석) 에 대한 회피책이다.

## 7. 렌더 효율성

“항상 60 fps 로 그린다” 가 아니라 **요청 기반 + 레이트 리밋** 모델이다.

1. 어디서든 그리기가 필요하면 `frame_requester.schedule_frame()` 또는 `schedule_frame_in(dur)` 을 호출. 호출자는 본인이 그릴 필요 없이 “언젠가 그려달라” 라는 신호만 보낸다 (`frame_requester.rs:48-56`).
2. `FrameScheduler` 가 들어온 deadline 들을 `next_deadline = min(...)` 으로 합치고 (`:104-117`), `FrameRateLimiter` (`tui/frame_rate_limiter.rs`) 가 마지막 emit 시각 + `MIN_FRAME_INTERVAL` (≈8.3 ms, 120 fps) 미만이면 deadline 을 뒤로 클램프.
3. deadline 도달 시 `draw_tx.send(())` broadcast → `TuiEventStream::poll_draw_event` 가 `TuiEvent::Draw` 를 메인 루프에 흘림.
4. `App::handle_tui_event` 의 `TuiEvent::Draw | Resize` 분기 (`app.rs:1113-1153`) 가 실제로 `tui.draw(...)` / `tui.draw_with_resize_reflow(...)` 를 호출. 그리기 직전 `chat_widget.pre_draw_tick()` 으로 streaming 청크를 한 번 더 commit 시킨다.
5. **Dirty 영역 추적은 ratatui 가 내부적으로 한다**. ratatui `Terminal::draw` 는 새 buffer 와 직전 buffer 를 diff 해 변경된 cell 만 stdout 으로 내보낸다. TUI 본체는 “전체 화면을 매번 rebuild” 하는 코드처럼 보이지만 실제 wire 출력은 부분 업데이트다.
6. 추가로 `terminal_resize_reflow_enabled` 일 때는 `handle_draw_pre_render(tui)` (`app.rs:1090`) 가 wrap policy 를 미리 다시 계산해 resize 직후의 한 프레임 깜빡임을 줄인다.

## 8. 테스트 가능성

`chatwidget` 은 메인 루프 / 터미널 / app-server 를 모두 우회해 **단위 테스트** 가 가능하도록 설계돼 있다. `codex-rs/tui/src/chatwidget/tests/` 안의 파일들 (`composer_submission.rs`, `exec_flow.rs`, `approval_requests.rs`, `permissions.rs`, `plan_mode.rs`, `popups_and_settings.rs` 등) 이 같은 패턴을 공유한다.

핵심 fixture (`chatwidget/tests/popups_and_settings.rs:71-92` 또는 `plan_mode.rs:1539-1560`):

```rust
let init = ChatWidgetInit {
    config: cfg.clone(),
    frame_requester: FrameRequester::test_dummy(),       // no-op scheduler
    app_event_tx: AppEventSender::new(unbounded_channel::<AppEvent>().0),
    workspace_command_runner: None,
    initial_user_message: None,
    enhanced_keys_supported: false,
    has_chatgpt_account: false,
    model_catalog: test_model_catalog(&cfg),
    feedback: codex_feedback::CodexFeedback::new(),
    is_first_run: true,
    ...
};
let chat = ChatWidget::new_with_app_event(init);
```

- `FrameRequester::test_dummy()` (`frame_requester.rs:60-67`) 는 mpsc 의 receiver 를 즉시 드롭해 그리기 신호를 silently 삼킨다.
- `app_event_tx` 는 `unbounded_channel` 을 직접 만들어 같은 함수 안에서 `rx` 를 보유. 테스트 본문은 위젯 메서드를 호출한 뒤 `drain_app_events(&mut rx)` (`tests/goal_validation.rs:36`) 로 발생한 `AppEvent` 를 모아 `assert_eq!` 로 검증한다 — 즉 “이 키를 누르면 정확히 이 outbound 시퀀스가 나온다” 를 보장.
- `helpers.rs::test_config()` 는 임시 `codex_home` 을 만들어 host config 가 새지 않도록 한다 (`tests/helpers.rs:5-24`).

이 패턴 덕분에 approval 큐, plan streaming, slash command, status surface 등 거의 모든 동적 동작이 터미널 없이 빠르게 테스트된다. UI 변형은 추가로 `insta` 스냅샷 (`chatwidget/tests/snapshots/`) 으로 잠그는 게 컨벤션이고, `cargo insta accept -p codex-tui` 로 받아들인다 (`CLAUDE.md`).

## 9. 정리

- 정적 트리는 `13-tui-structure.md` 가 책임지고, 본 문서는 **시간 축**: 사용자 키 → `TuiEvent` → `AppEvent` 또는 `ChatWidget` 메서드 → app-server RPC → server event → 다시 위젯 상태 → `Draw` 신호 → ratatui diff → 터미널.
- 본 루프는 `App::run` 의 4-way `select!` 한 군데가 전부다. 다른 모든 “비동기” 처리는 채널과 `FrameRequester` 를 통해 결국 이 루프 한 곳으로 합류한다.
- approval 같은 server-initiated 흐름도 outbound 채널이 같은 라우팅을 거쳐 다시 일관된 시퀀스를 만든다.
- 테스트는 `ChatWidget` 단위에서 outbound `AppEvent` 만 검증하므로, 본 루프 변경은 통합 / e2e 영역으로 넘긴다.
