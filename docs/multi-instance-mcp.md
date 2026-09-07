# Multi-instance MCP: one connection, several Lific servers

`lific mcp --instances <FILE>` runs the stdio MCP proxy in front of **several**
separately authenticated Lific instances at once. One MCP entry in your agent's
config; one tool surface; explicit per-call routing.

This is the local proxy, extended. Nothing changes on the servers, they do not
know about each other, and no credential ever crosses between them.

The case it exists for: you keep a private tracker, and you also read and write
a public community one. Two MCP entries would work, but then every tool appears
twice and the agent has to guess which `create_issue` it wants. With this, there
is one `create_issue`, and it takes an `instance` argument.

## Configuration

Write a TOML file. It lives wherever you like; `~/.config/lific/instances.toml`
is a reasonable default. It contains **no secrets** (see below).

```toml
# Optional. Read-only tools may omit `instance` and land here.
# Tools that write never use it.
default = "private"

[instances.private]
url = "https://lific.example.com"
token_env = "LIFIC_PRIVATE_TOKEN"

[instances.community]
url = "https://community.lific.example.com"
token_env = "LIFIC_COMMUNITY_TOKEN"
```

### Keys

| Key | Where | Meaning |
| --- | --- | --- |
| `default` | top level | Alias used when an **allowlisted read-only tool** omits `instance`. Optional. Must name a configured alias. |
| `instances.<alias>.url` | per instance | Base URL of that server. `http://` or `https://`, no `user:password@`, no `?query`, no `#fragment`. |
| `instances.<alias>.token_env` | per instance | Name of an environment variable holding that instance's bearer token. |
| `instances.<alias>.credential` | per instance | `"login"` to use the token `lific login` stored for that URL, or `"none"` for an instance that takes no authentication. |

Exactly one of `token_env` or `credential` is required per instance. There is no
default and no inference: two aliases can never end up sharing a credential
because something resolved one for their origin.

Aliases are lowercase, and may contain letters, digits, `-` and `_`, up to 32
characters.

### There is no key that holds a token

Deliberately. A secret written into this file is a secret in your dotfiles, in
your backups, and eventually in a paste. The file only ever *names* where the
token lives, so the file itself is safe to keep in a normal config directory.

For the same reason a URL may not carry userinfo or a query string. Those are
the two other places people put secrets, and this file's URLs get printed, in
logs and in `list_instances`.

## Getting a token

Either mechanism Lific already has works. Per instance, pick one.

**A dedicated API key (recommended for agents).** In the web UI of that
instance, open **Connected Tools** and create a key. Export it under the name
you put in `token_env`:

```fish
set -x LIFIC_PRIVATE_TOKEN   lific_...
set -x LIFIC_COMMUNITY_TOKEN lific_...
```

Put the exports wherever your agent's environment comes from. A key per
instance is the point: revoking the community key must not affect the private
one.

**The CLI login you already have.** If you have run `lific login --url
https://lific.example.com`, set `credential = "login"` for that instance and it
reuses the stored token:

```toml
[instances.private]
url = "https://lific.example.com"
credential = "login"
```

There is no new OAuth flow here. `lific login` remains the way to obtain a
token interactively, and this reads what it stored.

`credential = "login"` reads the **stored** token only: the keyring, then the
credentials file. Unlike a plain CLI invocation it does not fall back to an
ambient `LIFIC_TOKEN`, because one exported variable must not silently become
the credential for every alias that happens to share an origin. An alias that
wants an environment variable names it with `token_env`.

## Running it

```bash
lific mcp --instances ~/.config/lific/instances.toml
```

No database is opened and no `--url` applies; each instance carries its own.
`--instances` conflicts with `--remote`, and is refused alongside `--url`.

At startup the proxy contacts every instance independently, runs `initialize`,
pages through `tools/list`, and checks that every backend advertises the **same
complete tool definitions**. This includes descriptions, annotations, titles,
input and output schemas, and any other metadata. Tool order and JSON object
key order may differ; field values and field presence must match exactly.
If they disagree, the launch fails and says which alias and which tool.
Schemas are never merged and versions are never guessed.

Even a wording difference fails compatibility: descriptions and annotations
instruct the agent, so choosing one backend's metadata would hide the other's.
Discovery and repository binding lookups share one **60-second startup
deadline across all instances**. Each HTTP request also has a 20-second limit.
Neither another page nor another alias resets the total budget.

## Using it from an agent

Every tool gains one argument:

```jsonc
{ "name": "create_issue", "arguments": { "instance": "community", "title": "…" } }
```

The rules, in full:

- **One instance configured:** `instance` is optional everywhere.
- **More than one, read-only tool:** `instance` may be omitted; the call goes to
  `default`. If no `default` is configured, the call is refused.
- **More than one, anything that writes, or any tool this build does not
  recognize:** `instance` is required. There is no default for writes.
- **Unknown alias, wrong type, empty string:** refused, before anything is
  forwarded. A typo cannot write to the wrong tracker because it never reaches a
  tracker.

The advertised schema says the same thing, so a client can catch an omission
without spending a call: with several instances configured, `instance` is listed
in `required` on every writing tool and on every tool this build does not
recognize, and left optional on the allowlisted reads. A backend's own
`additionalProperties: false` is preserved.

`--url` and `LIFIC_URL` are ignored in this mode, since every instance carries
its own. Passing `--url` explicitly alongside `--instances` is an error; merely
having `LIFIC_URL` exported in your shell is not.

