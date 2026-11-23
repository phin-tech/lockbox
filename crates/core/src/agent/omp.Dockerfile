# oh-my-pi in a sandbox, from its standalone release, which carries Bun inside it, so nothing else runs it. The image
# is Node's, as every agent's is, since Playpen writes an agent's settings with node once the sandbox is made. Each
# download is checked against the digest its release published, pinned here with the version.
FROM node:24-bookworm-slim
ARG OMP_VERSION
ARG OMP_SHA256_X64
ARG OMP_SHA256_ARM64
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git procps ripgrep \
    && rm -rf /var/lib/apt/lists/*
RUN case "${TARGETARCH}" in \
        amd64) arch=x64; sum="${OMP_SHA256_X64}" ;; \
        arm64) arch=arm64; sum="${OMP_SHA256_ARM64}" ;; \
        *) echo "no omp for ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
    && curl -fsSL -o /usr/local/bin/omp \
        "https://github.com/can1357/oh-my-pi/releases/download/v${OMP_VERSION}/omp-linux-${arch}" \
    && echo "${sum}  /usr/local/bin/omp" | sha256sum -c - \
    && chmod 755 /usr/local/bin/omp
