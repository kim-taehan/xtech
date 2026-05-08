# 16. Sandboxing layer

이 문서는 xtech fork 의 샌드박싱 계층을 정리한다. 모델이 `shell` 툴을 호출했을 때 자식 프로세스가 어떤 OS 메커니즘으로 격리되고, 어떤 경로/네트워크가 허용되며, escalation 이 어떻게 일어나는지를 한 번에 짚는다. fork 자체에서 새로 추가한 격리 메커니즘은 없고, upstream 의 구조를 그대로 이어받는다 — 차이는 (1) bwrap 을 fork 가 binary 로 bundle 한다는 점과 (2) `~/.xtech` 가 사용자 홈 트리로 잡혀 있어 writable root 계산 결과가 달라진다는 점 정도다.

## 1. 샌드박스 모드 enum

`SandboxMode` 는 사용자가 `config.toml` / CLI 에서 고르는 high-level 모드다. 정의는 `codex-rs/protocol/src/config_types.rs:68`:

```rust
pub enum SandboxMode {
    #[default]
    ReadOnly,        // "read-only"
    WorkspaceWrite,  // "workspace-write"
    DangerFullAccess,// "danger-full-access"
}
```

세 가지 뿐이다. 사용자의 요청에 있던 `Untrusted` 는 샌드박스 모드가 **아니라** `HookTrustStatus` 의 variant (`codex-rs/protocol/src/protocol.rs:1536`) 이며 hook 신뢰도 표시용이다. 샌드박스 도메인에는 들어오지 않는다.

`SandboxMode` 는 정책 enforcement 의 입력일 뿐이고, 실제 실행 시점에는 더 세분화된 두 값으로 분해된다 — 둘 다 `codex-rs/protocol/src/protocol.rs` 에 있다:

- `SandboxPolicy` (`:1006`) — 레거시/호환용 형태. variant: `DangerFullAccess`, `ReadOnly { network_access }`, `ExternalSandbox { network_access }`, `WorkspaceWrite { writable_roots, network_access, exclude_tmpdir_env_var, exclude_slash_tmp }`.
- `PermissionProfile` — 신규 표현. 파일시스템/네트워크 권한을 entry 단위로 표현하고 `to_runtime_permissions()` 로 `(FileSystemSandboxPolicy, NetworkSandboxPolicy)` 한 쌍을 만든다.

런타임은 `PermissionProfile` 을 정본으로 쓰고, 외부 노출용 wire protocol (app-server v2, exec output) 에만 `SandboxPolicy` 를 다시 합성한다 — `compatibility_sandbox_policy_for_permission_profile` (`codex-rs/sandboxing/src/manager.rs:263`) 가 그 변환을 한다.

플랫폼 백엔드 선택은 별도 enum `SandboxType` (`codex-rs/sandboxing/src/manager.rs:23`):

```rust
pub enum SandboxType { None, MacosSeatbelt, LinuxSeccomp, WindowsRestrictedToken }
```

`get_platform_sandbox(windows_sandbox_enabled)` 가 `cfg!(target_os)` 와 Windows 플래그 하나를 보고 사용 가능한 backend 를 결정한다.

## 2. OS 별 메커니즘

### macOS — Seatbelt (`sandbox-exec`)

크레이트: `codex-rs/sandboxing/` (cfg `target_os = "macos"`).

핵심 파일은 `src/seatbelt.rs` 와 `src/seatbelt_base_policy.sbpl` (closed-by-default `(deny default)` 정책에 sysctl-read / process-info / file-read* / unix-socket 등을 화이트리스트로 추가). 네트워크는 별도 `seatbelt_network_policy.sbpl` 이 켜질 때만 합쳐진다. `MACOS_PATH_TO_SEATBELT_EXECUTABLE = "/usr/bin/sandbox-exec"` 를 hard-code 해서 PATH 주입 공격을 방지한다.

