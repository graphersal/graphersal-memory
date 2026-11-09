# How it works

> Pre-release: this page describes the target design. Nothing here is implemented yet; see the
> [status table](../README.md#status).

## Processes

```text
harness ──stdio──> shim (graphersal-memory mcp --profile use) ──socket──┐
                                                                        v
browser ──http───> ui (graphersal-memory ui; develop)        ──socket──> daemon (graphersal-memory daemon)
                                                                          owns the stores, socket only;
                                                                          per store: one session thread
                                                                          + background job workers
```

- **Shim** (`graphersal-memory mcp`). Your harness starts it, one per agent session, exactly like
  any other stdio MCP server. It finds the project (for a developer, the repository root of the
  working directory), connects to the daemon's socket, starting the daemon if none is running,
  sends one handshake line, and then relays MCP messages in both directions. It holds no state.
- **Daemon** (`graphersal-memory daemon`). This is the only process that opens the stores. The
  first shim starts it, and it runs detached. It exits after an idle time: no connection, no
  background work and no `ui` command for two hours by default, which is configurable. It listens
  on the local socket only and opens no network port. If two shims start at the same moment, a
  start lock file decides which one spawns the daemon.
- **UI** (`graphersal-memory ui`, develop mode). It runs while its terminal runs, serves the
  browser UI on loopback and talks to the daemon over the same socket. A forgotten browser tab
  does not keep the daemon alive.

Several agent sessions, in several terminals or harnesses, can use the memory at the same time.
They all talk to one daemon, which is the single writer of each store.

## Socket, trust and the handshake

- On every platform, the shim and the daemon talk over a Unix domain socket (`AF_UNIX`, also
  available on Windows 10 1803 and later). Windows named pipes are the fallback if `AF_UNIX`
  proves unreliable there.
- The trust boundary is your OS user. The socket file is readable only by you, in a directory
  only you can open. Any process of your user can connect, which is no more than it could already
  do with the store files. There is no token to manage.
- The handshake is one JSON line from the shim. It carries the protocol version, the profile
  (`use` or `develop`), the project and the harness. The daemon answers with its version and the
  store it routed the connection to. After that, the connection carries plain MCP stdio framing.
- Upgrades: when a new binary meets an older running daemon, the daemon is asked to drain and
  restart, and the shim reconnects to the new one.

## Stores

- **Per-user data directory** by default (macOS `~/Library/Application Support/graphersal-memory`,
  Linux `$XDG_DATA_HOME/graphersal-memory`, Windows `%LOCALAPPDATA%\graphersal-memory`), not inside
  your repository.
- **One shared store by default.** Every memory carries its project, and search defaults to the
  current project plus your global memory: preferences and cross-project rules.
- **Isolated projects** (planned): a project can get a store of its own, for example for a client
  under NDA, for handing a project to a team, or for an experiment. Your global memory is copied
  into it one way. Every MCP connection goes to exactly one store, and nothing references across
  stores.
- Background work (backups, checkpoints, session indexing, summaries, imports) runs off the thread
  that answers the agent, so a long job never makes a memory lookup wait.

## Profiles: the two MCP servers

| Server | Default | Tools | Enforcement |
|---|---|---|---|
| `use` | on | search and read memories, remember, forget, search and read past sessions | the engine's access policy allows only what these tools write |
| `develop` | off | the above plus schema, saved queries, indexes, health, benchmark | for improving the memory's structure; enable it when you need it, disable it so it costs no context |

The daemon chooses the profile per connection, from the handshake. A client cannot raise its own
profile from inside MCP.

## The engine

The stores, queries, embeddings, access policies and the MCP endpoint come from
[Graphersal](https://github.com/graphersal/graphersal): `graphersal` (the engine), `graphersal-mcp`
(the MCP endpoint, connections, profiles and stdio framing) and `graphersal-serve` (the hosted
graph runtime). graphersal-memory adds the product around them: daemon, shim, store registry,
installation into harnesses, the memory schema and functions, import and export, and the A/B.
