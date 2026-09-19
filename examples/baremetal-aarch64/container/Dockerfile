# Linux toolchain for the source runtime build, the console probe and the bare-metal
# image: clang/lld, the pinned .NET SDK, Rust with the aarch64-unknown-none target and QEMU.
# Build:  docker build -t dotnet-pal-baremetal examples/baremetal-aarch64/container
# Run:    docker run --rm -v "$PWD:/work" -v dotnet-pal-nuget:/nuget -w /work dotnet-pal-baremetal bash scripts/source-runtime.sh /path/to/runtime
FROM ubuntu:24.04
ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git xz-utils python3 build-essential clang lld llvm cmake ninja-build \
    zlib1g-dev libicu-dev liblttng-ust-dev libunwind-dev libssl-dev libkrb5-dev time \
    qemu-system-arm qemu-user-static file && rm -rf /var/lib/apt/lists/*
ARG DOTNET_VERSION=10.0.401
ENV DOTNET_ROOT=/opt/dotnet PATH=/opt/dotnet:/root/.cargo/bin:${PATH} DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1
RUN curl -fsSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh && \
    bash /tmp/dotnet-install.sh --version "${DOTNET_VERSION}" --install-dir /opt/dotnet && rm /tmp/dotnet-install.sh && dotnet --version
ARG RUST_VERSION=1.98.1
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" && \
    rustup target add aarch64-unknown-none && rustup component add rust-src && cargo --version
ENV NUGET_PACKAGES=/nuget
WORKDIR /work
