# 13. TUI structure

이 문서는 `codex-rs/tui` 크레이트의 모듈 지도와 컴포넌트 위계를 정리한다. xtech fork 의 TUI 도 거의 100% upstream 코드를 그대로 쓰며, 브랜딩 문자열 4 군데만 손댔다 — 따라서 이 문서는 “fork 차이” 보다 “원본 구조를 어디서부터 읽으면 되는가” 에 무게를 둔다. 새 화면을 추가하거나 기존 화면을 손질해야 할 때의 진입점 지도로 쓴다.

기준 경로: `codex-rs/tui/src/`. 파일 인용은 모두 이 디렉터리 하위 상대경로다. 스타일 / 테스트 규약은 `codex-rs/tui/styles.md` 와 repo 루트 `AGENTS.md` 의 “TUI code conventions” 섹션이 단일 원천이다.

목차:

1. 모듈 맵 — `tui/src/` 에 무엇이 어디 있는가
2. 사이즈 경고 / hot 영역 — “더 키우지 말라” 파일들
3. 렌더링 트리 위계 — App → ChatWidget → HistoryCells + BottomPane
4. 이벤트 입력 — Key / Paste / Mouse 라우팅
5. 스타일 컨벤션 — `Stylize` 헬퍼와 금지 사항
6. Wrap / line utilities — 무엇을 언제 쓰는가
7. Snapshot 테스트 — insta 사용법
8. 새 화면을 추가하려면 — 모달 / overlay / cell 패턴
9. fork 가 손댄 곳 — xtech 브랜딩 4 군데

## 1. 모듈 맵

`tui/src/` 는 약 80 개 이상의 .rs 파일이 한 폴더에 평면적으로 깔려 있고, 그 위에 일부 디렉터리 모듈이 얹혀 있는 구조다. 핵심만 분류하면 다음과 같다.

런타임 / 진입점:
- `lib.rs` — 외부에서 호출되는 `run_main` 류 진입점. 모듈 선언 (`mod app; mod chatwidget; mod bottom_pane; ...`) 과 onboarding/login 흐름이 한 파일에 모여 있다.
- `bin/` — 실행 바이너리 셸 (`tui` 스탠드얼론).
- `cli.rs` — TUI 모드의 CLI 인자 정의.
- `tui.rs` + `tui/{event_stream,frame_rate_limiter,frame_requester,job_control,keyboard_modes}.rs` — 터미널 raw 모드, alt screen, 키보드 enhancement, paste 정규화, 프레임 요청 큐. crossterm 이벤트를 `TuiEvent { Key, Paste, Resize, Draw }` 로 정규화해서 위로 흘려준다.

상위 오케스트레이션:
- `app.rs` (1k+ LoC) — `App` 구조체. 전역 상태 (현재 chat widget, overlays, app server session, backtrack 상태) 를 보유하고 메인 이벤트 루프 (`handle_tui_event`) 를 실행한다.
- `app/` 서브모듈 — `event_dispatch.rs`, `input.rs`, `thread_routing.rs`, `app_server_events.rs`, `session_lifecycle.rs`, `resize_reflow.rs`, `startup_prompts.rs` 등. `app.rs` 가 비대해지지 않게 책임을 쪼개 둔 곳. 새 “앱-레벨” 로직은 여기에 추가하는 것이 디폴트.
- `app_event.rs`, `app_event_sender.rs`, `app_command.rs`, `app_backtrack.rs` — 내부 이벤트/커맨드 enum 과 backtrack(Esc 두 번 → 이전 사용자 입력 위치로 돌아가기) 상태머신.

채팅 위젯 (대화 화면):
- `chatwidget.rs` (11k+ LoC, **hot zone**) — 채팅 본체. `ChatWidget` 구조체가 history cells, active streaming cell, bottom pane, overlay 들을 모두 들고 있고 `Renderable` 을 구현한다. 모든 in-conversation 키 입력 / 페이스트 / 서버 이벤트가 결국 여기로 모인다.
- `chatwidget/` 서브모듈 — `goal_menu.rs`, `interrupts.rs`, `mcp_startup.rs`, `realtime.rs`, `session_header.rs`, `slash_dispatch.rs`, `skills.rs`, `user_messages.rs`, `tests.rs` 등 ~20 개. `chatwidget.rs` 자체가 이미 너무 크므로 새 기능은 무조건 이쪽 서브모듈에 추가한다.

