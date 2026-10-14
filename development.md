# Development

## Version control

Chord uses Jujutsu (`jj`) in a colocated Git checkout, following Haniverse's
workflow with `dev` as the integration bookmark. Existing Git history and remotes
are reused. Install `jj` on macOS with `brew install jj`, then initialize each new
checkout from the repository root:

```sh
jj git init --colocate
jj config edit --repo
```

Add the following repository-local configuration. It overrides Haniverse's
user-level `trunk` aliases only for Chord; user identity and other user settings
are inherited.

```toml
[revset-aliases]
'trunk()' = 'dev@origin'

[aliases]
gf = ["git", "fetch"]
pull = ["util", "exec", "--", "bash", "-c", """
set -e
jj git fetch
jj bookmark set dev -r dev@origin --allow-backwards
jj rebase -d dev
"""]
push = ["util", "exec", "--", "bash", "-c", """
set -e
target=$(jj log --no-graph -r 'heads(::@ & mutable() & ~description(exact:"") & ~empty())' -T 'commit_id' 2>/dev/null || true)
if [ -z "$target" ]; then
  echo "error: nothing to push (no described, non-empty commit on this branch)" >&2
  exit 1
fi
jj bookmark move dev --to "$target"
exec jj git push -b dev "$@"
""", "bash"]
```

Track the integration bookmark once:

```sh
jj bookmark track dev --remote origin
```

Everyday commands:

```sh
jj st
jj --no-pager diff --git
jj --no-pager log
jj describe -m "Describe the change"
jj push       # advance dev to the latest described, non-empty change and push
jj pull       # fetch origin, update dev, and rebase onto it
```

Changes are snapshotted automatically; there is no staging area. `jj push` skips
empty or undescribed changes and forwards options such as `--dry-run` to
`jj git push`. `jj pull` rebases the working copy, so run it when shared work is
ready to move. The agent workflow is documented in
[the jujutsu skill](./.agents/skills/jujutsu/SKILL.md).

## Toolchain