호출 형태: `manager.transform()` 이 base policy + 동적 allow 절(writable_roots, proxy 포트, allowlisted unix sockets) 을 문자열로 합성한 뒤 `["/usr/bin/sandbox-exec", "-p", policy_text, "--", original_argv...]` 를 만든다 (`manager.rs:196-213`).

### Linux — Landlock + seccomp + Bubblewrap

크레이트: `codex-rs/linux-sandbox/` (helper 바이너리 `codex-linux-sandbox`) + `codex-rs/sandboxing/` (정책 builder).

흐름은 두 단계다:

1. 메인 codex 프로세스가 `manager.transform()` 으로 helper CLI 인자를 만든다 — `codex-rs/sandboxing/src/landlock.rs::create_linux_sandbox_command_args_for_permission_profile` 가 `--sandbox-policy-cwd`, `--command-cwd`, `--permission-profile <json>`, 옵션 플래그 (`--use-legacy-landlock`, `--allow-network-for-proxy`), `--`, 그리고 원래 argv 를 이어 붙인다.
2. `codex-linux-sandbox` 가 그 인자를 파싱하고 (`linux-sandbox/src/linux_run_main.rs`) :
   - `landlock.rs::apply_permission_profile_to_current_thread` 가 `prctl(PR_SET_NO_NEW_PRIVS)` + seccomp BPF 로 네트워크 syscall 을 차단한다 (`socket(AF_INET/AF_INET6/...)` 를 막거나 proxy-routed 모드에서 loopback 만 통과). 파일시스템은 기본적으로 bubblewrap 이 담당하고, `--use-legacy-landlock` 일 때만 in-process Landlock ruleset 을 깐다.
   - 중요한 디테일 하나: `PR_SET_NO_NEW_PRIVS` 는 setuid binary 의 권한 상승을 막기 때문에 setuid 형 bwrap 과 충돌한다. 그래서 `apply_permission_profile_to_current_thread` 는 “seccomp 가 정말 필요한가? legacy Landlock 이 정말 필요한가?” 를 둘 다 보고, 둘 중 하나라도 true 일 때만 NNP 를 켠다 (`linux-sandbox/src/landlock.rs:60-65`). 결과적으로 “read-only + 네트워크 enabled” 같은 무해한 조합에서는 NNP 가 꺼진 채 bwrap 만으로 격리한다.
   - `launcher::exec_bwrap` 이 bubblewrap 을 `execve` 한다 — 시스템 `bwrap` 을 `find_system_bwrap_in_path` 로 먼저 찾고, 실패하면 fork 가 bundle 한 in-tree 바이너리로 fallback (다음 절). 시스템 bwrap 을 쓸 때는 `--help` 출력을 파싱해 `--argv0` 와 `perms` extension 을 모두 지원하는지 capability probe 한다 (`launcher.rs:108`) — 둘 다 만족하지 않으면 시스템 binary 를 거부하고 bundled 로 떨어진다.

### Linux 부수 — Bubblewrap (`bwrap` 크레이트)

`codex-rs/bwrap/` 는 `vendor/bubblewrap` 의 C 소스를 컴파일해서 fork 자신의 `bwrap` 바이너리를 빌드한다 (`build.rs` → `bubblewrap.c`, `bind-mount.c`, `network.c`, `utils.c` 를 `cc` crate 로 빌드). cfg flag `bwrap_available` 로 결과를 노출하고, `main.rs` 가 단순히 `bwrap_main(argc, argv)` C 심볼을 호출하는 thin shell 이다.

이 결과물은 `codex-resources/bwrap` 위치로 release artifact 와 함께 dotted/배포되며, `linux-sandbox/src/bundled_bwrap.rs` 가 codex 실행 파일 옆에서 찾아 사용한다. 시스템 bwrap 이 너무 오래되어 `--argv0` (v0.9.0 이후) 가 없거나 setuid 가 깨진 환경에서도 fork 가 직접 컴파일한 binary 로 동작이 보장된다 — fork 의 air-gap / 사내망 배포 시나리오와 직접 연결된다.

