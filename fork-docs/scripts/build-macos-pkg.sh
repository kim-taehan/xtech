#!/usr/bin/env bash
# Build a macOS installer package (.pkg) for the fork's `codex` binary.
#
# Two modes (decided automatically by env vars):
#
#   1) Unsigned local build — for handing the .pkg to a teammate on the same
#      laptop / same network. Recipient will need:
#        sudo installer -pkg codex-<ver>-<arch>.pkg -target /
#        # if Gatekeeper complains:
#        xattr -d com.apple.quarantine codex-<ver>-<arch>.pkg
#
#   2) Signed + notarized — set the env vars below and the script signs the
#      .pkg with your Developer ID Installer cert and submits to Apple's
#      notary service. Stapled output ships outside your network.
#
#        export CODEX_PKG_SIGN_IDENTITY="Developer ID Installer: Foo Bar (TEAMID)"
#        export CODEX_PKG_NOTARY_PROFILE="AC_PASSWORD"   # `xcrun notarytool store-credentials` 로 미리 저장한 프로파일
#
# Optional: set CODEX_PKG_VERSION (default: short git sha).
#           set CODEX_PKG_TARGET_ARCH=x86_64 to cross-build for Intel.
#           set CODEX_PKG_UNIVERSAL=1 to produce an arm64+x86_64 universal binary.
#           set CODEX_CMD_NAME=<name> to rename the installed command (default: xtech).
#                The cargo bin stays "codex" so dev workflow is untouched; only
#                the .pkg payload renames it.
#
# Output:   <repo>/dist/<cmdname>-<version>-<arch>.pkg
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CARGO_DIR="${REPO_ROOT}/codex-rs"
DIST_DIR="${REPO_ROOT}/dist"

VERSION="${CODEX_PKG_VERSION:-$(git -C "${REPO_ROOT}" rev-parse --short HEAD 2>/dev/null || echo dev)}"
HOST_ARCH="$(uname -m)"   # arm64 or x86_64
TARGET_ARCH="${CODEX_PKG_TARGET_ARCH:-${HOST_ARCH}}"
UNIVERSAL="${CODEX_PKG_UNIVERSAL:-0}"
CMD_NAME="${CODEX_CMD_NAME:-xtech}"
IDENTIFIER="com.kimtaehan.${CMD_NAME}"

# cargo on PATH
export PATH="${HOME}/.cargo/bin:${PATH}"

mkdir -p "${DIST_DIR}"
STAGE_ROOT="$(mktemp -d)"
trap 'rm -rf "${STAGE_ROOT}"' EXIT

mkdir -p "${STAGE_ROOT}/usr/local/bin"

cd "${CARGO_DIR}"

build_for() {
  local triple="$1"
  rustup target add "${triple}" >/dev/null
  cargo build --release --bin codex --target "${triple}"
  echo "${CARGO_DIR}/target/${triple}/release/codex"
}

INSTALLED_BIN="${STAGE_ROOT}/usr/local/bin/${CMD_NAME}"

if [[ "${UNIVERSAL}" == "1" ]]; then
  echo ">> building universal (arm64 + x86_64) -> ${CMD_NAME}"
  ARM_BIN="$(build_for aarch64-apple-darwin)"
  X86_BIN="$(build_for x86_64-apple-darwin)"
  lipo -create -output "${INSTALLED_BIN}" "${ARM_BIN}" "${X86_BIN}"
  ARCH_TAG="universal"
else
  case "${TARGET_ARCH}" in
    arm64|aarch64) TRIPLE="aarch64-apple-darwin" ;;
    x86_64|x86-64) TRIPLE="x86_64-apple-darwin" ;;
    *) echo "unknown target arch: ${TARGET_ARCH}"; exit 1 ;;
  esac
  echo ">> building ${TRIPLE} -> ${CMD_NAME}"
  BIN_PATH="$(build_for "${TRIPLE}")"
  cp "${BIN_PATH}" "${INSTALLED_BIN}"
  ARCH_TAG="${TARGET_ARCH}"
