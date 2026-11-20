# Security policy

grauwolf runs AI agents that call tools, reach networks and handle credentials, so we take security
reports seriously and thank everyone who reports a problem responsibly.

## Reporting a vulnerability

**Please do not report security issues in public GitHub issues, discussions or pull requests.**

Report them privately instead:

1. Preferably through GitHub's
   [private vulnerability reporting](https://github.com/agentsw0rk/grauwolf/security/advisories/new)
   ("Report a vulnerability" in the *Security* tab), or
2. by e-mail to `<security contact e-mail>`.

Please include what is affected (component, version or commit), how to reproduce it, the impact you
see, and whether it is already known publicly.

## What happens next

- We acknowledge the report within **5 working days**.
- We assess it, keep you informed and agree on a disclosure date; we aim to fix critical issues
  within **90 days**.
- Fixes are released with a GitHub security advisory and, where applicable, a CVE. Reporters are
  credited unless they prefer otherwise.

## Supported versions

grauwolf is a 1.0 pre-release; security fixes go into `main` and the next release. Once 1.0 is out,
this section will name the supported release lines.

## Scope

In scope are the code and images of this repository: the runtime and its sandbox for tool code,
the CLI and the programs `grauwolf-tools`, `grauwolf-k8-ops` and `grauwolf-studio`, the identity
service and the integrations with SPIFFE/SPIRE and OpenBao. Vulnerabilities in third-party
dependencies are welcome too; we coordinate with their maintainers.

Design notes on the security model are in the ADRs, in particular
[ADR-0109](docs/adr/0109-workload-identity-with-spiffe.md),
[ADR-0110](docs/adr/0110-workload-identity-for-tools-and-credentials.md) and
[ADR-0111](docs/adr/0111-delegation-with-transaction-tokens.md).
