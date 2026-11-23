# Playpen

Playpen runs your coding agents in [OpenShell](https://github.com/NVIDIA/OpenShell) sandboxes on your Mac, and lets
you decide what each one may reach. An agent works in a sandbox with a copy of your code; it reaches its model and the
services you attach, and nothing else, until you say so.

## Make a sandbox

**File ▸ New Agent…** (⌘N) asks which agent (Claude Code, Codex, Pi, oh-my-pi or PiG), a name, the folders to copy
in, any kits, and the model key to use. Pi, oh-my-pi and PiG take an API key for OpenRouter, Anthropic, OpenAI or a
LiteLLM proxy, and the model to start with, which a LiteLLM proxy needs. Folders are copied into `/sandbox` with their git history, respecting `.gitignore`. Open the agent
in a Herdr terminal, Ghostty or Terminal from the sandbox's **Open** button. A sandbox outlives its sessions:
close the window and the agent keeps going when Herdr is installed.

## Decide what it reaches

A sandbox's policy denies everything it doesn't allow. When something in a sandbox tries to reach a host the policy
doesn't list, OpenShell drafts a rule for it, and Playpen shows it under **Network requests**, with the program that
made the connection. **Approve** allows it, **Reject** doesn't, and **Approve All** takes the lot. Approved rules take
a few seconds to reach the sandbox, so run the blocked command again.

**Recent activity** lists what the sandbox has connected to lately, and **Blocked** narrows it to what was stopped.

## Keys stay out of the sandbox

**Providers** lists the credentials OpenShell holds, grouped by gateway: a model key, a Claude or ChatGPT sign-in, or a
service such as GitHub. The agent sees only a stand-in, which OpenShell swaps for the real key on requests to that
service. **Add to Gateway…** copies a provider to another gateway, and **Attach service…** on a sandbox gives it one.

## Gateways

A gateway is where OpenShell keeps and runs sandboxes: Docker on this Mac, a MicroVM gateway Playpen can set up (each
sandbox a small virtual machine with its own Linux kernel), or a Kubernetes cluster. Add or remove one from the
**Gateways** list. Sandboxes are listed under the gateway that made them, and two gateways can each hold a sandbox of
the same name.

## Bring the work back

**Bring Back** fetches what the agent did in a sandbox as a git branch named `playpen/<sandbox>`, in the repository the
folder came from. Your working copy is never touched.

## Ports

Forward a sandbox's port to `localhost` from its detail pane, and open it in the browser from there.

## From the command line

`Playpen.app/Contents/MacOS/Playpen` answers commands without opening a window, each with `--json` and `--gateway`:

```sh
playpen gateways
playpen sandboxes --gateway microvm
playpen sandbox stop claude
playpen sandbox attach claude github
playpen providers --json
playpen services show github
```

## Keys

⌘K lists every command. ⌘/ shows every key by its menu. ⌘, opens Settings, where the theme follows the appearance
and the text size is yours to set.

## Requires

macOS 14 or later, and [OpenShell](https://github.com/NVIDIA/OpenShell) with Docker (or its MicroVM driver). Every
agent runs from an image Playpen builds or pulls; you sign in to Claude and ChatGPT, or add an API key, in Providers.