History cells (transcript 단위):
- `history_cell.rs` (5.8k LoC) — `pub(crate) trait HistoryCell` 정의 (`raw_lines`, `lines(width)`, `transcript_animation_tick` 등) 와 `UserHistoryCell`, `AgentMarkdownCell`, `ReasoningSummaryCell`, `PlainHistoryCell`, `SessionHeaderHistoryCell`, `UnifiedExecInteractionCell`, `UpdateAvailableHistoryCell` 등 수십 개 구현체.
- `history_cell/hook_cell.rs` — hook 관련 cell 만 분리.
- 보조: `diff_render.rs`, `diff_model.rs`, `markdown.rs`, `markdown_render.rs`, `markdown_stream.rs`, `streaming/` (assistant 스트리밍 청크 누적 → 활성 cell 갱신).

Bottom pane (입력창 + 푸터 + 모달):
- `bottom_pane/mod.rs` (2.8k LoC, **hot zone**) — `BottomPane` 구조체. `handle_key_event` 가 현재 활성 view → composer 순으로 라우팅한다. 모달 view 스택 (approval, file search popup, custom prompt, hooks browser, skill popup, list selection, multi select picker, mcp elicitation form, status line setup, terminal title setup, ...) 을 관리.
- `bottom_pane/chat_composer.rs` (10k+ LoC, **hot zone**) + `chat_composer/` + `chat_composer_history.rs` + `paste_burst.rs` + `textarea.rs` — 텍스트 입력. paste-burst 분류기, 슬래시 명령 popup, 파일 검색 popup, 멘션 코덱, 입력 히스토리. 이 파일의 docstring + `docs/tui-chat-composer.md` 가 동작 원천.
- `bottom_pane/footer.rs` (2k LoC, **hot zone**) — 단축키 힌트 바, esc-to-backtrack 힌트, quit reminder, shortcut overlay.
- 모달 view 들: `approval_overlay.rs`, `command_popup.rs`, `custom_prompt_view.rs`, `feedback_view.rs`, `file_search_popup.rs`, `hooks_browser_view.rs`, `list_selection_view.rs`, `mcp_server_elicitation.rs`, `memories_settings_view.rs`, `multi_select_picker.rs`, `pending_thread_approvals.rs`, `skill_popup.rs`, `skills_toggle_view.rs`, `slash_commands.rs`, `status_line_setup.rs`, `status_surface_preview.rs`, `title_setup.rs`, `unified_exec_footer.rs`, `request_user_input/`. 새 모달 화면은 여기에 모듈 하나 추가하고 `BottomPaneView` 트레이트를 구현하는 것이 컨벤션.

Onboarding / 인증:
- `onboarding/mod.rs`, `onboarding/onboarding_screen.rs`, `onboarding/welcome.rs`, `onboarding/auth.rs`, `onboarding/auth/`, `onboarding/keys.rs`, `onboarding/trust_directory.rs` — 첫 실행 / 미인증 상태에서 보이는 wizard.

상태 카드 / 사이드 패널:
- `status/mod.rs`, `status/card.rs`, `status/account.rs`, `status/rate_limits.rs`, `status/format.rs`, `status/helpers.rs`, `status/tests.rs` — `/status` 슬래시 명령으로 띄우는 카드. 모델/계정/세션/rate limit 요약.

