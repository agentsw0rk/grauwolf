# E12 – Testing


| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F12.1 | Mock PromptExecutor with a DSL (`mockLLMAnswer("..") onRequestContains "..")`, `mockLLMToolCall(tool, args)`) | `MockExecutor::builder().answer("..").when_contains("..").tool_call::<T>(args).when(..)` | M1 |
| F12.2 | Mock tools / tool mocking (return value per args) | `MockTool`, recording of the calls | M1 |
| F12.3 | Graph structure tests (node order, edges, check reachable paths) | `strategy.testing().assert_edge(a, b)`, test node calls in isolation | M2 |
| F12.4 | Run individual nodes in isolation (with a dummy context) | `TestContext::new()` | M1 |
| F12.5 | Event recording for assertions | `RecordingFeature` collects `AgentEvent`s | M1 |
| F12.6 | HTTP mocks for provider clients | `wiremock` / fixture JSONs per provider | M1 |
| F12.7 | Integration tests against real providers (opt-in via env, Ollama in CI) | `#[ignore]` + env gate; Ollama via Docker in CI | M2 |
