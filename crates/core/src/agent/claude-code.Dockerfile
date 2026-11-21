# Claude Code for an OpenShell sandbox. Playpen builds it once for each version it pins, which it passes in.
FROM node:24-bookworm-slim

ARG CLAUDE_CODE_VERSION

# git and ripgrep, since the sandbox won't let Claude Code fetch anything it finds missing.
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates git ripgrep \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}"