fi

chmod +x "${INSTALLED_BIN}"

UNSIGNED_PKG="${DIST_DIR}/${CMD_NAME}-${VERSION}-${ARCH_TAG}-unsigned.pkg"
FINAL_PKG="${DIST_DIR}/${CMD_NAME}-${VERSION}-${ARCH_TAG}.pkg"

echo ">> pkgbuild ${UNSIGNED_PKG}"
SCRIPTS_DIR="$(dirname "${BASH_SOURCE[0]}")/macos-pkg-scripts"
PKGBUILD_ARGS=(
  --root "${STAGE_ROOT}"
  --identifier "${IDENTIFIER}"
  --version "${VERSION}"
  --install-location "/"
)
if [[ -d "${SCRIPTS_DIR}" ]]; then
  # Make sure script files are executable in the staged copy.
  chmod +x "${SCRIPTS_DIR}"/* 2>/dev/null || true
  PKGBUILD_ARGS+=(--scripts "${SCRIPTS_DIR}")
fi
pkgbuild "${PKGBUILD_ARGS[@]}" "${UNSIGNED_PKG}"

if [[ -n "${CODEX_PKG_SIGN_IDENTITY:-}" ]]; then
  echo ">> productsign with ${CODEX_PKG_SIGN_IDENTITY}"
  productsign --sign "${CODEX_PKG_SIGN_IDENTITY}" "${UNSIGNED_PKG}" "${FINAL_PKG}"
  rm -f "${UNSIGNED_PKG}"

  if [[ -n "${CODEX_PKG_NOTARY_PROFILE:-}" ]]; then
    echo ">> notarytool submit (profile: ${CODEX_PKG_NOTARY_PROFILE})"
    xcrun notarytool submit "${FINAL_PKG}" \
      --keychain-profile "${CODEX_PKG_NOTARY_PROFILE}" --wait
    echo ">> stapler staple"
    xcrun stapler staple "${FINAL_PKG}"
  else
    echo "!! notary profile unset — skipping notarization (signed but not notarized)"
  fi
else
  mv "${UNSIGNED_PKG}" "${FINAL_PKG}"
  echo "!! no Developer ID set — produced an UNSIGNED pkg"
  echo "   recipients will need: xattr -d com.apple.quarantine <pkg>"
fi

# --- Tarball for curl|bash distribution -------------------------------------
# Produces both a stable-named (`xtech-<arch>.tar.gz`) and version-suffixed
# tarball. Upload the stable-named one to a GitHub release; install.sh
# consumes it via `releases/latest/download/`.
TAR_VERSIONED="${DIST_DIR}/${CMD_NAME}-${VERSION}-${ARCH_TAG}.tar.gz"
TAR_STABLE="${DIST_DIR}/${CMD_NAME}-${ARCH_TAG}.tar.gz"
echo ">> tarball ${TAR_VERSIONED}"
tar -C "${STAGE_ROOT}/usr/local/bin" -czf "${TAR_VERSIONED}" "${CMD_NAME}"
cp "${TAR_VERSIONED}" "${TAR_STABLE}"

echo
echo "=================================================="
echo "OK: ${FINAL_PKG}"
echo "    $(du -h "${FINAL_PKG}"     | awk '{print $1}')"
echo "OK: ${TAR_STABLE}"
echo "    $(du -h "${TAR_STABLE}"    | awk '{print $1}')"
echo
echo "Manual install:    sudo installer -pkg \"${FINAL_PKG}\" -target /"
echo "GitHub release:    upload ${TAR_STABLE##*/}  (and ${CMD_NAME}-x86_64.tar.gz when universal/intel built)"
echo "After install:     \`${CMD_NAME}\` is on PATH at /usr/local/bin/${CMD_NAME}"
echo "=================================================="
