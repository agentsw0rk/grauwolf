# Maintainers

This file lists the maintainers of grauwolf, their areas and how to reach them. Maintainers review
and merge changes, cut releases, and take part in decisions as described in
[GOVERNANCE.md](GOVERNANCE.md).

## Current maintainers

| Name | GitHub | Affiliation | Areas |
|---|---|---|---|
| `<Full Name>` | [`@<github-handle>`](https://github.com/<github-handle>) | `<Affiliation>` | Lead maintainer – all areas: runtime, code generation, tools, identity (SPIFFE, OpenBao, delegation), `grauwolf-k8-ops`, grauwolf studio, documentation |

## Areas

The repository is organised in layers (see [docs/architecture.md](docs/architecture.md)). Until
more maintainers join, the lead maintainer owns every area; new maintainers are added per area:

| Area | Paths |
|---|---|
| Domain core | `crates/domain/` |
| Adapters (providers, tools, MCP, A2A, storage, identity) | `crates/adapters/` |
| Programs (CLI, `grauwolf-tools`, `grauwolf-k8-ops`, studio backend) | `crates/apps/` |
| grauwolf studio frontend | `studio/web/` |
| Documentation and ADRs | `docs/`, `README.md` |
| Build, CI and images | `xtask/`, `.github/`, `docker/`, `scripts/` |

## Emeritus maintainers

None yet.

## Contact

- Questions and proposals: [GitHub issues](https://github.com/agentsw0rk/grauwolf/issues)
- Security issues: see [SECURITY.md](SECURITY.md) – never in public issues
- Code of conduct: see [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
- Maintainers: `<maintainers contact e-mail>`