기타 보조:
- `render/{mod,renderable,line_utils,highlight}.rs` — `Renderable` 트레이트 (ChatWidget / BottomPane / 모든 view 가 구현하는 표준 인터페이스: `render`, `desired_height`, `cursor_pos`, `cursor_style`), `Insets`, `prefix_lines`, syntax highlight 어댑터.
- `wrapping.rs` — `word_wrap_line`, `word_wrap_lines`, `word_wrap_lines_borrowed`, `wrap_ranges`, `wrap_ranges_trim`, `RtOptions` (initial/subsequent indent), URL-aware wrapping 보조 (`line_contains_url_like`, `line_has_mixed_url_and_non_url_tokens`). 라인 wrap 의 단일 진입점.
- `markdown.rs`, `markdown_render.rs`, `markdown_stream.rs`, `markdown_render_tests.rs` — 마크다운 → ratatui Line 변환 + 스트리밍 누적.
- `live_wrap.rs`, `transcript_reflow.rs`, `resize_reflow_cap.rs` — 스트리밍 중 인라인 wrap (전송된 prefix 만큼만 잘라 보내고 나머지를 다음 청크와 이어 붙임), 터미널 resize 시 transcript 전체 재계산.
- `keymap.rs`, `key_hint.rs`, `keymap_setup*.rs` — 기본 키바인딩 정의 + `~/.codex/keybindings.json` 형태의 사용자 커스터마이즈 wizard.
- `slash_command.rs`, `bottom_pane/slash_commands.rs`, `chatwidget/slash_dispatch.rs` — 슬래시 명령 등록 / 디스패치. 새 슬래시 명령은 이 세 파일을 함께 손댄다.
- `notifications/`, `pager_overlay.rs`, `theme_picker.rs`, `model_catalog.rs`, `model_migration.rs`, `external_editor.rs`, `update_*.rs`, `resume_picker/`, `keymap_setup/`, `ide_context/`, `multi_agents.rs`, `goal_display.rs`, `branch_summary.rs`, `shimmer.rs` — 부속 기능들.
- `test_backend.rs`, `test_support.rs`, `snapshots/`, `chatwidget/snapshots/`, `bottom_pane/snapshots/`, `onboarding/snapshots/`, `status/snapshots/`, `render/snapshots/`, `chatwidget/tests/`, `app/tests/` — 테스트 인프라 + 베이스라인.

## 2. 사이즈 경고 / hot 영역

`AGENTS.md` (§ “Avoid large modules”) 와 `CLAUDE.md` 가 명시한 “더 키우지 말라” 파일은 다음과 같다 (현재 LoC):

| 파일 | 현재 LoC | 비고 |
| --- | --- | --- |
| `chatwidget.rs` | ~11,160 | 새 standalone 메서드 추가 금지 — `chatwidget/` 서브모듈로 |
| `bottom_pane/chat_composer.rs` | ~10,460 | paste-burst / Enter 처리 변경 시 `docs/tui-chat-composer.md` 동기화 필수 |
| `history_cell.rs` | ~5,880 | trait 정의 + 구현체 다수. 새 cell 종류는 별도 파일로 |
| `bottom_pane/mod.rs` | ~2,810 | 모달 view 라우팅 허브 |
| `bottom_pane/footer.rs` | ~2,020 | 힌트 바 — 새 hint set 은 새 함수가 아닌 props 확장으로 |
| `app.rs` | ~1,170 | 이미 `app/` 서브모듈로 분리 진행 중 |

워크스페이스 공통 사이즈 타겟은 “테스트 제외 500 LoC, 800 LoC 이상이면 새 모듈로 분리” 다. 위 파일들은 이미 다 한참 넘었으므로 **새 기능을 추가할 때는 곁가지 모듈 신설이 디폴트**. 기존 파일을 손대는 건 근처에 동일 책임 코드가 있고 inline 이 명백히 더 읽힐 때만.

`chatwidget.rs` 는 “orchestration 만” 하라는 추가 규칙이 있다 — handler 본체는 서브모듈 함수로 빼고 `chatwidget.rs` 는 거기로 dispatch 하는 얇은 호출만 둔다. 실제로 `chatwidget/` 하위에는 `goal_menu.rs`, `interrupts.rs`, `mcp_startup.rs`, `realtime.rs`, `session_header.rs`, `slash_dispatch.rs`, `skills.rs`, `user_messages.rs`, `goal_status.rs`, `goal_validation.rs`, `hooks.rs`, `keymap_picker.rs`, `plan_implementation.rs`, `plugins.rs`, `reasoning_shortcuts.rs`, `status_surfaces.rs`, `side.rs`, `ide_context.rs` 가 이미 분리돼 있다. PR 에서 `chatwidget.rs` 자체에 새 메서드를 추가하면 리뷰어가 “이건 어느 서브모듈에 속하느냐” 를 묻는다고 보면 된다.

`chat_composer.rs` 의 동작 / 가정 (Enter 핸들링, retro-capture, flush/clear 규칙, `disable_paste_burst`, 비-ASCII / IME 처리) 가 바뀌면 `bottom_pane/AGENTS.md` 가 명시적으로 요구하듯 모듈 docstring 과 narrative 문서 (`docs/tui-chat-composer.md`) 를 같은 PR 에서 동기화해야 한다. 코드만 바꾸고 문서를 안 맞추면 다음 사람이 읽었을 때 docstring 이 거짓말이 된다.

