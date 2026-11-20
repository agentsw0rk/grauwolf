# ADR-0122: SPIFFE federation – agents of other trust domains

- **Status:** Proposed (User, 2026-11-15: "Q5 has the option for federation later – note it as its
  own ADR and feature; I want to have it later"). Not scheduled yet.
- **Date:** 2026-11-15
- **Related:** ADR-0109 (SPIFFE for agents; decision Q5 of 2026-11-15: only the own trust domain by
  default, `GRAUWOLF_SPIFFE_TRUST_DOMAINS` reserved), ADR-0120 (`grauwolf-k8-ops`; it refuses a
  `trust-domain set` on a server with federation today), ADR-0111 (delegation).

## Context
With ADR-0109, agents check each other within one trust domain: the SPIRE server issues the SVIDs,
and the trust bundle holds that domain's CA and JWT keys. Agents in another cluster, or another
organisation's SPIRE, belong to another trust domain. A service can accept their SVIDs only if it has
that domain's bundle. **SPIFFE federation** distributes such bundles:
- Each SPIRE server publishes its bundle at a *bundle endpoint* and fetches the bundles of the domains
  it federates with.
- `ClusterFederatedTrustDomain` objects of spire-controller-manager configure this.
- Registration entries name the domains a workload may trust (`federatesWith`).
- The Workload API then hands the workload the foreign bundles too.

## Decision (to be refined when it is scheduled)
1. **grauwolf side.**
   - `GRAUWOLF_SPIFFE_TRUST_DOMAINS` lists the foreign trust domains a service accepts callers from.
     It is empty by default, meaning the own domain only.
   - On the client side, `auth.spiffe.trust_domain` or an `id` may name a foreign domain.
   - Bundles come from the Workload API's federated bundles. grauwolf fetches no bundle endpoint
     itself.
   - The mapping file of ADR-0109 assigns foreign SPIFFE IDs to users like own ones.
   - An ID from a domain that is not listed is refused with `403`, even if its bundle is known.
2. **Cluster side (`grauwolf-k8-ops`).**
   - Commands to manage federation: `federation add <domain> --bundle-endpoint <url> [--profile
     https_spiffe|https_web]`, plus `federation list` and `federation remove`. They create the
     `ClusterFederatedTrustDomain`, set `federatesWith` on the agents' `ClusterSPIFFEID`, and
     publish the own bundle endpoint (server config `federation { bundle_endpoint { … } }`, a Service,
     optionally an Ingress).
   - `doctor` checks that the foreign bundles are present.
   - Changing the trust domain with federation configured needs a decision of its own: today
     `trust-domain set` refuses it, and that stays until this ADR is implemented.
3. **Tests.** Fast tests with two test trust domains and the fake Workload API. A live check needs a
   second trust domain, for example a second SPIRE server in its own namespace of the owner's cluster.

## Consequences
- Agents of other clusters or partners can call grauwolf agents with their own identities, and vice
  versa, without shared secrets.
- Trust becomes an explicit list per service; a foreign domain is never accepted implicitly.
- Open questions to settle before implementing:
  - how a foreign domain's ID is mapped by default (no default: every ID needs an entry);
  - whether delegation (ADR-0111) crosses domains;
  - bundle endpoint exposure and its TLS (SPIFFE or web PKI profile).
