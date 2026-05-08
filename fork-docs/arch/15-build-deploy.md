# 15. Build & Deploy tooling

이 문서는 xtech fork 의 빌드 시스템, 릴리스 산출물 생성, macOS 배포 파이프라인을 정리한다. 일상 개발은 Cargo 로 충분하지만, 릴리스용 universal 바이너리 / `.pkg` / GitHub release 자산은 별도의 스크립트 레인을 거친다. 이 둘이 어디서 갈라지는지, 그리고 Bazel 이 왜 lockfile 을 따로 갖는지를 한 번에 본다.

상세 컨트리뷰터 룰은 repo root `AGENTS.md` 에 있고, 이 문서는 fork 가 추가한 배포 경로 (`fork-docs/scripts/build-macos-pkg.sh`, `dist/install.sh`) 와 그 주변의 함정을 다룬다.

## 1. Cargo + Bazel 공존

repo 안에 빌드 시스템이 두 개 살고 있다. 같은 Rust 소스를 본다는 점은 같지만 **lockfile 이 분리** 돼 있고 의존성 해석 경로도 다르다.

| 시스템 | lockfile | 진입점 | 언제 쓰나 |
| --- | --- | --- | --- |
| Cargo | `codex-rs/Cargo.lock` | `cargo` / `just` | 일상 개발, 단위/통합 테스트, 로컬 release 빌드 |
| Bazel | `MODULE.bazel.lock` (+ `MODULE.bazel`) | `bazel` / `just bazel-*` | CI, RBE (remote build execution), 공식 릴리스 |

원칙은 단순하다: **개발은 Cargo, 공식 릴리스는 Bazel**. 다만 fork 의 macOS 패키징 스크립트 (`fork-docs/scripts/build-macos-pkg.sh`) 는 의도적으로 Cargo 만 쓴다 — Apple SDK 를 hermetic 하게 들고오는 Bazel 셋업 없이도 손맥북에서 바로 `.pkg` 를 만들 수 있어야 하기 때문.

`Cargo.toml` 변경이 있으면 두 lockfile 이 어긋날 수 있다. 그래서 의존성을 건드린 PR 은:

```bash
just bazel-lock-update    # MODULE.bazel.lock 재생성
just bazel-lock-check     # CI 가 도는 검증과 동일
```

을 돌려 **둘 다 같은 커밋에** 넣어야 한다. `bazel-lock-check` 가 빠지면 CI 에서 `MODULE.bazel.lock out of date` 로 떨어진다.

`MODULE.bazel` 자체는 toolchain 까지 박제 (`@llvm//toolchain:all`, macOS SDK 26.4 archive 의 sha256) 돼 있어 Bazel 빌드는 시스템에 설치된 Xcode / clang 을 거의 안 본다. 이게 RBE 로 그대로 옮겨갈 수 있는 이유다.

## 2. justfile 핵심 레시피

`justfile` 은 repo root 에 있지만 첫 줄이 `set working-directory := "codex-rs"` 라서 모든 레시피가 `codex-rs/` 안에서 실행된다. 자주 쓰는 7 개:

| 레시피 | 실제 명령 | 메모 |
| --- | --- | --- |
| `just codex [args]` (`just c`) | `cargo run --bin codex -- "$@"` | 로컬에서 fork CLI 를 바로 실행. cargo bin 이름은 여전히 `codex` |
| `just exec [args]` | `cargo run --bin codex -- exec "$@"` | headless 1-shot. fork 의 chat-completions 경로 디버깅에 자주 씀 |
| `just fmt` | `cargo fmt -- --config imports_granularity=Item` | Rust 편집 후 자동 (사용자 허가 없이 돌림) |
| `just fix [-p crate]` | `cargo clippy --fix --tests --allow-dirty` | 1 크레이트 범위 권장. 공유 크레이트 손댔을 때만 unscoped |
| `just test` | `RUST_MIN_STACK=8MiB cargo nextest run --no-fail-fast` | **느림. 사용자 허락 받고 돌릴 것** |
| `just bazel-codex [args]` | `bazel run //codex-rs/cli:codex --run_under="cd $PWD &&"` | Bazel 경로로 같은 binary 실행 (재현용) |
| `just bazel-lock-update` | `bazel mod deps --lockfile_mode=update` | `Cargo.toml` 변경 후 짝꿍 |