### Windows — `windows-sandbox-rs`

크레이트: `codex-rs/windows-sandbox-rs/` (Windows 에서만 컴파일). 다음 메커니즘을 조합한다:

- restricted token (`token.rs`) + integrity level 강등.
- DACL/ACL 로 워크스페이스 외 파일 deny-write (`acl.rs`, `workspace_acl.rs`).
- WFP (Windows Filtering Platform, `wfp.rs` / `wfp_setup.rs`) 로 outbound 필터.
- private desktop 격리 옵션 (`desktop.rs`).
- elevated helper IPC (`elevated/`, `runner_pipe.rs`, `runner_client.rs`) — 권한 상승이 필요한 setup 작업을 별도 elevated process 로 위임.

Windows 백엔드는 `WindowsSandboxLevel::Disabled` 가 기본이라 명시적으로 켜야 활성화된다 (`get_platform_sandbox` 의 분기).

## 3. 샌드박스 정책 생성 — `WorkspaceWrite` 의 writable roots

핵심 함수는 `codex-rs/protocol/src/protocol.rs:1179` 의 `SandboxPolicy::get_writable_roots_with_cwd(cwd) -> Vec<WritableRoot>`. `WorkspaceWrite` variant 일 때 다음을 모은다:

1. 사용자가 `writable_roots` 에 명시한 경로 (config 의 `[sandbox_workspace_write].writable_roots`).
2. 항상: `cwd` (현재 워크스페이스 루트).
3. UNIX 에서: `/tmp` — `exclude_slash_tmp = false` 이고 실제로 디렉터리일 때만.
4. `$TMPDIR` — `exclude_tmpdir_env_var = false` 이고 환경변수가 비어있지 않을 때 (macOS 는 per-user TMPDIR, Linux/Windows 는 정의돼 있을 때만).

각 root 안에는 `default_read_only_subpaths_for_writable_root` 가 만든 read-only sub-path 가 같이 붙는다 — `.git/hooks`, `.codex/`, `.git/config` 등 escalation 에 쓰일 수 있는 메타데이터 파일이 자동 deny 된다.

`~/.xtech/memories/` 같은 fork 고유 경로는 **이 기본 set 에는 들어가지 않는다**. memories 는 codex 프로세스 본체가 직접 읽고 쓰는 자원이고, 모델이 spawn 한 자식 프로세스가 거기에 write 해야 하는 시나리오는 없다. 워크스페이스에서 memory 를 만지려면 사용자가 명시적으로 `writable_roots` 에 추가하거나 별도 메모리 툴 (memories 크레이트의 RPC) 을 통하게 되어 있다.

`PermissionProfile` 측 entry-기반 표현은 더 세밀하다 — `FileSystemSandboxEntry { path, access }` 를 항목별로 allow/deny 하고, `FileSystemSpecialPath::{Root, ProjectRoots, Tmpdir, SlashTmp, Minimal}` 같은 sentinel 로 cwd 의존 경로를 추상화한다 (`codex-rs/sandboxing/src/policy_transforms.rs:372`). `effective_permission_profile` 이 베이스 + tool 호출이 들고 온 `AdditionalPermissionProfile` 을 머지해 최종 enforcement 셋을 만든다.

Seatbelt 측 변환은 `build_seatbelt_access_policy` (`sandboxing/src/seatbelt.rs:336`) 가 담당한다. 각 writable root 를 `(param "WRITABLE_ROOT_N")` placeholder 로 만들고, 그 값을 sandbox-exec 의 `-D` 매개변수로 넘기는 식이다 — 즉 SBPL 텍스트는 정적이고, root 경로/socket 경로 같은 동적 값은 모두 named param 으로 주입된다. 보호해야 할 sub-path (예: `<root>/.git/hooks`) 는 같은 파라미터화 로직을 거쳐 `(require-not (subpath (param "...EXCLUDED_M")))` 로 deny 절이 합성된다. 추가로 root 안에서 절대 만들어지면 안 되는 metadata 이름 (`protected_metadata_names`) 은 정규식 형태로 박힌다.

