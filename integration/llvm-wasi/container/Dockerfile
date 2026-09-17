# Reproducible Linux toolchain for the managed LLVM/WASI pipeline.
# The experimental ILCompiler.LLVM host pack is published for linux-x64,
# linux-arm64 and win-x64 only, so macOS developers run the managed steps here.
# Build:  docker build -t dotnet-pal-llvm integration/llvm-wasi/container
# Run:    docker run --rm -it -v "$PWD:/work" -w /work dotnet-pal-llvm bash scripts/llvm-investigate.sh
FROM ubuntu:24.04
ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git xz-utils python3 build-essential clang lld llvm cmake ninja-build \
    zlib1g-dev libicu-dev && rm -rf /var/lib/apt/lists/*
ARG NODE_VERSION=25.9.0
RUN arch="$( [ "$TARGETARCH" = arm64 ] && echo arm64 || echo x64 )" && \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${arch}.tar.xz" -o /tmp/node.tar.xz && \
    tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 && rm /tmp/node.tar.xz && node --version
ARG DOTNET_VERSION=10.0.401
ENV DOTNET_ROOT=/opt/dotnet PATH=/opt/dotnet:/root/.cargo/bin:${PATH} DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1
RUN curl -fsSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh && \
    bash /tmp/dotnet-install.sh --version "${DOTNET_VERSION}" --install-dir /opt/dotnet && rm /tmp/dotnet-install.sh && dotnet --version
ARG RUST_VERSION=1.98.1
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" && \
    rustup target add wasm32-wasip1 wasm32-unknown-unknown wasm32v1-none && cargo --version
ARG WASI_SDK_RELEASE=29
RUN arch="$( [ "$TARGETARCH" = arm64 ] && echo arm64 || echo x86_64 )" && \
    curl -fsSL "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_RELEASE}/wasi-sdk-${WASI_SDK_RELEASE}.0-${arch}-linux.tar.gz" -o /tmp/wasi-sdk.tar.gz && \
    mkdir -p /opt && tar -xzf /tmp/wasi-sdk.tar.gz -C /opt && mv "/opt/wasi-sdk-${WASI_SDK_RELEASE}.0-${arch}-linux" /opt/wasi-sdk && \
    sha256sum /tmp/wasi-sdk.tar.gz > /opt/wasi-sdk/archive.sha256 && rm /tmp/wasi-sdk.tar.gz && /opt/wasi-sdk/bin/clang --version | head -1
ENV WASI_SDK_PATH=/opt/wasi-sdk NUGET_PACKAGES=/nuget
WORKDIR /work
