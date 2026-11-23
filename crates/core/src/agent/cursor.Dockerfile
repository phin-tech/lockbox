# Cursor's agent in a sandbox, from the archive Cursor's installer downloads, with its own Node inside. It's unpacked
# to /opt/cursor-agent, so the program that reaches Cursor is /opt/cursor-agent/node whatever the version. The image
# is Node's, as every agent's is, since Playpen writes an agent's settings with node once the sandbox is made. Each
# download is checked against the digest Cursor published, pinned here with the version.
FROM node:24-bookworm-slim
ARG CURSOR_VERSION
ARG CURSOR_SHA256_X64
ARG CURSOR_SHA256_ARM64
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git procps ripgrep \
    && rm -rf /var/lib/apt/lists/*
RUN case "${TARGETARCH}" in \
        amd64) arch=x64; sum="${CURSOR_SHA256_X64}" ;; \
        arm64) arch=arm64; sum="${CURSOR_SHA256_ARM64}" ;; \
        *) echo "no Cursor agent for ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
    && curl -fsSL -o /tmp/cursor.tar.gz \
        "https://downloads.cursor.com/lab/${CURSOR_VERSION}/linux/${arch}/agent-cli-package.tar.gz" \
    && echo "${sum}  /tmp/cursor.tar.gz" | sha256sum -c - \
    && mkdir -p /opt/cursor-agent \
    && tar -xzf /tmp/cursor.tar.gz -C /opt/cursor-agent --strip-components=1 \
    && rm /tmp/cursor.tar.gz \
    && ln -s /opt/cursor-agent/cursor-agent /usr/local/bin/cursor-agent