## 3. 렌더링 트리 위계

```
Tui (raw 터미널, alt-screen, frame loop)
 └─ App                              app.rs
     ├─ Overlay (선택, e.g. transcript pager / backtrack)   pager_overlay.rs
     └─ ChatWidget                   chatwidget.rs
         ├─ HistoryCells (transcript)                       history_cell.rs
         │    └─ raw_lines / lines(width) → Vec<Line<'static>>
         ├─ Active streaming cell (in-flight)               streaming/
         └─ BottomPane                bottom_pane/mod.rs
              ├─ Active modal view (option) — approval, popup, picker, …
              ├─ ChatComposer         bottom_pane/chat_composer.rs
              │    ├─ Textarea
              │    ├─ Slash command popup
              │    ├─ File search popup
              │    └─ Mention/paste indicators
              └─ Footer               bottom_pane/footer.rs
```

각 레벨의 책임:

- **`Tui`** — crossterm 라이프사이클, alt screen 토글, focus 추적, frame rate limit, paste 가드. UI 의미 지식 없음.
- **`App`** — `Tui` 가 흘려준 `TuiEvent` 를 받아 overlay 가 활성이면 overlay 로, 아니면 `ChatWidget` 으로 라우팅. App-server 세션, 모델/auth 상태 같은 “세션 외 글로벌” 상태는 여기.
- **`ChatWidget`** — 한 thread (대화 세션) 의 모든 것. history cell 시퀀스, active cell, bottom pane, slash dispatch, server event 핸들러. `Renderable` 을 구현해서 area 를 받으면 `BottomPane` 의 desired height 를 빼고 위쪽에 history, 아래쪽에 BottomPane 을 그린다.
- **`HistoryCell`** — 단일 transcript 항목 (사용자 메시지 / 어시스턴트 메시지 / 추론 요약 / exec 결과 / diff / 세션 헤더 / hook 알림 / …). 폭이 주어지면 `lines(width)` 로 wrap 된 라인을 돌려준다. 일부 cell 은 시간에 따라 변하므로 (`activity_indicator`, motion) `transcript_animation_tick()` 으로 갱신을 알린다.
- **`BottomPane`** — 입력창 영역 전체. 활성 modal view 가 있으면 그것이 그려지고 (composer 는 가려짐), 없으면 `ChatComposer` + `Footer` 를 세로로 쌓는다. 키 입력 라우팅도 동일한 우선순위.
- **`ChatComposer`** — 텍스트 입력 + paste-burst 분류 + 슬래시/멘션/파일 popup. Enter 시 `InputResult::Submitted(text)` 를 위로 돌려준다.
- **`Footer`** — 한 줄 ~ 몇 줄짜리 단축키 힌트. 상태 의존적 (스트리밍 중 / 승인 대기 중 / esc-backtrack 가능 / quit reminder 표시 / shortcut overlay 펼쳐짐 등). 새 hint 카테고리를 추가하려면 `footer.rs` 의 props 구조에 필드를 더하고 `footer_from_props_lines` 분기를 늘리는 식으로 — 새 함수를 따로 export 하지 않는다.

이 위계의 핵심 invariant 는 “키 입력은 위에서 아래로, 결과는 아래에서 위로” 다. 상위가 하위로 키 이벤트를 push 하고, 하위는 `InputResult` / `BottomPaneView::completion` / `AppEvent` 같은 “결과 객체” 를 위로 return 한다. 콜백 / observer 패턴은 의도적으로 쓰지 않는다 — 디버깅이 어려워지기 때문.

## 4. 이벤트 입력 (Key / Mouse / Paste)

전체 이벤트 루프 / 채널 다중화의 시간 축은 `14-tui-event-loop.md` 가 따로 다룬다 — 본 절은 “정적 라우팅 트리” 만, 즉 한 입력이 어느 함수에서 어느 함수로 전달되는지의 코드 경로만 정리한다. 흐름은 한 방향이다.

