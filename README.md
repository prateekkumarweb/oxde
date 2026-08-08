# Berth

**Run your own deploy platform.**

Berth is a self-hostable alternative to Vercel, Netlify, and Coolify, written in Rust, small enough to run on a Raspberry Pi. It gives you app/deployment management via a dashboard and JSON API, subdomain-based routing, zip-upload and git-based deploys (including a build step), and long-lived app processes run in rootless Podman containers.

## Features

- **Git deploys**: shallow clone straight from a repo URL and branch, no CI runner in between.
- **Zip upload**: drag a build folder into the dashboard when there's no repo to point at.
- **Run real apps**: not just static files, long-running processes run in their own containers.
- **Subdomain routing**: each app gets its own origin, isolated from every other app on the instance.
- **Build step**: set a build command and Berth runs it in a container before serving the output.
- **Dashboard & API**: everything the dashboard does is a call to the same JSON API you can script against.

## Requirements

- **Rust**, edition 2024 (a recent stable toolchain, install via [rustup](https://rustup.rs)).
- **[Vite+](https://viteplus.dev/guide/)** installed globally, so the `vp` command is on your `PATH`, used to build the dashboard frontend (`berth-ui/`). Vite+ manages the Node.js runtime and package manager (`pnpm`) for you.
- **`protoc`** (the Protocol Buffers compiler), needed to build `berth-proto`'s generated gRPC code - install via your package manager (e.g. `brew install protobuf`, `apt install protobuf-compiler`).
- **Podman** (rootless), reachable at its default local socket, needed to run git-sourced apps declared as a long-lived process ("run mode") or with a build step. Not required for zip-upload or static git deploys. Only `berth-agent` touches Podman - `berth-hub` never does.
  - On macOS, container IPs aren't reachable from the host by default; install [`podman-mac-net-connect`](https://github.com/AlmirKadric-Published/podman-mac-net-connect) to route to them for local testing.

## Configuration

`berth-hub` reads a TOML config file, `berth.toml` in the working directory by default (override with `$BERTH_CONFIG`). Copy [`berth.example.toml`](berth.example.toml) to `berth.toml` and adjust it, it documents every setting, required and optional, with comments.

`berth-agent` reads its own TOML config file, `berth-agent.toml` in the working directory by default (override with `$BERTH_AGENT_CONFIG`). Copy [`berth-agent.example.toml`](berth-agent.example.toml) to `berth-agent.toml` and adjust it, it documents every setting, required and optional, with comments.

## Build & run

Building always builds the dashboard frontend first, since `dashboard_assets.rs`'s `rust-embed` derive needs `berth-ui/dist` to exist at compile time.

```sh
cargo xtask build              # builds berth-ui/dist, then cargo build
cargo xtask build -- --release # release build
cargo xtask build-ui           # dashboard frontend only (vp install && vp build in berth-ui/)
cargo run -p berth-hub          # requires cargo xtask build-ui at least once first
cargo run -p berth-agent        # also required - see below
```

Berth is two binaries: `berth-hub` (dashboard, API, reverse proxy) and `berth-agent` (talks to Podman, runs containers). Both need to be running, on the same host - the hub dials the agent over gRPC at `127.0.0.1:50051`. `cargo xtask build` builds both, since it's a plain workspace build.

For production, run both under your init system of choice so they restart on crash and start on boot (e.g. two systemd units). Since `berth-agent` talks to *rootless* Podman, it needs to run as the same user whose Podman session it's using - a systemd *user* service (not a system-wide one) for that user is the natural fit.

Other useful commands:

```sh
cargo test              # run tests
cargo test <test_name>  # run a single test
cargo check             # check without building
cargo +nightly fmt      # format
cargo clippy            # lint
```

## Database migrations (`berth-db/`)

`berth-db/` holds Berth's SQLite-compatible database (`data_dir/berth.db`, via `turso`/`toasty`) and its schema, defined in `berth-db/src/models.rs`. Schema changes go through migrations rather than being pushed wholesale on every startup.

Berth applies any pending migration automatically on startup - there's nothing to run by hand to bring an existing `data_dir/berth.db` up to date.

When you change a model in `berth-db/src/models.rs`, generate the migration for it and commit the result:

```sh
cargo xtask migration generate --name describe_the_change
```

This diffs the new model shape against the last generated snapshot and writes the SQL migration, an updated schema snapshot, and a history entry under `toasty/` (`toasty/migrations/`, `toasty/snapshots/`, `toasty/history.toml`) - check all three into version control alongside the model change. `cargo xtask migration apply` runs the same apply step Berth runs at startup, useful for testing a migration without starting the server; `cargo xtask migration --help` lists the rest (`drop`, `reset`, `snapshot`), inherited from [`toasty-cli`](https://tokio-rs.github.io/toasty/0.8.0/guide/schema-management.html).

## Dashboard frontend (`berth-ui/`)

`berth-ui/` is a React 19 + TypeScript + Vite+ project with its own `package.json`/lockfile, not part of the Cargo workspace. `xtask/` (a real Cargo workspace member, aliased as `cargo xtask` via `.cargo/config.toml`) is what wires it into the Rust build above so it can't be forgotten.

To work on it directly:

```sh
vp install   # install dependencies, run after cloning and whenever package.json/lockfile change
vp dev       # start the Vite dev server with hot reload
vp build     # type-check (tsc) and produce a production build
vp preview   # preview a production build
vp check     # format, lint, type-check
vp test      # run tests
```

If setup, runtime, or package-manager behavior looks wrong, run `vp env doctor`. Run `vp help` for the full command list, or `vp <command> --help` for details on a specific one. Docs are local at `node_modules/vite-plus/docs` or online at https://viteplus.dev/guide/.

## License

MIT, see [`LICENSE`](LICENSE).
