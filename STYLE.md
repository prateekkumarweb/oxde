# Coding Style Guide

## Locality of behavior

Strive to keep related code together. A struct is immediately followed by its inherent `impl` block and then its trait impls, before the next struct is declared in the file.

## Dependencies

Declare every dependency's version once, in `[workspace.dependencies]` at the repo root `Cargo.toml`, with no features. Individual crates depend on it with `workspace = true` and add whatever features *they* need:

```toml
# root Cargo.toml
[workspace.dependencies]
tokio = "1.53.1"

# some-crate/Cargo.toml
[dependencies]
tokio = { workspace = true, features = ["full"] }
```

Never pin a version directly in a member crate's `Cargo.toml`.

## Characters

Write comments and documentation using only characters found on a US layout keyboard. Avoid special characters such as em dashes, Unicode arrows, or the ellipsis. Use their plain-ASCII equivalents instead. This does not apply to user-facing UI text, where the correct typographic character is fine:

- `-` or `--` instead of an em dash (`—`)
- `->` instead of a Unicode arrow (`→`)
- `...` (three periods) instead of a single ellipsis character (`…`)

## Tests

Unit tests (`#[cfg(test)] mod tests`) always go at the very bottom of the file.

## References

- https://github.com/tokio-rs/topcoat/blob/main/STYLE.md