부수적으로 자주 등장하는 것들:

- `just write-config-schema` — `ConfigToml` 변경 시 `codex-rs/core/config.schema.json` 재생성
- `just write-app-server-schema` — app-server v2 API 모양이 바뀌었을 때
- `just argument-comment-lint` — Bazel 백엔드 dylint (CI 도 동일)
- `just build-for-release` — `bazel build //codex-rs/cli:release_binaries --config=remote` (공식 릴리스 산출물)

`just test` 는 nextest 가 깔려 있어야 한다 (`cargo install cargo-nextest`). lockfile 이 커서 cargo 명령이 30 초 ~ 분 단위로 걸릴 수 있는데, **PID 로 죽이지 말 것** — 락 파일이 깨지면 lockfile 재생성이 필요해진다.

## 3. Release profile 의 트레이드오프

`codex-rs/Cargo.toml` 의 `[profile.release]` 는 사이즈 최적화 쪽에 강하게 기울어져 있다:

```toml
[profile.release]
lto = "fat"
split-debuginfo = "off"
strip = "symbols"
codegen-units = 1
```

의미:

- `lto = "fat"` — 모든 크레이트를 한 LTO unit 으로 합쳐 cross-crate 인라이닝. 산출물은 작고 빠르지만 **링크 단계가 단일 스레드로 수십 초 ~ 분** 걸린다.
- `codegen-units = 1` — 같은 이유. 병렬화 포기 / 인라이닝 극대화. 첫 release 빌드가 “멈춘 듯” 보이는 주범.
- `strip = "symbols"` — 심볼 제거. backtrace 가 깨지므로 디버깅용 빌드는 release 를 피하든지 따로 build profile 을 추가.

**우회: LTO 끄기.** 로컬에서 release 빠르게 한 번 보고 싶을 때:

```bash
CARGO_PROFILE_RELEASE_LTO=off cargo build --release --bin codex
```

`CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16` 까지 같이 주면 더 빨라진다. 단, 이렇게 만든 바이너리는 사이즈/성능 면에서 공식 릴리스와 다르므로 **배포용으로 쓰지 말 것**. `fork-docs/scripts/build-macos-pkg.sh` 는 이 env 를 의도적으로 안 깐다.

## 4. Build artifact 위치와 이름

cargo 가 만드는 release 바이너리 경로:

```
codex-rs/target/<triple>/release/codex
```

triple 예: `aarch64-apple-darwin`, `x86_64-apple-darwin`. **cargo bin 이름은 `codex` 로 고정** — fork 도 이 이름을 안 바꿨다 (workspace member `cli` 의 `[[bin]] name = "codex"`).

“`xtech` 라는 이름” 은 빌드 산출물이 아니라 **install 시점에 rename** 된다. 패키징 스크립트가 staged 디렉터리에 복사하면서 `usr/local/bin/xtech` 로 이름을 바꾼다 (`build-macos-pkg.sh:58`):

```bash
INSTALLED_BIN="${STAGE_ROOT}/usr/local/bin/${CMD_NAME}"   # CMD_NAME=xtech
```

이 분리 덕에 **개발 워크플로 (`just codex …`, `cargo run --bin codex`) 는 그대로 두고**, 사용자에게는 `xtech` 명령으로 노출된다. 다른 사내 fork 와 충돌하지 않는다.

`CODEX_CMD_NAME` env 로 install 명령 이름을 더 바꿀 수도 있다 (`build-macos-pkg.sh:37`). pkg identifier 는 `com.kimtaehan.${CMD_NAME}` 로 자동 결정.

## 5. Cross-compile + Universal binary

`build-macos-pkg.sh` 는 세 가지 모드를 지원한다 (env 로 선택):

| 모드 | 환경변수 | 결과 |
| --- | --- | --- |
| host (기본) | (없음) | `uname -m` 그대로. M-시리즈면 `aarch64-apple-darwin` 만 |
| 강제 arch | `CODEX_PKG_TARGET_ARCH=x86_64` | Intel cross-build |
| universal | `CODEX_PKG_UNIVERSAL=1` | arm64 + x86_64 둘 다 빌드 후 `lipo` |

universal 경로의 핵심 (`build-macos-pkg.sh:60-65`):

