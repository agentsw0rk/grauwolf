"""Minimal MCP server over stdio for tests: one tool `upper` that upper-cases text."""
import json
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
            "serverInfo": {"name": "fake-stdio", "version": "1.0"},
        }
    elif method == "tools/list":
        result = {"tools": [{
            "name": "upper",
            "description": "Upper-cases text",
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
        }]}
    elif method == "tools/call":
        text = request["params"]["arguments"]["text"]
        result = {"content": [{"type": "text", "text": text.upper()}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
