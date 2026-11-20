"""Minimal MCP server over stdio for tests: one tool `env` that returns the value of an
environment variable of the server process (or `<unset>`)."""
import json
import os
import sys

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {
            "protocolVersion": request["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "env-stdio", "version": "1.0"},
        }
    elif method == "tools/list":
        result = {"tools": [{
            "name": "env",
            "description": "Value of an environment variable",
            "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}},
        }]}
    elif method == "tools/call":
        value = os.environ.get(request["params"]["arguments"]["name"], "<unset>")
        result = {"content": [{"type": "text", "text": value}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
