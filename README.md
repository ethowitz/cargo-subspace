# cargo subspace

## tl;dr

This tool forces rust-analyzer to lazily index crates in the workspace as you open new files. It
is useful if you have a very big cargo workspace and you find that rust-analyzer often slows to a
crawl.

## What the heck is this

This tool exists to improve the rust-analyzer experience for large cargo workspaces.
Cargo workspaces can contain a (theoretically) unbounded number of crates. Many organizations
prefer to maintain a single cargo workspace (e.g. in a monorepo) to keep dependency versions
consistent across different services or libraries and simplify tooling. However, rust-analyzer
doesn't handle this well--it indexes the *entire* cargo workspace eagerly.
`rust-analyzer.check.workspace=false` improves this slightly by running `cargo check` only on
the crate currently being worked on, but it doesn't prevent rust-analyzer from indexing code and
building proc macros for the whole workspace when it first starts up.

Rather than allowing rust-analyzer to discover all the crates in the workspace at startup, this
tool tells rust-analyzer about the crates in your workspace selectively as you open new files. It
invokes `cargo metadata` to get the dependency graph for the workspace and then prunes the graph
such that only 1) the crate that owns the current file and 2) that crate's dependencies remain in
the graph. This is supported by rust-analyzer's "rust-project.json" feature, which allows
rust-analyzer to use third party build tools (e.g. bazel or buck) to discover crates in your
project. (This project still uses cargo under the hood, but it integrates into rust-analyzer
through this path.)

## Caveat

Note that, because crates are indexed lazily as you open source code files, you will not be able to
perform any actions that require knowledge of the **dependents** of the current crate (that is, the
crates in your workspace that *depend on* the current crate). Some examples include:

- Searching for a symbol in a crate for which you've not yet opened any source code files
- Finding references to symbols (e.g. functions, types, etc.) defined in the current crate in
  crates that *depend on* the crate (unless you've already opened a file from a dependent crate)

## Installation

First, make sure that the `rust-src` component is installed for your rust toolchain. This downloads
the source code for the crates built-in to rust (e.g. `std` and `core`) so rust-analyzer can
properly index them.

```sh
rustup component add rust-src
```

Then:

```sh
cargo install --locked cargo-subspace
```

## Configuration

This tool is designed to be invoked directly by your editor. I've tested it with both VSCode and
neovim, but theoretically, it should work with any editor that has LSP support.

### VSCode

Add the following to your `settings.json`:

```json
{
  "rust-analyzer.workspace.discoverConfig": {
    "command": [
        "cargo-subspace",
        "discover",
        "{arg}"
    ],
    "progressLabel": "cargo-subspace",
    "filesToWatch": [
        "Cargo.toml"
    ]
  },
  "rust-analyzer.check.overrideCommand": [
    "cargo-subspace",
    "check", // You can also use "clippy" here
    "$saved_file",
  ],
}
```

### neovim

These settings should be set wherever you configure your LSP servers in your neovim config. I use 
the great [rustaceanvim](https://github.com/mrcjkb/rustaceanvim) plugin, but these settings can
also be set via lspconfig.

```lua
["rust-analyzer"] = {
  check = {
    overrideCommand = {
      "cargo-subspace",
      "clippy",
      "$saved_file",
    },
  },
  workspace = {
    discoverConfig = {
      command = {
        "cargo-subspace",
        "discover",
        "{arg}",
      },
      progressLabel = "cargo-subspace",
      filesToWatch = {
        "Cargo.toml",
      },
    },
  },
}
```

## Troubleshooting/Debugging

If you run into trouble, please feel free to open an issue with the following:

- A detailed description of the problem, including steps to reproduce
- Your rust toolchain version
- Your `cargo-subspace` version
- Verbose logs from the errant invocation of this tool (you can collect verbose logs by
  running `cargo-subspace` with the `--verbose` flag). By default, logs are stored in
  `$HOME/.local/state/cargo-subspace/cargo-subspace.log`

You may also feel free to open an issue if you have a feature request. Provided the feature makes
sense and is not too involved, I would be happy to consider it. I'll also accept pull requests if
you're feeling inspired to implement it yourself.

**NOTE:** This project is currently untested on Windows. Please feel free to test it and tell me
about your experience!
