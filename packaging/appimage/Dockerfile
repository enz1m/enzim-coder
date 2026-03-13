FROM ubuntu:26.04

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/opt/rustup \
    CARGO_INSTALL_HOME=/opt/cargo-install \
    PATH=/opt/cargo-install/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    APPIMAGE_BUILDER=/opt/appimage-builder.AppImage \
    APPIMAGE_TOOL=/opt/appimagetool.AppImage

RUN apt-get update && apt-get install -y \
    appstream \
    build-essential \
    ca-certificates \
    curl \
    desktop-file-utils \
    dpkg-dev \
    file \
    git \
    glib2.0-bin \
    libadwaita-1-dev \
    libfuse2t64 \
    libglib2.0-dev \
    libgtk-4-dev \
    libgtksourceview-5-dev \
    librsvg2-bin \
    librsvg2-common \
    musl-tools \
    patchelf \
    pkg-config \
    python3 \
    python3-venv \
    squashfs-tools \
    xz-utils \
    zsync \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /opt/cargo-install/bin \
    && curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain 1.92.0 \
    && ln -sf /opt/rustup/toolchains/1.92.0-x86_64-unknown-linux-gnu/bin/cargo /opt/cargo-install/bin/cargo \
    && ln -sf /opt/rustup/toolchains/1.92.0-x86_64-unknown-linux-gnu/bin/rustc /opt/cargo-install/bin/rustc \
    && ln -sf /opt/rustup/toolchains/1.92.0-x86_64-unknown-linux-gnu/bin/rustdoc /opt/cargo-install/bin/rustdoc \
    && chmod -R a+rX /opt/rustup /opt/cargo-install

RUN curl -L --fail --retry 3 -o /opt/appimage-builder.AppImage \
    https://github.com/AppImageCrafters/appimage-builder/releases/download/v1.1.0/appimage-builder-1.1.0-x86_64.AppImage \
    && chmod +x /opt/appimage-builder.AppImage

RUN curl -L --fail --retry 3 -o /opt/appimagetool.AppImage \
    https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage \
    && chmod +x /opt/appimagetool.AppImage

RUN printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > /usr/local/bin/apt-key \
    && chmod +x /usr/local/bin/apt-key
