# E10 – Planner agents


| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F10.1 | Planner abstraction (plan → execute step → replan) as a strategy | `trait Planner<State>`; usable as a strategy | M3 |
| F10.2 | LLM-based planner | Plan as structured output (E01) | M3 |
| F10.3 | LLM planner with critic (plan evaluation/improvement) | Second LLM call | M3 |
| F10.4 | GOAP: actions (preconditions, effects, cost), goals, A* search | Builder; state generic `S: Clone + Eq + Hash`; A* own implementation or `pathfinding` crate | M3 |
| F10.5 | Custom cost functions | Closure `Fn(&S) -> f64` | M3 |
| F10.6 | Belief state vs. actual execution (replan on deviation) | Replan loop | M3 |
| F10.7 | Planner events in the pipeline | Event variants in E06 | M3 |

## Rust specifics
GOAP can be implemented very cleanly and quickly in Rust (state as a value type, A* without GC). A good candidate for a later "showcase" feature.