1. `tui::Tui::event_stream()` 이 crossterm `EventStream` 을 폴링하고, focus 변화, bracketed paste, keyboard enhancement 프로토콜 응답을 안에서 흡수한다. 남은 사용자 입력만 `TuiEvent::Key(KeyEvent)` 또는 `TuiEvent::Paste(String)` 로 변환되어 위로 올라간다 (`tui.rs:354`). 동시에 `frame_rate_limiter.rs` / `frame_requester.rs` 가 “draw 요청” 을 별도 채널로 만들어 `TuiEvent::Draw` 로 합류시킨다 — 그래서 한 `select!` 루프 안에서 입력과 리페인트가 한 enum 으로 통일된다.
2. `App::handle_tui_event` (`app.rs:1082`) 가 첫 라우터다. overlay (예: transcript pager, backtrack 모드) 가 살아있으면 `handle_backtrack_overlay_event` 로 가로채고, 그렇지 않으면:
   - `TuiEvent::Key(k)` → `App::handle_key_event` (전역 단축키 — Ctrl+C 두 번 종료, Ctrl+T 트랜스크립트, `/` 슬래시 등) 거친 뒤 미소비 이벤트는 `ChatWidget::handle_key_event` (`chatwidget.rs:5081`) 로 전달.
   - `TuiEvent::Paste(s)` → `\r` 을 `\n` 으로 정규화 후 `ChatWidget::handle_paste` (`chatwidget.rs:5472`) 로. 이 정규화는 iTerm2 등이 newline 을 CR 로 보내지만 `tui-textarea` 는 LF 만 인식한다는 부조화를 보정하기 위함이다.
   - `TuiEvent::Draw|Resize` → `pre_draw_tick` → `desired_height` 계산 → `tui.draw()` 콜백에서 `ChatWidget::render`. resize-reflow 가 켜져 있으면 `tui.draw_with_resize_reflow` 가 별도 경로로 transcript 를 다시 흘린다 (`app/resize_reflow.rs`, `transcript_reflow.rs`).
3. `ChatWidget` 안에서는 “활성 모달 / overlay → BottomPane → composer” 순서로 키를 흘려본다. `BottomPane::handle_key_event` (`bottom_pane/mod.rs:550`) 가 동일한 패턴을 한 번 더 적용한다 — 활성 view 가 있으면 거기로, 없으면 composer 로. 활성 view 가 “완료” 되면 `pop_active_view_with_completion` 으로 스택에서 빠지고 결과는 `BottomPaneView::completion()` 으로 상위에 보고된다.
4. Esc 의 의미는 컨텍스트 가변이다 — 모달 활성: 모달 dismiss; 모달 없음 + 작업 진행 중: `Op::Interrupt` 송신 (`bottom_pane/mod.rs:606-619`); 모달도 작업도 없음: 두 번 누르면 `app_backtrack` 이 백트랙 모드 진입. 따라서 Esc 핸들링을 새로 추가할 때는 이 우선순위 사다리에 끼워 넣어야 하고, “전역 Esc 동작 추가” 처럼 한 곳만 고치는 식의 변경은 거의 항상 잘못 된다.
5. 마우스는 별도 라우팅 채널이 없다 — crossterm 마우스 이벤트는 alt-screen 모드에서 부분적으로 받고, scroll wheel 정도만 `App` 의 `event_stream` 어댑터에서 transcript pager 키로 변환된다. 본 fork 는 마우스 라우팅을 추가하지 않았다.
6. paste 는 두 경로다. 터미널이 bracketed paste 를 지원하면 `Tui` 가 한 덩어리 `TuiEvent::Paste(String)` 로 묶어 올린다. 지원하지 않거나 IME / 비-ASCII 문자가 끼면 `KeyEvent` 가 빠르게 연속으로 오는데, `bottom_pane/paste_burst.rs` 가 시간 간격 / 길이 휴리스틱으로 “이건 사람 타자가 아니라 페이스트” 라고 판단해 composer 내부에서 다시 한 덩어리로 합친다. 이 분류기 동작이 바뀌면 `chat_composer.rs` 의 모듈 doc + `docs/tui-chat-composer.md` 동기화가 필수 (`bottom_pane/AGENTS.md` 명시).

## 5. 스타일 컨벤션

`tui/styles.md` 가 짧고 단정적이다. 핵심:

- **Headers** 는 `bold`, **secondary** 는 `dim`, primary 는 default.
- **색**: cyan = 사용자 입력 / 선택 / status, green = 성공 / 추가, red = 에러 / 삭제, magenta = Codex 자체.
- **금지**: `.white()`, `.black()`, `.blue()`, `.yellow()`, 임의의 RGB. 기본 전경색이 사용자 테마와 맞물려 더 잘 보이므로 “색을 빼는” 게 디폴트.
- **`Stylize` 헬퍼만 쓴다**: `"text".dim()`, `"text".cyan().underlined()`, `"text".bold()`, `"text".into()`.
- **`Span::styled` / 수동 `Style` 은 컴파일타임 상수 스타일에는 금지** — 런타임에 계산된 `Style` 일 때만 허용 (`Span::from(text).set_style(style)` 도 OK).
- **동등한 두 형식 사이의 churn 금지**: `Line::from(vec![…])` ↔ `vec![…].into()`, `Span::styled` ↔ `set_style` 은 가독성 / 기능 이득이 명확할 때만 바꾼다.
- 한 줄에 들어가게 rustfmt 후 더 짧게 유지되는 형식을 고른다.

clippy.toml 에 색 관련 일부 룰이 박혀 있어 위반 시 lint 가 잡는다.

## 6. Wrap / line utilities

용도별로 도구가 갈린다 — 섞어 쓰지 않는다.

- **plain `&str` 한 덩어리를 wrap**: `textwrap::wrap` 직접 호출. fork/upstream 둘 다 `textwrap` crate 를 의존성으로 가진다.
- **`ratatui::text::Line` 한 줄을 wrap**: `wrapping::word_wrap_line(line, width_or_options)`. 들여쓰기가 필요하면 옵션으로 `RtOptions { initial_indent, subsequent_indent }` 를 넘긴다 — 직접 인덴트 prefix 를 붙이는 코드를 새로 짜지 않는다.
- **`Vec<Line>` 을 한꺼번에 wrap**: `wrapping::word_wrap_lines(iter, width_or_options)` (소유 라인 반환) 또는 `word_wrap_lines_borrowed`.
- **이미 wrap 된 라인들에 prefix 를 붙임**: `render::line_utils::prefix_lines(lines, first, rest)` — 첫 줄과 이후 줄에 다른 prefix 를 줄 수 있다. 직접 `lines.iter().map(|l| ...prepend...)` 로 짜지 않는다.
- **range 단위 wrap (커서 위치 보존 등)**: `wrapping::wrap_ranges` / `wrap_ranges_trim`. composer / live wrap 에서 사용.
- **streaming 도중의 부분 wrap**: `live_wrap.rs::take_prefix_by_width` 가 이미 출력된 만큼만 잘라서 다음 청크와 이어 붙인다.

## 7. Snapshot 테스트 (insta)

TUI 는 거의 모든 visible 변경에 insta snapshot 추가/갱신을 요구한다 (`AGENTS.md` 명시 — “any change that affects user-visible UI MUST include corresponding insta snapshot coverage”). 절차:

1. 변경 후 `cargo test -p codex-tui` — 실패하면 `*.snap.new` 가 해당 모듈의 `snapshots/` 에 생성된다 (`tui/src/snapshots/`, `tui/src/chatwidget/snapshots/`, `tui/src/bottom_pane/snapshots/`, `tui/src/onboarding/snapshots/`, `tui/src/status/snapshots/`, `tui/src/render/snapshots/`).
2. `cargo insta pending-snapshots -p codex-tui` 로 대기 중 목록 확인.
3. `cargo insta show -p codex-tui <path/to/file.snap.new>` 로 diff 미리보기. 또는 `*.snap.new` 를 에디터에서 직접 열어 본다 — 텍스트 diff 라 가독성 좋음.
4. **본인이 의도한 snapshot 만** 받아들여야 하므로 무작정 `accept` 보다 한 파일씩 `cargo insta accept -p codex-tui --snapshot <name>` 를 쓰거나, 의도가 명확한 경우에만 일괄 `cargo insta accept -p codex-tui`.
5. 도구가 없으면 `cargo install cargo-insta`.