Chord uses [proto](https://moonrepo.dev/proto) to install the versions of Bun and Rust pinned for the repository.

Install proto on macOS, Linux, or WSL:

```sh
bash <(curl -fsSL https://moonrepo.dev/install/proto.sh) 0.61.1 --yes
```

Restart your shell, then install the toolchain and project dependencies from the repository root:

```sh
proto install
bun install --frozen-lockfile
```

Start the desktop app with:

```sh
bun run dev
```

On macOS, the development runner builds with Tauri and uses Cargo's runner hook
to launch a signed, immutable `.app` snapshot through Launch Services. The executable
and embedded Bun dylib are copied out of the build directories; the copy's Bun
load command points directly to its bundled dylib. Rebuilding Chord or rbun while
the app is running cannot replace these files. Vite hot reload still works.
Running `bun run dev` inside `apps/chord-tauri` uses the same launcher and starts Vite.
Raw `cargo run` or `tauri dev` bypasses this launcher and runs the build output directly.

Snapshots live in `~/Library/Application Support/<development identifier>/runtime-snapshots/`,
outside Cargo's target directory. `current` is updated atomically after signing and
verification; CLI installation points through it so CLI links survive rebuilds.
Every app and CLI worker holds a shared runtime lease. Collection removes only
non-current snapshots whose lease can be acquired exclusively. Do not manually
remove a snapshot while it is in use. The runner forwards stop signals to the
specific launched app after verifying its PID's executable path.

The default signing identity comes from `bundle.macOS.signingIdentity`; override it
with `CHORD_DEV_SIGNING_IDENTITY` or `APPLE_SIGNING_IDENTITY`. A certificate-backed
identity is required; the launcher does not fall back to ad-hoc signing. Grant
Accessibility and Input Monitoring to **Chord (Development)** once after switching
to this launcher. Warp's grants no longer authorize the app. Chord continues to
stop interception whenever access is unavailable and resumes after a grant.
Permission logs retain denied/unknown/unexpected states and report executable
replacement separately, rather than claiming every failed check was a revocation.

Verify the snapshot launcher on a Mac with the configured signing certificate and
an active desktop session using
`bun test ./packages/dev/improve/chord/configs/dev/tauri/runtime.test.ts`.
The tests exercise signed executable/library isolation, generation leases,
PID-safe signaling, and Launch Services output/shutdown. Rust's runtime-worker
lease and permission-state tests are included in `cargo test --lib`.

Compare isolated package workers with the previous in-process Bun runtime using
`bun apps/chord-tauri/src-tauri/tests/package-isolation/benchmark.ts` from the root.
It builds optimized binaries and records paired latency, throughput, startup, and
resource measurements. See [the benchmark guide](./apps/chord-tauri/src-tauri/tests/package-isolation/README.md#performance-comparison)
for cases, options, and interpretation.

On macOS, a new development app launch replaces the running instance with the
same development bundle identifier. Startup logs the previous PID, sends SIGTERM,
and waits for it to exit before starting the new instance. If it ignores SIGTERM
for two seconds, startup verifies its socket ownership and sends SIGKILL. Concurrent launches
are serialized. CLI chord commands still forward to the running app, and
production/beta launches retain the existing instance.

### Input ownership across app instances

Each process registers at most one keyboard event tap and one Caps Lock HID listener.
All app channels share an OS-held input lease. Development bundle identifiers
(`com.leonsilicon.chord.development` and its dotted suffixes) take priority over
production and other channels. Among instances of the same priority, the current
owner stays active until it exits or becomes ineligible.

An instance must have Accessibility and Input Monitoring enabled and its input
handlers running before requesting ownership. Standby instances forward keyboard
events untouched and ignore Caps Lock input. On handoff, Chord drains in-flight
input processing, exits chord mode, clears held-key state, and rejects queued
input from the previous ownership generation. Ownership is checked every 25 ms;
a handoff also waits for any current input handler to finish.

Lease files live in the user's application-support directory under
`com.leonsilicon.chord/input-ownership-v1/`, shared across bundle identifiers and
independent of `TMPDIR`. **Do not delete these files while Chord is running.**
Their existence does not indicate ownership: the OS releases the locks even on
crash or force-quit, so a remaining lockfile cannot suppress another instance.
Ownership transitions appear in logs as `Input ownership: active` / `standby`.

Both dev and production must run a build containing this protocol. Older installed
builds do not participate and must be quit or updated before running alongside dev.

### Runtime log levels

While the development runner is open, focus the `tauri` pane and enter a log command:

```text
log debug
log info
log trace
log toggle
log status
```

Level changes take effect immediately and are persisted as `logLevel` in the development app's
state store. `RUST_LOG` remains a startup override and takes precedence over the persisted level.

The project tool versions are declared in [`.prototools`](./.prototools). Rust is also pinned in [`rust-toolchain.toml`](./rust-toolchain.toml) because proto delegates Rust version selection to rustup.

## Editing an installed chordpack locally

Chord's core `0` bindings live in the sibling checkout at
`../chords/@keychord/chords-chord/chords/com/leonsilicon/chord/macos.toml`.
Development builds and reloads read that file directly. Keep its distribution copy
at `apps/chord-tauri/data/chords-chord/chords/com/leonsilicon/chord/macos.toml`
in sync; builds without the sibling checkout embed that copy. The core file is
always loaded globally and cannot be disabled or replaced by another package source.

In Settings → Chords, choose **Link Local Folder** on an installed chord repo and
enter the absolute path to your local checkout. Its `package.json` name must match
the installed package. Links also work for pinned chordpacks and persist across
app restarts.

Chord loads the linked folder in place. After editing chords or JavaScript, click
**Reload** on the repo (or use the tray reload action) to reload configs and rebuild
the JS runtime. Sync is disabled while linked. **Unlink** restores the cached
version without changing your local files.

## Chord monorepos

In Settings → Chords → **Chord Monorepos**, add a GitHub repository or enter an
absolute local monorepo path and click the link icon. A GitHub monorepo can also
be linked to a local checkout using its link icon. **Add Repo** automatically
detects a monorepo when it contains `chords-*` directories. Chord discovers these
folders directly below the root or nested in other folders; no `packages/`
directory is required. Each matching folder is loaded as its own package, using
its `package.json` name or, when absent, its folder name. A matching folder must
contain `package.json` or a `chords/`, `js/`, or `bin/` directory; empty folders
and leftover build folders are ignored. Discovery stops at each package and
skips hidden directories (including `.git`, `.jj`, `.moon`, and `.build`),
`node_modules`, and `target`, so vendored dependencies and build outputs are
not treated as separate packages. Repos with their own root `chords/` folder
continue to load as standalone packages.

Local links persist across restarts. **Reload** rescans the monorepo, including
new or removed packages, and rebuilds the JS runtime. **Unlink** stops loading a
standalone local monorepo or restores the cached GitHub source for a linked repo;
it leaves local files intact.

Packages with the same name are overridden in this order (last wins): default
config packages, cached GitHub sources, standalone local monorepos, locally
linked GitHub sources, and individual local folders. All local sources therefore
take precedence over remote GitHub packages. Paths and repo slugs are sorted within each group for
consistent results. Duplicate package names within one monorepo are rejected.

## Stores

There can be many owners of Observables (e.g. the AppHandle needs to `.manage` it so we can read the current state when initializing a window, and certain structs should be able to own it in order to modify it).

## JavaScript runtime

Chord runs all JS handlers on Bun, embedded through the
[`rbun`](https://github.com/KeyChord/rbun) crate checked out next to this repo
at `../rbun`. Packages can use Bun and Node APIs and load native code through
Node-API add-ons with `process.dlopen`. The build links `libbun_embed.dylib`
from the rbun checkout, so build it once:

```sh
# one-time: build Bun and libbun_embed.dylib in the rbun checkout (~20 min cold)
(cd ../rbun && bun install && bun dev/improve/rbun/configs/bun/build/_build-bun.ts)

bun run dev
```

The runtime integration lives in `src-tauri/src/bun_js/` and exposes the
`chord` module to packages. The desktop app supervises one child process per
installed JavaScript package; it never executes package JavaScript or loads
package native add-ons in the GUI process. The hidden `__package-worker` CLI
command owns each persistent VM and its native UI event loop. State and keyboard
host APIs use authenticated, bounded IPC. A stable supervisor replaces crashed
workers and restores handler factories in registration order without configuration
reload. Handler IDs remain valid; failed actions are not replayed. Repeated failures
use exponential backoff from 250 ms to 30 seconds, resetting after a healthy minute. See the
[isolation regression](apps/chord-tauri/src-tauri/tests/package-isolation/README.md).
The CLI uses the same embedded runtime:

```sh
target/debug/chord bun ./script.ts
target/debug/chord run ./script.ts export-name arg1 arg2
```

## Releases

Every push to `beta` runs [`.github/workflows/release.yaml`](.github/workflows/release.yaml), which
builds the app for Apple Silicon and Intel and uploads the DMGs to the rolling
[`beta`](https://github.com/KeyChord/Chord/releases/tag/beta) prerelease. Beta uses the
`com.leonsilicon.chord.beta` application identifier, while local development uses
`com.leonsilicon.chord.development` and production uses `com.leonsilicon.chord`. This keeps each
channel's settings, packages, caches, logs, and other application-scoped data separate. Each run
replaces the previous assets, so the download URLs stay stable.

CI cannot build the vendored Bun itself — that takes ~30 minutes. Instead
[`KeyChord/rbun`](https://github.com/KeyChord/rbun) builds `libbun_embed.dylib` once per Bun source
commit and publishes it as a `bun-embed-<sha>` release asset; Chord's workflow downloads it and
points the build at it with `RBUN_BUN_LIB_DIR` plus a generated overlay config that overrides
`bundle.macOS.frameworks`. Both repos are checked out side by side so that the relative `rbun` path
in `Cargo.toml` resolves — the dylib satisfies the linker, but cargo still needs rbun's source
manifest. After bumping the Bun submodule in rbun, let its `build-bun-embed` workflow finish before
the next Chord release build.

### Code signing

Release builds are signed with the Developer ID certificate and notarized: `spctl -a -vv` on a
downloaded `.dmg` reports `source=Notarized Developer ID`, so the app opens by double-click with no
Gatekeeper prompt. The secrets below live at the organization level (they do not appear in
`gh secret list`, which shows only repository secrets). Each signing step is individually gated on
its secret, so the workflow still produces an ad-hoc signed build if one is ever removed.

| Secret | Effect |
| --- | --- |
| `APPLE_DEVELOPER_CERTIFICATE_FILE_BASE64`, `APPLE_DEVELOPER_CERTIFICATE_PASSWORD` | Signs with a Developer ID certificate instead of ad-hoc |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | Notarizes and staples the bundle |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Produces the signed updater artifact the in-app updater consumes |

The updater artifact is only built when `TAURI_SIGNING_PRIVATE_KEY` is set: Tauri fails the build if
an updater bundle is requested while `plugins.updater.pubkey` is configured without a private key.
Generate the keypair with `bun tauri signer generate -w ~/.tauri/chord.key`.
