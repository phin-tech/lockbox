# Pi in a sandbox. Playpen gives the build its version, and OpenShell runs it as the sandbox user, with /sandbox as
# home, where Playpen writes Pi's settings after the sandbox is made. Pi is a Node program, so the program that reaches
# its model is node itself. Its install scripts are skipped, as its own guide to containers does.
FROM node:24-bookworm-slim
ARG PI_VERSION
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates git procps ripgrep \
    && rm -rf /var/lib/apt/lists/*
RUN npm install -g --ignore-scripts "@earendil-works/pi-coding-agent@${PI_VERSION}"