## 4. 셸 명령 실행 흐름

요약 흐름 (`codex-rs/core/src/tools/orchestrator.rs:219` 부근, `codex-rs/core/src/exec.rs:140 / :360` 가 entry):

1. 모델이 `shell` (또는 `local_shell`, `apply_patch`) 툴 호출 → `tools/orchestrator.rs` 가 `ToolRuntime::run(req, attempt, ctx)` 로 dispatch.
2. `SandboxAttempt` (`tools/sandboxing.rs:373`) 가 현재 turn 의 `permission_profile`, `sandbox: SandboxType`, `sandbox_cwd`, `codex_linux_sandbox_exe` 등을 들고 있다.
3. 툴이 raw `SandboxCommand { program, args, cwd, env, additional_permissions }` 를 만들고 `attempt.env_for(...)` 에 넘긴다.
4. 그 안에서 `SandboxManager::transform(SandboxTransformRequest { … })` (`sandboxing/src/manager.rs:168`) 가:
   - `effective_permission_profile` 로 base + additional 머지.
   - `SandboxType` 분기로 backend wrapper 를 합성:
     - `MacosSeatbelt` → `/usr/bin/sandbox-exec -p <policy> -- ...`.
     - `LinuxSeccomp` → `<codex_linux_sandbox_exe> --sandbox-policy-cwd <cwd> --permission-profile <json> [--allow-network-for-proxy] -- ...`.
     - `WindowsRestrictedToken` → argv 그대로 (격리는 `spawn_child` 에서 token 으로 적용).
     - `None` → no-op.
   - 결과 `SandboxExecRequest { command, cwd, env, sandbox, … }` 를 반환.
5. `ExecRequest::from_sandbox_exec_request` (`core/src/sandboxing/mod.rs:111`) 가 환경변수를 마지막으로 보강:
   - `network_sandbox_policy` 가 비활성이면 `CODEX_SANDBOX_NETWORK_DISABLED=1`.
   - macOS + Seatbelt 면 `CODEX_SANDBOX=seatbelt`.
6. `execute_exec_request` → `exec()` (`core/src/exec.rs:915`) → `spawn_child_async` (`core/src/spawn.rs:51`) 가 실제로 자식 프로세스 spawn. unix 에서 `pre_exec` 으로 TTY detach + `prctl(PR_SET_PDEATHSIG)` (Linux) 로 부모-자식 lifecycle 을 묶는다. `cmd.env_clear()` 후 화이트리스트된 환경변수만 다시 주입하므로 부모 프로세스의 `LD_*` / `DYLD_*` / 비밀 토큰 등이 자식으로 그대로 흐르지 않는다 (`core/src/spawn.rs:75-80`).

`SandboxablePreference` enum (`manager.rs:42`) 이 자동 선택을 미세조정한다:

| pref | 동작 |
| --- | --- |
| `Auto` | `should_require_platform_sandbox` 가 true 일 때만 OS 백엔드 적용. 즉 “안전한 read-only 정책 + 네트워크 enabled” 처럼 격리가 무의미한 경우에는 `None` 으로 떨어진다. |
| `Require` | 무조건 platform sandbox 사용. 사용 불가능한 플랫폼이면 `None` 으로 fallback (조용히 약화 — 호출자가 별도로 검증해야 한다는 뜻). |
| `Forbid` | `None` 고정. 디버그/테스트 시나리오용. |

`should_require_platform_sandbox` (`policy_transforms.rs:509`) 의 의미는 “사용자가 지정한 정책을 OS 가 강제해 줘야 의미가 있는가” 이다 — 네트워크가 막혀 있거나, 파일시스템이 `Restricted` 이고 full disk write 가 없으면 platform sandbox 가 필요하다고 판정.

## 5. 샌드박스 환경변수

자식 프로세스가 자기 격리 상태를 자기 자신이 인지할 수 있도록 두 변수가 정의돼 있다 — `codex-rs/core/src/spawn.rs:20`:

