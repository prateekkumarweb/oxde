# OxDe UI

The OxDe dashboard frontend built with [Vite+](https://viteplus.dev).

## Prerequisites

- [Vite+](https://viteplus.dev/guide/) installed globally, so the `vp` command is on your `PATH`. Vite+ manages the Node.js runtime and package manager (`pnpm`) for you.

## Install

```sh
vp install
```

Run this after cloning and any time you pull changes that touch `package.json` or the lockfile.

## Develop

```sh
vp dev
```

Starts the Vite dev server with hot reload.

## Build

```sh
npm run build
```

Type-checks (`tsc`) and produces a production build via `vp build`.

## Preview a production build

```sh
vp preview
```

## Check & test

```sh
vp check   # format, lint, type-check
vp test    # run tests
```

## Troubleshooting

If setup, runtime, or package-manager behavior looks wrong:

```sh
vp env doctor
```

Run `vp help` for the full command list, or `vp <command> --help` for details on a specific one.

Docs are local at `node_modules/vite-plus/docs` or online at https://viteplus.dev/guide/.
