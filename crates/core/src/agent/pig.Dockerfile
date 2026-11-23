# PiG, the Go port of Pi, in a sandbox, from its release: one static program. The image is Node's, as every agent's
# is, since Playpen writes an agent's settings with node once the sandbox is made. Each download is checked against
# the digest its release published, pinned here with the version.
FROM node:24-bookworm-slim
ARG PIG_VERSION
ARG PIG_SHA256_AMD64
ARG PIG_SHA256_ARM64
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git procps ripgrep \
    && rm -rf /var/lib/apt/lists/*
RUN case "${TARGETARCH}" in \
        amd64) sum="${PIG_SHA256_AMD64}" ;; \
        arm64) sum="${PIG_SHA256_ARM64}" ;; \
        *) echo "no pig for ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
    && name="pig-${PIG_VERSION}-linux-${TARGETARCH}" \
    && curl -fsSL -o /tmp/pig.tar.gz \
        "https://github.com/MichaelKinsy/PiG/releases/download/v${PIG_VERSION}/${name}.tar.gz" \
    && echo "${sum}  /tmp/pig.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/pig.tar.gz -C /tmp \
    && install -m 755 "/tmp/${name}/pig" /usr/local/bin/pig \
    && rm -rf /tmp/pig.tar.gz "/tmp/${name}"