| 변수 | 값 | 의미 |
| --- | --- | --- |
| `CODEX_SANDBOX_NETWORK_DISABLED` | `"1"` | shell tool 호출이고 네트워크가 막혔을 때 set. |
| `CODEX_SANDBOX` | `"seatbelt"` | 현재 macOS Seatbelt 만 발급. 향후 다른 backend 도 자체 토큰을 추가할 수 있게 열어 둠. |

쓰임새는 두 가지다 — (a) 자식 안에서 도는 ollama/lmstudio 등 클라이언트 코드가 “네트워크 못 쓰니까 미리 abort” 처럼 빠른 실패 (`codex-rs/lmstudio/src/client.rs:213` 외 다수), (b) 통합 테스트가 “지금 우리는 sandbox 안이라 진짜로 sandbox-exec 을 또 spawn 할 수 없다” 를 인식하고 skip — `codex-rs/core/tests/suite/exec.rs:19` 가 표준 패턴.

## 6. 네트워크 차단

세 layer 가 모두 같은 방향을 가리킨다:

1. **정책 단계** — `NetworkSandboxPolicy::Restricted` 가 `permission_profile` 에 박힌다 (default).
2. **OS enforcement** — Seatbelt 에서는 base policy 에 `(deny network*)` 만 있고 network policy 텍스트가 합쳐지지 않는다. Linux 에서는 `apply_permission_profile_to_current_thread` 가 `socket()` syscall 을 차단하는 seccomp BPF 를 install (`linux-sandbox/src/landlock.rs::install_network_seccomp_filter_on_current_thread`). Windows 에서는 WFP 필터가 outbound 를 막는다.
3. **Hint 단계** — `CODEX_SANDBOX_NETWORK_DISABLED=1` 가 자식 env 에 들어가서 OS-level deny 가 발동하기 전에 application-level 에서 fail-fast 할 수 있게 한다.

“managed network” (loopback 으로 라우트되는 사내 proxy) 모드에서는 `allow_network_for_proxy = true` 가 되고, Seatbelt 는 loopback 포트만 화이트리스트에 추가, Linux 는 `NetworkSeccompMode::ProxyRouted` BPF 가 `127.0.0.1` 만 통과시키는 변종을 install 한다. 구체적으로 `dynamic_network_policy_for_network` (`sandboxing/src/seatbelt.rs:258`) 가:

1. `proxy.ports` 에 들어 있는 loopback 포트 각각에 대해 `(allow network-outbound (remote ip "localhost:<port>"))` 를 생성.
2. `allow_local_binding` 이 켜져 있으면 `*:*` 바인딩 + `localhost:*` inbound/outbound 를 추가하고, DNS 위해 `:53` outbound 도 열어 둠.
3. unix socket allowlist 가 있으면 `system-socket (socket-domain AF_UNIX)` + path-scoped `network-bind/network-outbound` 를 합성.
4. 위 동적 절 + 정적 `MACOS_SEATBELT_NETWORK_POLICY` (loopback DNS, certain darwin notification ports 등) 을 이어 붙여 base policy 에 append.

proxy 가 설정돼 있는데 실제 endpoint 를 추출하지 못하면 빈 문자열을 돌려 fail-closed — 이게 “네트워크가 부분적으로 열려서 secret 이 새는” 사고를 막는 가드.

## 7. Bypass / Escalation — `shell-escalation`

크레이트: `codex-rs/shell-escalation/` (Unix only). 사용자가 turn 마다 “이 한 번만 sandbox 밖에서 실행하게 해 줘” 를 승인할 때 그 메커니즘이다.

핵심 아이디어 (`src/unix/mod.rs` 머리말의 ASCII 다이어그램):

