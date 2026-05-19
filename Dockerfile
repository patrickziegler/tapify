FROM rust:trixie

RUN apt-get update && apt-get install -y \
    build-essential \
    cmake \
    clang \
    pkg-config \
    git \
    curl \
    jq \
    ffmpeg \
    libpipewire-0.3-dev \
    pipewire-bin \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://antigravity.google/cli/install.sh | bash

RUN curl -fsSL https://deb.nodesource.com/setup_lts.x | bash - \
 && apt-get install -y nodejs \
 && npm install -g @google/gemini-cli \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