```bash
ARM_BIN="$(build_for aarch64-apple-darwin)"
X86_BIN="$(build_for x86_64-apple-darwin)"
lipo -create -output "${INSTALLED_BIN}" "${ARM_BIN}" "${X86_BIN}"
ARCH_TAG="universal"
```

`build_for` 안에서 `rustup target add <triple>` 을 먼저 호출하므로 cross target toolchain 이 자동으로 설치된다. `lipo -create` 가 두 마하-O 를 fat binary 로 묶는다 — 단일 파일이 양쪽 아키텍처에서 그대로 실행됨.

universal 한 번이면 `dist/install.sh` 가 arm64 / x86_64 양쪽을 같은 자산 (`xtech-universal.tar.gz`) 으로 처리하므로 GitHub release 자산을 줄일 수 있다.

## 6. macOS 패키징 — `.pkg` + tarball 동시 생성

`fork-docs/scripts/build-macos-pkg.sh` 는 한 번 실행으로 두 가지 산출물을 만든다:

1. **`.pkg`** — `pkgbuild` 로 만드는 macOS Installer 패키지. GUI 더블클릭 / `installer -pkg` 둘 다 가능.
2. **tarball** — `xtech-<arch>.tar.gz` (stable name) + `xtech-<version>-<arch>.tar.gz` (versioned). curl|bash 설치 (`install.sh`) 에서 사용.

플로우:

1. `mktemp -d` 로 stage root 만들고 `usr/local/bin/${CMD_NAME}` 에 바이너리 복사 (또는 lipo).
2. `pkgbuild --root … --identifier com.kimtaehan.xtech --install-location / --scripts <…>/macos-pkg-scripts <out>.pkg`.
3. (옵션) `productsign` → `notarytool submit --wait` → `stapler staple`.
4. 같은 stage 의 `usr/local/bin/${CMD_NAME}` 만 골라 `tar -czf` 로 tarball 두 개 (versioned + stable) 생성.

**postinstall — 설정 템플릿 drop.** `pkgbuild --scripts` 로 묶이는 `fork-docs/scripts/macos-pkg-scripts/postinstall` 이 root 권한으로 돌면서:

- `/dev/console` 의 stat 으로 실제 로그인 사용자를 추출 (root 의 `$HOME` 이 아니므로).
- `~/.config/xtech/xtech.json` 이 없을 때만 `{ baseURL, apiKey, model }` 빈 템플릿을 떨어뜨림.
- `chown <user>` + `chmod 600` 으로 권한 정리.

이미 파일이 있으면 건드리지 않는다 — 재설치로 사용자의 토큰이 날아가지 않도록.

## 7. GitHub release 흐름

배포 경로는 일부러 단순하게 잡았다:

1. 로컬에서 universal pkg + tarball 생성:
   ```bash
   CODEX_PKG_UNIVERSAL=1 ./fork-docs/scripts/build-macos-pkg.sh
   ```
2. `dist/` 안에 산출물 (`xtech-<sha>-universal.pkg`, `xtech-universal.tar.gz`, `xtech-<sha>-universal.tar.gz`) 이 떨어진다.
3. `gh release create vX.Y.Z dist/xtech-universal.tar.gz dist/xtech-<sha>-universal.pkg --title "…" --notes-file …` 로 업로드.
4. 사용자는 `dist/install.sh` 의 한 줄 실행:
   ```bash
   curl -fsSL https://raw.githubusercontent.com/kim-taehan/xtech/main/dist/install.sh | bash
   ```

`install.sh` 는 release tag 를 모르더라도 동작하도록 **`releases/latest/download/`** 경로를 사용한다 (`dist/install.sh` 안의 `PKG_URL` 분기). GitHub 가 `latest` alias 를 따라 자동으로 최신 release 자산을 서빙해 준다. 특정 버전 핀은 `XTECH_VERSION=v0.1.0`.

`install.sh` 가 하는 일을 한 줄로: tarball 받아서 풀고 `/usr/local/bin/xtech` 로 install, 그리고 postinstall 과 같은 `~/.config/xtech/xtech.json` 템플릿을 떨어뜨림 (curl|bash 경로는 sudo 없이 user 권한으로 처리).

## 8. Notarize / 공증

코드 서명은 **선택적** 이다. fork 는 두 모드를 다 지원한다:

| 모드 | 트리거 | 사용자 측 추가 작업 |
| --- | --- | --- |
| 무서명 | env 미설정 | `xattr -d com.apple.quarantine <pkg>` 로 Gatekeeper 우회. 사내 / 같은 네트워크 한정 권장 |
| 서명 + 공증 | `CODEX_PKG_SIGN_IDENTITY` + `CODEX_PKG_NOTARY_PROFILE` | 없음 — 일반 macOS 더블클릭 설치 |

서명/공증 절차 (`build-macos-pkg.sh:98-110`):

```bash
productsign --sign "${CODEX_PKG_SIGN_IDENTITY}" "${UNSIGNED_PKG}" "${FINAL_PKG}"
xcrun notarytool submit "${FINAL_PKG}" --keychain-profile "${CODEX_PKG_NOTARY_PROFILE}" --wait
xcrun stapler staple "${FINAL_PKG}"
```

- `CODEX_PKG_SIGN_IDENTITY` — 키체인의 `Developer ID Installer: Foo Bar (TEAMID)` 정확히. 코드 서명 (`Developer ID Application`) 과 다른 cert 다. `pkgbuild` 산출물은 installer 서명을 요구.
- `CODEX_PKG_NOTARY_PROFILE` — `xcrun notarytool store-credentials <name>` 으로 사전에 저장한 keychain profile 이름. App Store Connect API key 또는 Apple ID + app-specific password 로 만든다.
- `--wait` 가 공증 끝까지 블록. 보통 1-3 분.
- `stapler staple` 이 ticket 을 `.pkg` 파일에 박아서 오프라인 설치도 Gatekeeper 통과.

서명은 했는데 notary profile 이 없으면 스크립트가 **서명만 된 상태로 남기고** 경고를 찍는다 (`!! notary profile unset — skipping notarization`).

## 9. Bazel 데이터 의존 — `include_str!` 함정

Bazel 은 Cargo 와 달리 **소스 트리 파일을 컴파일 시점 file API 에 자동 노출하지 않는다**. 즉 다음 셋 중 하나라도 추가하면 cargo 는 통과해도 Bazel CI 가 깨진다:

- `include_str!("…")`, `include_bytes!("…")`
- `sqlx::migrate!("./migrations")` 같은 매크로
- 런타임에 fixture 를 여는 통합 테스트

수정 위치는 해당 크레이트의 `BUILD.bazel`. 세 가지 attribute 가 있다:

| attribute | 용도 |
| --- | --- |
| `compile_data` | 컴파일러가 매크로 시점에 보는 파일 (`include_str!` 등) |
| `build_script_data` | `build.rs` 가 읽는 파일 |
| `data` / `test_data_extra` | 런타임 (특히 통합 테스트) 에서 여는 파일 |

`codex-rs/core/BUILD.bazel` 이 좋은 예 — 거의 모든 `**` 를 `compile_data` 에 박아 놓고 (`tools/`, `templates/`, `prompts/` …) `BUILD.bazel` 과 `Cargo.toml` 만 제외.

```python
codex_rust_crate(
    name = "core",
    compile_data = glob(
        include = ["**"],
        exclude = ["**/* *", "BUILD.bazel", "Cargo.toml"],
        allow_empty = True,
    ),
    rustc_env = { "CARGO_MANIFEST_DIR": "codex-rs/core" },
    integration_compile_data_extra = [
        "//codex-rs/apply-patch:apply_patch_tool_instructions.md",
        "templates/realtime/backend_prompt.md",
    ],
    …
)
```

핵심 룰 두 개:

1. **`include_str!` 를 추가했으면 같은 PR 에서 BUILD.bazel 도 손볼 것.** Cargo CI 는 통과하지만 Bazel CI / `just bazel-test` 에서 “file not found” 로 떨어진다.
2. **fixture 위치 찾을 때 `env!("CARGO_MANIFEST_DIR")` 직접 쓰지 말 것.** Bazel 의 RBE sandbox 에서 manifest dir 이 의미가 달라진다. `codex_utils_cargo_bin::find_resource!` 또는 `cargo_bin::cargo_bin("…")` 을 써야 runfiles 와 cargo target/ 양쪽에서 동작한다.

cross-system 함정의 흔한 발현: 테스트가 로컬에서 `cargo test` 로는 통과하는데 RBE 에서 빨갛게 뜬다 → 거의 항상 위 두 룰 위반이다.
