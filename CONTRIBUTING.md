# Contributing to grauwolf

Thank you for your interest in grauwolf! Contributions of all kinds are welcome: bug reports, ideas,
documentation, examples, reviews and code.

## Before you start

- For larger changes, **open an issue first** so the approach can be discussed.
- Read the related [architecture decision records](docs/adr/README.md) before changing a design. A
  change of design is recorded as a new ADR in the same pull request.
- The [developer guide](docs/developer-guide/README.md) explains the architecture, the layers and how
  to extend grauwolf; [docs/architecture.md](docs/architecture.md) the dependency rules.
- Please follow the [code of conduct](CODE_OF_CONDUCT.md).
- This repository is produced by an export script from the maintainers' private development repository (one commit per export, without the Enterprise feature grauwolf studio's code); accepted contributions are applied there and show up in the next export.

## Development setup

You need Rust 1.85 or newer (edition 2024) with `clippy`, `rustfmt` and the
`wasm32-unknown-unknown` target. In a fresh Debian/Ubuntu environment,
`scripts/dev-container-setup.sh` installs everything (also the frontend test bindings and `~/.grauwolf-env`; `--check` only reports). The studio frontend (`studio/web/`, Enterprise, only in the private repository) also needs
Node.js 22 or newer; `scripts/studio.sh` builds and starts the studio.

## How we work

- **Test first.** One test case per cycle: write the test, make it pass, refactor. A commit contains
  the test, the implementation and the documentation of one behaviour.
- **Small commits** with clear messages in the imperative ("Add …", "Fix …") that explain why.
- **Checks before every commit:**

  ```sh
  cargo fmt --all
  cargo xtask check --affected   # clippy -D warnings, fast tests of what changed, architecture test
  ```

  and before a pull request is merged:

  ```sh
  cargo xtask check --full       # the whole workspace, including slow tests and the feature matrix
  ```

  For changes in `studio/web/`: `npm run lint && npm test && npm run check-api && npm run build`.
- **Documentation** is part of the change: the user and developer guides (`docs/user-guide/`,
  `docs/developer-guide/`), the README for new features, and the crate's `AGENTS.md`. Example specs
  in the guides are tested.
- **Language:** code, comments, messages, identifiers and documentation are in English. The project
  name is always written in lowercase: `grauwolf`.

## Developer Certificate of Origin (DCO)

All commits must be signed off to certify that you wrote the change or have the right to submit it
under the project's license, as described in the
[Developer Certificate of Origin](https://developercertificate.org/):

```sh
git commit -s -m "Add …"
```

This adds a line `Signed-off-by: Your Name <your@email>` to the commit message.

## Pull requests

1. Fork the repository and create a branch from `main`.
2. Make your change with tests and documentation; run the checks above.
3. Open a pull request that explains what changes and why, and links the issue and any ADR.
4. A maintainer reviews it; address the comments. Once approved and green, a maintainer merges it.

## Reporting bugs and security issues

- Bugs and feature requests: [GitHub issues](https://github.com/agentsw0rk/grauwolf/issues), with
  the grauwolf version (`grauwolf version`), the spec or a minimal example, and what you expected.
- **Security issues must not be reported in public issues** – see [SECURITY.md](SECURITY.md).

## License

By contributing you agree that your contributions are licensed under the
[Apache License, Version 2.0](LICENSE).
