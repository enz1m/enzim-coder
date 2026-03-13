#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "${ROOT_DIR}/Cargo.toml" | head -n 1)"
APPIMAGE_PATH="${1:-${ROOT_DIR}/dist/EnzimCoder-${VERSION}-x86_64.AppImage}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required but not found in PATH." >&2
  exit 1
fi

if [ ! -f "${APPIMAGE_PATH}" ]; then
  echo "AppImage not found: ${APPIMAGE_PATH}" >&2
  exit 1
fi

APPIMAGE_DIR="$(cd -- "$(dirname -- "${APPIMAGE_PATH}")" && pwd)"
APPIMAGE_NAME="$(basename -- "${APPIMAGE_PATH}")"

docker run --rm \
  -v "${APPIMAGE_DIR}:/dist:ro" \
  ubuntu:24.04 \
  bash -lc "
    apt-get update >/dev/null &&
    apt-get install -y xvfb xauth x11-utils dbus-x11 openbox libfuse2 >/dev/null 2>&1

    cat > /tmp/run.sh <<'SH'
#!/usr/bin/env bash
set -euo pipefail

Xvfb :99 -screen 0 1440x900x24 >/tmp/xvfb.log 2>&1 &
XVFB_PID=\$!
export DISPLAY=:99

openbox >/tmp/openbox.log 2>&1 &
OPENBOX_PID=\$!

export APPIMAGE_EXTRACT_AND_RUN=1

cleanup() {
  kill \"\$APP_PID\" 2>/dev/null || true
  kill \"\$OPENBOX_PID\" 2>/dev/null || true
  kill \"\$XVFB_PID\" 2>/dev/null || true
  wait || true
}
trap cleanup EXIT

set +e
/dist/${APPIMAGE_NAME} >/tmp/app.out 2>/tmp/app.err &
APP_PID=\$!
sleep 8
set -e

if ! kill -0 \"\$APP_PID\" 2>/dev/null; then
  wait \"\$APP_PID\"
  echo 'AppImage exited before creating a stable window' >&2
  echo '--- STDERR ---' >&2
  sed -n '1,160p' /tmp/app.err >&2
  echo '--- STDOUT ---' >&2
  sed -n '1,80p' /tmp/app.out >&2
  exit 1
fi

WINDOW_TREE=\"\$(xwininfo -root -tree)\"
printf '%s\n' \"\$WINDOW_TREE\"

if ! printf '%s\n' \"\$WINDOW_TREE\" | grep -q 'Enzim Coder'; then
  echo 'AppImage process stayed alive but no Enzim Coder window was found' >&2
  echo '--- STDERR ---' >&2
  sed -n '1,160p' /tmp/app.err >&2
  exit 1
fi
SH

    chmod +x /tmp/run.sh
    dbus-run-session -- /tmp/run.sh
  "
