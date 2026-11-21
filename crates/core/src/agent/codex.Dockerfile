# Codex in a sandbox. Playpen gives the build its version, and OpenShell runs it as the sandbox user, with /sandbox
# as home, where Playpen writes Codex's settings after the sandbox is made.
FROM node:24-bookworm-slim
ARG CODEX_VERSION
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates git ripgrep \
    && rm -rf /var/lib/apt/lists/*
RUN npm install -g @openai/codex@${CODEX_VERSION}
