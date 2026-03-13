#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE_TAG="${APPIMAGE_DOCKER_IMAGE:-enzimcoder/appimage-builder:local}"
DOCKERFILE="${APPIMAGE_DOCKERFILE:-${ROOT_DIR}/packaging/appimage/Dockerfile}"
UID_GID="$(id -u):$(id -g)"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required but not found in PATH." >&2
  exit 1
fi

mkdir -p "${ROOT_DIR}/dist" "${ROOT_DIR}/.appimage-build"

echo "[appimage] Building builder image ${IMAGE_TAG}..."
docker build -f "${DOCKERFILE}" -t "${IMAGE_TAG}" "${ROOT_DIR}"

echo "[appimage] Building AppImage inside container..."
docker run --rm \
  --user "${UID_GID}" \
  --workdir /workspace \
  -e HOME=/workspace/.appimage-build/home \
  -e CARGO_HOME=/workspace/.appimage-build/cargo-home \
  -e DIST_DIR=/workspace/dist \
  -v "${ROOT_DIR}:/workspace" \
  "${IMAGE_TAG}" \
  /workspace/scripts/build_appimage_in_container.sh