The read-only allowlist is fixed in the binary: `search`, `list_issues`,
`get_issue`, `get_board`, `list_resources`, `get_page`, `get_plan`,
`get_activity`, `list_comments`, `list_attachments`, `get_attachment`,
`export`. It is not derived from the backend's `readOnlyHint`, because that is
data supplied by the server being routed to.

### Discovery

A synthetic `list_instances` tool reports the aliases, their hosts, which is the
default, whether each is authenticated, and the project each one resolved for
the current repository. It never reports credentials.

### Knowing where a result came from

Every result carries its source alias, written by the proxy after the backend
answered:

- `result._meta["dev.lific/instance"]` , the authoritative value.
- A trailing text block, `lific:instance=<alias>`, so it is visible in a plain
  transcript.
- On failures, the error message is prefixed `[instance <alias>]` and
  `error.data.instance` names it.

A backend cannot forge this. Any provenance it puts in its own payload is
stripped and replaced with the alias the proxy actually routed to. That matters
because two instances can legitimately hold the same identifier: `LIF-42` on
`private` and `LIF-42` on `community` are different issues, and the payloads may
be indistinguishable.

### Repository bindings

Each instance resolves the current directory's project binding **itself**,
against its own server, with its own credential. `private` and `community` may
disagree about what this checkout is, and each answer stays with its own
server. An omitted `project` is filled in from that instance's binding only.
Omitted or `null` tool `arguments` become an empty object before the binding is
applied. An unbound instance never inherits another instance's project.

## OpenCode configuration

One local MCP entry replaces the two you would otherwise need. Add to
`~/.config/opencode/opencode.json`:

```jsonc
{
  "mcp": {
    "lific": {
      "type": "local",
      "enabled": true,
      "timeout": 60000,
      "command": [
        "lific",
        "mcp",
        "--instances",
        "/home/you/.config/lific/instances.toml"
      ],
      "environment": {
        "LIFIC_PRIVATE_TOKEN": "lific_...",
        "LIFIC_COMMUNITY_TOKEN": "lific_..."
      }
    }
  }
}
```

If you would rather not keep tokens in the OpenCode config, drop the
`environment` block and export the variables from the shell that launches
OpenCode.

Set `timeout` to `60000` milliseconds to match the proxy's 60-second startup
budget. It discovers all backends before answering the client's initialization;
a shorter client timeout can cut that discovery short.

## What it will not do

- **No per-call URLs or credentials.** A tool call selects an alias, nothing
  else. There is no way to make it reach a server you did not configure.
- **No fallback.** An instance that is down, rejects the credential, or fails a
  call produces a failed call for that alias. It never retries elsewhere.
- **No ambient state.** There is no "active instance" to switch, so nothing a
  previous call did can change where the next one goes.
- **No redirects.** Each backend's HTTP client refuses to follow them, so a
  bearer token is never replayed to a `Location` the backend chose.
- **No plaintext credentials off-box.** A credentialed `http://` instance on a
  non-loopback host is refused at startup, same rule as the rest of the CLI.
- **No resources or prompts.** `resources/*`, `prompts/*`, `completion/*` and
  `logging/setLevel` are refused with `-32601` rather than routed to some
  instance. Their URIs, names and list cursors are per-server and would be
  meaningless behind one connection. Add a separate `lific mcp --remote` entry
  for a single server if you need them.

## Limits

Fixed in the binary, all of them bounding something a backend or a client
controls:

| Limit | Value |
| --- | --- |
| Response body per call | 4 MiB |
| Repository binding lookup body | 4 MiB |
| Request line on stdin | 1 MiB |
| Config file | Regular file, at most 256 KiB |
| Instances per config | 16 |
| `tools/list` pages per instance | 50 |
| Tools per instance | 512 |
| Combined serialized tool definitions per instance, across all pages | 4 MiB |
| Call timeout / connect timeout | 120 s / 10 s |
| Startup HTTP request timeout | 20 s |
| Total discovery and binding deadline, across all instances | 60 s |

An over-limit request line is counted and discarded as it arrives rather than
buffered, and the next line is still served.

Config reads reject directories, devices and named pipes. The read itself is
capped too, so a file growing after its size check cannot bypass the limit.

Discovery also totals the serialized JSON bytes of every tool definition on
every page. An instance exceeding 4 MiB in total fails startup even if each
response fits the per-call limit. Each instance has its own budget.

Configured tokens are scrubbed from every message this proxy prints or relays,
on the raw bytes and again on the decoded document (strings and object keys, at
every depth), so a backend cannot smuggle its credential past the filter by
spelling it in `\u` escapes.

Every backend answer is checked before it is relayed: it must be `jsonrpc:
"2.0"`, carry exactly the `id` of the request it is answering, and carry exactly
one of `result` or `error`. A result must be an object; an error must have an
integer `code` and a string `message`. The same envelope check runs during
startup discovery. Routed tool results must also decode as an MCP
`CallToolResult`, including a valid `content` array. An empty array is valid;
a missing array or malformed content is not. Invalid replies become errors
carrying the original request ID and the actual source alias, rather than being
relayed as successful results.

## Single-instance mode is unchanged

`lific mcp` (local database) and `lific mcp --remote --url <URL>` behave exactly
as before. Neither gains an `instance` argument, neither consults this file.