- Codex 메인 프로세스가 `EscalateServer` 를 띄우고 unix-domain socketpair 의 한쪽 fd 를 자식 shell 에 그대로 상속시킨다. fd 번호는 `CODEX_ESCALATE_SOCKET` 환경변수로 알려준다 (`escalate_protocol.rs:11`).
- shell 은 patch 된 zsh (`patches/zsh-exec-wrapper.patch`) 가 모든 `exec()` 직전에 `EXEC_WRAPPER` (= `codex-execve-wrapper` 바이너리) 를 호출하도록 후킹된다.
- `codex-execve-wrapper` 가 해당 socket 으로 `EscalateRequest { file, argv, workdir, env }` 를 보낸다.
- 서버가 `EscalationPolicy::determine_action` 을 돌려 결정:
  - `Run` → wrapper 가 자기 자신을 `execve` 해서 sandbox 안에서 그대로 실행.
  - `Escalate` → wrapper 가 stdin/stdout/stderr fd 를 서버로 SCM_RIGHTS 로 forward, 서버가 sandbox 밖에서 진짜로 실행 후 exit code 를 회신. 보통 사용자 approval prompt 가 여기서 뜬다.
  - `Deny` → wrapper 가 stderr 에 사유를 찍고 1 로 종료.

이 구조 덕분에 모델이 “sandbox 가 막을 명령 (예: `sudo apt install`, `git push`)” 을 임의로 spawn 해도 patch 된 shell 안에 머무는 한 모든 exec 가 codex 본체의 정책 게이트를 통과한다. sudo 자체가 special 처리되는 건 아니고, escalation 결정은 단지 `argv` 를 보고 사용자에게 묻거나 자동승인하는 일반 흐름이다.

`EscalationPolicy` 트레잇 (`escalation_policy.rs:7`) 은 한 메서드 — `determine_action(file, argv, workdir) -> EscalationDecision` — 만 갖고, core 쪽 구현체가 turn 의 approval mode 와 사용자 응답 채널에 매핑된다. 따라서 “정책” 자체는 sandbox 정책과 독립적으로 진화한다 — sandbox layer 가 OS 차원의 enforcement 라면, escalation layer 는 그 위에 얹는 사용자 동의 기반 우회 통로.

zsh patch (`patches/zsh-exec-wrapper.patch`) 는 `EXEC_WRAPPER` 환경변수가 set 돼 있을 때 `Src/exec.c` 의 `execve` 호출을 wrapper 로 우회시키는 작은 변경이다. fork 가 자체적으로 patched zsh binary 를 빌드해 ship 한다 — bwrap 과 같이 “시스템에 의존하지 않고 격리 환경에서 동작 보장” 을 위한 결정.

## 8. 보조 — `process-hardening`

`codex-rs/process-hardening/` 는 sandbox 그 자체는 아니지만 같은 보안 표면에 있다. `pre_main_hardening()` 이 `#[ctor::ctor]` 로 main 직전에 실행되어:

- Linux: `prctl(PR_SET_DUMPABLE, 0)` (ptrace attach 차단), `RLIMIT_CORE=0`, `LD_*` 환경변수 제거.
- macOS: `ptrace(PT_DENY_ATTACH)`, `RLIMIT_CORE=0`, `DYLD_*` / `MallocStackLogging*` 환경변수 제거.
- BSD: `RLIMIT_CORE=0` + `LD_*` 정리.

코어가 메모리 dump 를 통해 user secret/세션 token 을 흘리는 경로를 차단하는 것이 목적.

## 9. 테스트가 어렵다는 점 — 손대지 말 것

`AGENTS.md` (그리고 fork 의 `CLAUDE.md`) 가 명시적으로 경고한다: **`CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` 와 `CODEX_SANDBOX_ENV_VAR` 를 참조하는 코드를 새로 추가하거나 수정하지 말라**. 현재 reference 들은 모두 “이 테스트가 sandbox 안에서 도는 걸 감지하면 즉시 skip” 이라는 회피 로직이고, 잘못 만지면 :

