# ADR-0023: Streaming is a side channel

- **Status:** Accepted (review iteration 1, 2026-11-06)
- **Date:** 2026-11-06

## Context
A `Stream<StreamFrame>` is neither serializable nor checkpointable, but node data must be (ADR-0010).

## Decision
Streaming nodes write frames into a frame sink of the context and produce streaming events. Their node output is always the aggregated result (`Message`). Streams never travel over edges.

## Consequences
Checkpoints remain possible; live output goes through events and the sink.
