# OxDe

**Your infrastructure. Your deploys.**

OxDe is a self-hostable alternative to Vercel, Netlify, and Coolify, written in Rust and designed to stay light on resources, from a spare server to a Raspberry Pi. It gives you app/deployment management via a dashboard and JSON API, subdomain-based routing, zip-upload and git-based deploys (including a build step), and long-lived app processes run in rootless Podman containers.

## Features

- **Git deploys**: point OxDe at a repository and it takes care of the rest.
- **Zip upload**: no repo yet? Upload a build folder straight from the dashboard.
- **Run real apps**: not just static files, long-running processes run in their own containers.
- **Subdomain routing**: every app gets its own subdomain and its own origin.
- **Build step**: run a build command before deploy, instead of shipping pre-built output.
- **Dashboard & API**: manage every app and deployment from the dashboard, or script it against the JSON API.

## Requirements

- **Rust**, edition 2024 (a recent stable toolchain, install via [rustup](https://rustup.rs)).
- **[Vite+](https://viteplus.dev/guide/)** installed globally, so the `vp` command is on your `PATH`, used to build the dashboard frontend (`oxde-ui/`). Vite+ manages the Node.js runtime and package manager (`pnpm`) for you.
- **Podman** (rootless), reachable at its default local socket, needed to run git-sourced apps declared as a long-lived process ("run mode") or with a build step. Not required for zip-upload or static git deploys.
  - On macOS, container IPs aren't reachable from the host by default; install [`podman-mac-net-connect`](https://github.com/AlmirKadric-Published/podman-mac-net-connect) to route to them for local testing.

## Configuration

OxDe reads a TOML config file, `oxde.toml` in the working directory by default (override with `$OXDE_CONFIG`). Copy [`oxde.example.toml`](oxde.example.toml) to `oxde.toml` and adjust it, it documents every setting, required and optional, with comments.

## Build & run

Building always builds the dashboard frontend first, since `dashboard_assets.rs`'s `rust-embed` derive needs `oxde-ui/dist` to exist at compile time.

```sh
cargo xtask build              # builds oxde-ui/dist, then cargo build
cargo xtask build -- --release # release build
cargo xtask build-ui           # dashboard frontend only (vp install && vp build in oxde-ui/)
cargo run                      # requires cargo xtask build-ui at least once first
```

Other useful commands:

```sh
cargo test              # run tests
cargo test <test_name>  # run a single test
cargo check             # check without building
cargo +nightly fmt      # format
cargo clippy            # lint
```

## Dashboard frontend (`oxde-ui/`)

`oxde-ui/` is a React 19 + TypeScript + Vite+ project with its own `package.json`/lockfile, not part of the Cargo workspace. `xtask/` (a real Cargo workspace member, aliased as `cargo xtask` via `.cargo/config.toml`) is what wires it into the Rust build above so it can't be forgotten.

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
