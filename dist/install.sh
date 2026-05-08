#!/usr/bin/env bash
#
# xtech one-liner installer for macOS (and Linux, when builds exist).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/OWNER/REPO/main/dist/install.sh | bash
#
# Pin a specific release tag (otherwise installs `latest`):
#   curl -fsSL .../install.sh | XTECH_VERSION=v0.1.0 bash
#
# Override hosting org / repo:
#   curl -fsSL .../install.sh | XTECH_OWNER=myco XTECH_REPO=xtech bash
#
# Direct asset URL (skips GitHub release lookup):
#   curl -fsSL .../install.sh | bash -s -- --url https://.../xtech-arm64.tar.gz
#
# What it does:
#   1. detect arch (arm64 / x86_64)
#   2. fetch xtech-<arch>.tar.gz from the GitHub release
#   3. extract and install the binary at /usr/local/bin/xtech (sudo)
#   4. drop a starter ~/.config/xtech/xtech.json if you don't already have one
#   5. print next-step reminder

set -euo pipefail

# --- 호스팅 (배포자 1회 편집) ----------------------------------------------------
# Replace with the GitHub repository hosting the .tar.gz release assets.
DEFAULT_OWNER="kim-taehan"
DEFAULT_REPO="xtech"
# ------------------------------------------------------------------------------

OWNER="${XTECH_OWNER:-${DEFAULT_OWNER}}"
REPO="${XTECH_REPO:-${DEFAULT_REPO}}"
VERSION="${XTECH_VERSION:-latest}"
DIRECT_URL="${XTECH_PKG_URL:-}"
INSTALL_DIR="${XTECH_INSTALL_DIR:-/usr/local/bin}"
CMD_NAME="xtech"

# Parse simple flags from `bash -s -- ...`.
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --url)     DIRECT_URL="$2"; shift 2 ;;
    --owner)   OWNER="$2"; shift 2 ;;
    --repo)    REPO="$2"; shift 2 ;;
    --dir)     INSTALL_DIR="$2"; shift 2 ;;
    --help|-h) sed -n '2,25p' "$0" 2>/dev/null || true; exit 0 ;;
    *)         echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

OS="$(uname -s)"
ARCH_RAW="$(uname -m)"
case "${ARCH_RAW}" in
  arm64|aarch64|x86_64) ;;
  *) echo "xtech: unsupported architecture '${ARCH_RAW}'." >&2; exit 1 ;;
esac

if [[ "${OS}" != "Darwin" ]]; then
  echo "xtech: this installer currently supports macOS only." >&2
  exit 1
fi

# Single universal binary covers both arm64 and x86_64.
ASSET="${CMD_NAME}-universal.tar.gz"

if [[ -z "${DIRECT_URL}" ]]; then
  if [[ "${VERSION}" == "latest" ]]; then
    PKG_URL="https://github.com/${OWNER}/${REPO}/releases/latest/download/${ASSET}"
  else
    PKG_URL="https://github.com/${OWNER}/${REPO}/releases/download/${VERSION}/${ASSET}"
  fi
else
  PKG_URL="${DIRECT_URL}"
fi

echo ">> os/arch: ${OS}/${ARCH_RAW}"
echo ">> asset:   ${PKG_URL}"

TMPDIR_REAL="$(mktemp -d -t xtech-install.XXXXXX)"
trap 'rm -rf "${TMPDIR_REAL}"' EXIT
TARBALL="${TMPDIR_REAL}/${ASSET}"

if ! curl -fL --progress-bar -o "${TARBALL}" "${PKG_URL}"; then
  cat >&2 <<EOF

xtech: download failed.
  URL:   ${PKG_URL}

흔한 원인:
  - 이 GitHub 저장소에 ${VERSION} 릴리스가 아직 없거나, 자산 이름이 다른 경우.
    릴리스 자산은 정확히 \`${ASSET}\` 이름으로 업로드돼 있어야 합니다.
  - 사내 프록시/방화벽으로 github.com 접근이 막힌 경우.
  - --owner / --repo 가 잘못 지정된 경우.
EOF
  exit 1
fi

echo ">> extracting"
tar -xzf "${TARBALL}" -C "${TMPDIR_REAL}"
EXTRACTED_BIN="${TMPDIR_REAL}/${CMD_NAME}"
if [[ ! -x "${EXTRACTED_BIN}" ]]; then
  echo "xtech: tarball did not contain a '${CMD_NAME}' binary." >&2
  exit 1
fi

# Install to INSTALL_DIR (default /usr/local/bin). sudo if needed.
need_sudo=""
if [[ ! -w "${INSTALL_DIR}" ]]; then
  need_sudo="sudo"
fi
echo ">> installing to ${INSTALL_DIR}/${CMD_NAME} (${need_sudo:-no-sudo})"
${need_sudo} install -m 0755 "${EXTRACTED_BIN}" "${INSTALL_DIR}/${CMD_NAME}"

# Drop ~/.config/xtech/xtech.json template if missing. Runs as the invoking user
# (no sudo) so file ownership is correct without postinstall gymnastics.
TARGET_HOME="${HOME}"
CODEX_DIR="${TARGET_HOME}/.config/xtech"
TARGET_JSON="${CODEX_DIR}/xtech.json"
mkdir -p "${CODEX_DIR}"

if [[ -f "${TARGET_JSON}" ]]; then
  echo ">> ${TARGET_JSON} 이미 존재 — 건드리지 않음"
else
  cat > "${TARGET_JSON}" <<'JSON'
{
  "_comment": "xtech remote-gateway config. Fill in all three fields before running xtech.",
  "baseURL": "",
  "apiKey":  "",
  "model":   ""
}
JSON
  chmod 600 "${TARGET_JSON}"
  echo ">> 생성: ${TARGET_JSON} (mode 600)"
fi

echo
echo "=================================================="
echo "OK: ${CMD_NAME} 설치 완료."
which "${CMD_NAME}" || true
echo
echo "다음 단계:"
echo "  1) ${TARGET_JSON} 의 \"apiKey\" 를 본인 토큰으로 교체"
echo "  2) \`${CMD_NAME}\` 또는 \`${CMD_NAME} exec \"say pong\"\` 으로 실행 확인"
echo "=================================================="