- 네트워크가 막힌 sandbox 안에서 네트워크 의존 테스트를 강제로 돌리려다 hang/실패.
- Seatbelt 안에서 `sandbox-exec` 을 다시 부르는 nested 테스트가 OS 단에서 거부되어 알 수 없는 실패.

이런 이유로 통합 테스트 (`core/tests/suite/exec.rs`, `compact_resume_fork.rs`) 는 시작 시 `std::env::var(CODEX_SANDBOX_ENV_VAR) == Ok("seatbelt")` 같은 가드를 두고 빠져나간다. 새 테스트도 같은 패턴을 답습할 것 — fork 자체는 이 부분을 건드리지 않았다.

## 9.5. WSL1 예외

`SandboxTransformError::Wsl1UnsupportedForBubblewrap` (`manager.rs:110`) 이 잡히는 자리는 좁다 — Linux 이고 bubblewrap 이 필요하며 WSL1 환경일 때만. `is_wsl1()` (`sandboxing/src/bwrap.rs`) 가 `/proc/sys/kernel/osrelease` 를 검사해서 WSL1 인지 식별하고, 그 경우 사용자에게 `WSL1_BWRAP_WARNING` 메시지로 “WSL2 로 업그레이드 또는 sandbox off” 안내를 띄우고 transform 단계에서 즉시 중단. WSL1 은 syscall 매핑 한계로 unprivileged user namespace + bind mount 가 정상 동작하지 않기 때문이다.

## 10. fork 차이 정리

- **bwrap bundling**: upstream 도 동일 메커니즘이지만, 본 fork 는 release artifact 에 `bwrap` 을 묶어 air-gap 환경에서 시스템 패키지 의존성을 0 으로 만든 것이 운영상 차이.
- **`~/.xtech` 홈**: writable root 계산과 `default_read_only_subpaths_for_writable_root` 의 메타데이터 매칭이 `.codex/` 를 read-only 로 보호하지만, fork 는 사용자 데이터를 `~/.xtech/` 로 옮겼다 — 즉 워크스페이스 안에 우연히 `.codex/` 가 있어도 fork 는 그걸 “자기 설정 디렉터리” 로 인식하지 않는다 (보호는 그대로 유지된다, 단지 의미가 약해진다).
- 그 외 정책 enum, Seatbelt SBPL, Landlock/seccomp BPF, escalation 프로토콜은 upstream 과 동일.

## 11. 관련 파일 색인

- `codex-rs/protocol/src/config_types.rs` — `SandboxMode`, `WindowsSandboxLevel`.
- `codex-rs/protocol/src/protocol.rs` — `SandboxPolicy`, `WritableRoot`, writable-root 계산.
- `codex-rs/sandboxing/src/manager.rs` — `SandboxManager`, `SandboxType`, transform pipeline.
- `codex-rs/sandboxing/src/seatbelt.rs` + `seatbelt_*.sbpl` — macOS 정책 합성.
- `codex-rs/sandboxing/src/landlock.rs` — Linux helper CLI 인자 빌더.
- `codex-rs/sandboxing/src/policy_transforms.rs` — permission profile 머지 / intersect.
- `codex-rs/linux-sandbox/src/{linux_run_main,landlock,launcher,bundled_bwrap}.rs` — Linux helper 실체.
- `codex-rs/bwrap/{build.rs,src/main.rs}` — bundled bubblewrap 빌드.
- `codex-rs/windows-sandbox-rs/src/lib.rs` — Windows backend.
- `codex-rs/shell-escalation/src/unix/{escalate_protocol,escalate_server,escalate_client,execve_wrapper}.rs` — escalation IPC.
- `codex-rs/process-hardening/src/lib.rs` — pre-main hardening.
- `codex-rs/core/src/spawn.rs` — `CODEX_SANDBOX_*` env var 정의 + `spawn_child_async`.
- `codex-rs/core/src/sandboxing/mod.rs` — core 측 ExecRequest 어댑터.
- `codex-rs/core/src/tools/{sandboxing,orchestrator}.rs` — 툴 → 샌드박스 디스패치.
