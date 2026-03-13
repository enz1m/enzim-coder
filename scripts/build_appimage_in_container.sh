#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' "${ROOT_DIR}/Cargo.toml" | head -n 1)}"
TEMPLATE="${ROOT_DIR}/packaging/appimage/AppImageBuilder.yml.in"
RECIPE="${ROOT_DIR}/.appimage-build/AppImageBuilder.yml"
DIST_DIR="${DIST_DIR:-${ROOT_DIR}/dist}"
BUILD_CACHE_DIR="${ROOT_DIR}/.appimage-build"
CARGO_HOME="${CARGO_HOME:-${BUILD_CACHE_DIR}/cargo-home}"
HOME="${HOME:-${BUILD_CACHE_DIR}/home}"
APPIMAGE_OUTPUT="${DIST_DIR}/EnzimCoder-${VERSION}-x86_64.AppImage"
APPIMAGE_UPDATE_INFORMATION="${APPIMAGE_UPDATE_INFORMATION:-gh-releases-zsync|enz1m|enzim-coder|latest|EnzimCoder-*-x86_64.AppImage.zsync}"
CUSTOM_APPRUN_SOURCE="${ROOT_DIR}/packaging/appimage/AppRun.c"
CUSTOM_APPRUN_BUILD="${BUILD_CACHE_DIR}/AppRun"
HOST_LOADER="${APPIMAGE_HOST_LOADER:-/lib64/ld-linux-x86-64.so.2}"

mkdir -p "${BUILD_CACHE_DIR}" "${DIST_DIR}" "${CARGO_HOME}" "${HOME}"
sed "s/@VERSION@/${VERSION}/g" "${TEMPLATE}" > "${RECIPE}"
rm -f "${APPIMAGE_OUTPUT}"
rm -f "${APPIMAGE_OUTPUT}.zsync"

export APPIMAGE_EXTRACT_AND_RUN=1
export RUSTUP_HOME="${RUSTUP_HOME:-/opt/rustup}"
export PATH="/opt/cargo-install/bin:${PATH}"
export CARGO_HOME
export HOME
export ARCH=x86_64

cd "${ROOT_DIR}"
"${APPIMAGE_BUILDER:-/opt/appimage-builder.AppImage}" --skip-tests --skip-appimage --recipe "${RECIPE}"

mkdir -p "$(dirname "${CUSTOM_APPRUN_BUILD}")"
APPIMAGE_LAUNCHER_CC="${APPIMAGE_LAUNCHER_CC:-$(command -v x86_64-linux-musl-gcc || command -v musl-gcc || true)}"
if [ -z "${APPIMAGE_LAUNCHER_CC}" ]; then
  echo "No musl C compiler found for AppRun build" >&2
  exit 1
fi

"${APPIMAGE_LAUNCHER_CC}" -static -Os \
  -D_FORTIFY_SOURCE=2 \
  -Wl,-z,relro,-z,now \
  -o "${CUSTOM_APPRUN_BUILD}" \
  "${CUSTOM_APPRUN_SOURCE}"

install -Dm755 "${CUSTOM_APPRUN_BUILD}" "${ROOT_DIR}/AppDir/AppRun"
while IFS= read -r -d '' elf_path; do
  interpreter="$(patchelf --print-interpreter "${elf_path}" 2>/dev/null || true)"
  if [ "${interpreter}" = "lib64/ld-linux-x86-64.so.2" ]; then
    patchelf --set-interpreter "${HOST_LOADER}" "${elf_path}"
  fi
done < <(find "${ROOT_DIR}/AppDir" -type f -print0)

rm -f "${ROOT_DIR}/AppDir/AppRun.env" "${ROOT_DIR}/AppDir/lib64"
rm -rf "${ROOT_DIR}/AppDir/runtime"
rm -rf \
  "${ROOT_DIR}/AppDir/usr/share/bug" \
  "${ROOT_DIR}/AppDir/usr/share/doc" \
  "${ROOT_DIR}/AppDir/var/cache/apt" \
  "${ROOT_DIR}/AppDir/var/lib/apt"

APPIMAGE_EXTRACT_AND_RUN=1 "${APPIMAGE_TOOL:-/opt/appimagetool.AppImage}" \
  -u "${APPIMAGE_UPDATE_INFORMATION}" \
  "${ROOT_DIR}/AppDir" \
  "${APPIMAGE_OUTPUT}"

if [ -f "${ROOT_DIR}/EnzimCoder-${VERSION}-x86_64.AppImage" ]; then
  mv -f "${ROOT_DIR}/EnzimCoder-${VERSION}-x86_64.AppImage" "${APPIMAGE_OUTPUT}"
fi

if [ ! -f "${APPIMAGE_OUTPUT}" ]; then
  echo "AppImage build did not produce ${APPIMAGE_OUTPUT}" >&2
  exit 1
fi

echo "AppImage created at ${APPIMAGE_OUTPUT}"
