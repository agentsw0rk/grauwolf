# grauwolf

grauwolf runs agents described by a spec – interpreted at run time or generated as Rust code – with
tools, knowledge, memory and sub-agents.

## Agent specs

**Agent tree**:
A spec file together with the specs of its sub-agents, recursively; a sub-agent reached over A2A is
a leaf.
_Avoid_: spec hierarchy, agent graph (the graph is inside one spec)

**Prepared agent tree**:
An agent tree whose sources are loaded and resolved – code files read, skills localised, tool
packages resolved against the lockfile – ready for provisioning.
_Avoid_: resolved spec, loaded bundle

## Running an agent

**Agent run**:
One run of an agent from its input to its result, opened by a start event and closed by a completed
or failed event; automatic memory and knowledge belong to it.
_Avoid_: execution, invocation, session

**Provisioning**:
Turning a loaded, resolved spec plus its environment into everything an agent needs to run: model
access, decider, tools from outside the graph and memory.
_Avoid_: setup, wiring, bootstrapping

**Provisioned agent**:
The result of provisioning one agent, built once per loaded agent and shared by all its runs.
_Avoid_: agent bundle, runtime kit

**Provisioned run**:
The view of a provisioned agent for one run and one user: the user's own tools and memory facts.
_Avoid_: session, user context

## Tools

**Tool namespace**:
The set of all tool names an agent offers, whatever their origin – own tools, MCP, knowledge,
memory, built-in groups, skills, sub-agents, packages, code pieces; every name occurs once.
_Avoid_: tool registry (that is the runtime collection), tool list

**Built-in tool group**:
A family of tools that ship with grauwolf and are switched on together in `builtin_tools`, such as
the file tools, the key-value store or the Jira tools.
_Avoid_: builtin module, tool pack (a tool package is something else)