테스트 어서션은 `pretty_assertions::assert_eq` 로 통일, 객체 단위 비교 선호. 프로세스 환경 변경 금지 (의존성을 위에서 주입). 테스트용 가짜 backend 는 `test_backend.rs` (in-memory ratatui buffer), 공통 헬퍼는 `test_support.rs` 에 모여 있다. 새 widget 의 snapshot 을 짤 때는 거기 있는 `render_snapshot(...)` 류 헬퍼를 재사용 — 직접 `Buffer::empty(...)` 부터 짜지 않는다.

## 8. 새 화면을 추가하려면

전형적 패턴 — “모달 + 입력 + 결과 콜백” 형태일 때:

1. `bottom_pane/<my_view>.rs` 를 신설하고 `BottomPaneView` 트레이트를 구현. `Renderable::render`, `handle_key_event`, `is_complete`, `completion`, `is_dismissible`, `prefer_esc_to_handle_key_event`, `on_ctrl_c` 등 필요한 것을 채운다. 기존 view (`approval_overlay.rs`, `list_selection_view.rs`, `multi_select_picker.rs`, `feedback_view.rs`) 가 가장 좋은 참고 예시.
2. 결과 전달은 두 가지 옵션. (a) view 가 끝날 때 `BottomPaneView::completion()` 으로 enum value 를 반환하고 호출자 (`ChatWidget` 또는 `App`) 가 그 값을 보고 후속 처리. (b) 비동기 / cross-component 라면 `app_event::AppEvent` 에 새 variant 를 추가하고 `AppEventSender` 를 view 가 들고 있다가 push.
3. `bottom_pane/mod.rs` 의 `BottomPane` 에 새 view 를 띄우는 `show_<view>(...)` 메서드를 추가하고, `push_view(Box<dyn BottomPaneView>)` 로 스택에 올린다. 키 라우팅은 자동 — `handle_key_event` 가 항상 스택 top 으로 흘려보낸다.
4. 호출자는 슬래시 명령 (`bottom_pane/slash_commands.rs` + `chatwidget/slash_dispatch.rs`) 또는 키 단축키 (`keymap.rs`) 에서 진입. 슬래시 명령 추가 시 `slash_command.rs` enum 에도 등록.
5. snapshot 테스트 (`bottom_pane/snapshots/`) 를 적어도 “초기 상태” / “입력 후 한 번” 두 개는 추가. `test_backend.rs` 의 헬퍼로 가짜 `Buffer` 를 만들어 `view.render(area, &mut buf)` 후 `insta::assert_snapshot!(snapshot_buffer(&buf))` 패턴.

전체 화면 (overlay) 일 때는 `pager_overlay.rs` 의 패턴을 따라 `App::overlay: Option<Overlay>` 에 새 variant 를 추가하고 `handle_backtrack_overlay_event` 와 비슷한 dispatcher 를 둔다. overlay 는 `BottomPane` 보다 위에서 그려지므로 `ChatWidget` 의 render 를 가로챈다.

대화 transcript 안에 새 종류 cell 을 띄울 때는 `history_cell.rs` 또는 `history_cell/<my>_cell.rs` 에 새 struct + `impl HistoryCell` 을 추가하고 (`raw_lines`, `lines(width)` 최소 두 개는 채워야 함), `chatwidget.rs::handle_thread_item` (또는 그 dispatcher 가 부르는 `chatwidget/` 서브모듈 함수, 예: `chatwidget/user_messages.rs`) 에서 서버 이벤트 → 새 cell 인스턴스 매핑을 추가한다. 시간 의존적 표시 (스피너, 경과 시간) 면 `transcript_animation_tick()` 도 구현 — 그래야 transcript pager 캐시가 무효화된다.

“화면 전체를 새로 짜는 wizard” (예: 신규 onboarding step) 면 `onboarding/` 의 패턴을 따른다. `OnboardingScreen` 이 step state machine 을 들고 있고 각 step 은 `WidgetRef` + key handler 인 단순 구조다.

## 9. fork 가 손댄 곳

xtech fork 가 TUI 에서 변경한 부분은 **브랜딩 문자열 4 군데뿐**이다. 동작 / 위계 / 키바인딩 / 레이아웃 / 이벤트 루프 / 모달 / 스타일 컨벤션 — 모두 upstream 그대로 둔다. 의도적으로 좁게 가져가는 이유는, 이 fork 가 “upstream 의 자체 코드 베이스 분기” 가 아니라 “게이트웨이 라우팅 + 라벨 변경” 만 얹은 얇은 fork 로 유지돼야 향후 rebase 비용이 낮기 때문이다.

