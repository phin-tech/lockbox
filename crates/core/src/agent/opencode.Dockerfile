# OpenCode in a sandbox, from its release: one program, with Bun inside it. The image is Node's, as every agent's is,
# since Playpen writes an agent's settings with node once the sandbox is made. Each download is checked against the
# digest its release published, pinned here with the version.
FROM node:24-bookworm-slim
ARG OPENCODE_VERSION
ARG OPENCODE_SHA256_X64
ARG OPENCODE_SHA256_ARM64
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git procps ripgrep \
    && rm -rf /var/lib/apt/lists/*
RUN case "${TARGETARCH}" in \
        amd64) arch=x64; sum="${OPENCODE_SHA256_X64}" ;; \
        arm64) arch=arm64; sum="${OPENCODE_SHA256_ARM64}" ;; \
        *) echo "no OpenCode for ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
    && curl -fsSL -o /tmp/opencode.tar.gz \
        "https://github.com/anomalyco/opencode/releases/download/v${OPENCODE_VERSION}/opencode-linux-${arch}.tar.gz" \
    && echo "${sum}  /tmp/opencode.tar.gz" | sha256sum -c - \
    && mkdir /tmp/opencode \
    && tar -xzf /tmp/opencode.tar.gz -C /tmp/opencode \
    && install -m 755 "$(find /tmp/opencode -type f -name opencode | head -1)" /usr/local/bin/opencode \
    && rm -rf /tmp/opencode.tar.gz /tmp/opencode