| 파일 | 줄 (대략) | 변경 |
| --- | --- | --- |
| `codex-rs/tui/src/history_cell.rs` | 1640, 1710 | `SessionHeaderHistoryCell` 의 박스 안 타이틀 (`">_ OpenAI Codex (vX)"`) 과 `raw_lines()` 양쪽에서 `OpenAI Codex` → `xTech code` |
| `codex-rs/tui/src/onboarding/welcome.rs` | 96-98 | `WelcomeWidget` 의 인사 문구를 `Welcome to Codex, OpenAI's command-line coding agent` → `Welcome to xTech code, a forked Codex variant routed to the internal Qwen gateway` 로. 두 번째 절 (“…routed to the internal Qwen gateway”) 가 사용자가 fork 임을 인지하는 첫 번째 단서 |
| `codex-rs/tui/src/status/card.rs` | 649 | `/status` 카드 헤더 `">_ OpenAI Codex (vX)"` → `">_ xTech code (vX)"`. `history_cell.rs` 의 세션 헤더와 표기를 맞춰 두 화면 간 라벨 불일치를 방지 |
| `codex-rs/exec/src/event_processor_with_human_output.rs` | 222 | `codex exec` 헤드라인 배너 `OpenAI Codex v… (research preview)` → `xTech code v… (research preview)`. TUI 가 아닌 exec 측이지만 사용자가 보는 “이름” 의 일관성을 맞추는 짝궁 변경 |

이 4 곳은 단일 커밋 (`f8c6d4fa1a fork: rename to xtech, route LLM calls to internal Qwen gateway`) 에 묶여 있고, 중간에 `kdex` / `Kdex` 같은 임시 이름이 거쳐간 흔적은 없다 — 한 번에 `xTech code` 로 통일됐다 (`grep -r kdex codex-rs/` 가 빈 결과).

추가로 손대지 **않은** 것:
- 슬래시 명령 이름 (`/status`, `/feedback`, `/skills` 등) — upstream 그대로.
- config 키 (`codex.*`), env var (`CODEX_*`) — 둘 다 upstream 명. `xtech` 라는 식별자는 사용자 노출 표면에서만 보이고 내부 식별자에는 침범하지 않는다.
- `~/.xtech` 디렉터리는 `codex-rs/utils/home-dir/` 단위에서 결정되며 TUI 는 단순히 `find_codex_home` 의 결과를 받아 쓴다 — TUI 안에는 “xtech” 라는 문자열이 박혀 있지 않다.
- snapshot baseline (`snapshots/*.snap`, `chatwidget/snapshots/*.snap`, `status/snapshots/*.snap`, `onboarding/snapshots/*.snap`) — 갱신되지 않았다. 위 4 군데 변경이 들어간 cell/widget 의 기존 snapshot 은 `OpenAI Codex` 를 기대하므로 `cargo test -p codex-tui` 가 실패한다. 본 fork 의 후속 작업으로 `cargo insta accept -p codex-tui` 의 의도된 부분만 받아들이는 정리가 필요하다 — 자세한 처리 계획은 `fork-docs/work-log-2026-05-08.md` 참조.

브랜딩 문자열을 더 추가/수정할 때는 위 4 곳 + `exec` 배너 + (필요 시) `lib.rs` 의 startup 메시지를 같은 PR 에서 함께 바꿔 “보이는 이름이 화면마다 다른” 상황을 방지한다. 그리고 변경의 동반 작업으로 snapshot 갱신은 반드시 같이 — 그래야 `just test` 로 회귀 검출이 살아 있다.

다른 fork 차이 (LLM 호출이 사내 Qwen 게이트웨이로 라우팅되는 부분, `~/.xtech` 디렉터리 디폴트, install/airgap 스크립트) 는 TUI 가 아닌 다른 크레이트의 책임이고 각각 `01-overview.md`, `06-config.md`, `airgap-audit-2026-05-08.md`, `ollama-migration.md` 에서 다룬다. 이 문서는 “xtech fork 에서 TUI 만 봤을 때 무엇이 달라지는가” 의 결정판으로 — 사실상 `OpenAI Codex` 라는 문자열 4 개가 답이다.
