#!/usr/bin/env python3
import sys
import json
import logging

# Configure logging to stderr so it doesn't interfere with stdout JSON-RPC
logging.basicConfig(stream=sys.stderr, level=logging.DEBUG, format='[MOCK_MCP] %(message)s')

def main():
    logging.info("Starting mock MCP server")

    while True:
        try:
            line = sys.stdin.readline()
            if not line:
                break

            line = line.strip()
            if not line:
                continue

            logging.info(f"Received: {line}")
            req = json.loads(line)
            req_id = req.get("id")
            method = req.get("method")

            resp = {
                "jsonrpc": "2.0",
                "id": req_id
            }

            if method == "initialize":
                resp["result"] = {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "serverInfo": {"name": "mock-server", "version": "1.0"}
                }
            elif method == "notifications/initialized":
                # Notification, no response
                continue
            elif method == "tools/list":
                resp["result"] = {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Echoes back the input",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "text": {"type": "string", "description": "Text to echo"}
                                },
                                "required": ["text"]
                            }
                        },
                        {
                            "name": "add",
                            "description": "Adds two numbers",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "a": {"type": "number"},
                                    "b": {"type": "number"}
                                },
                                "required": ["a", "b"]
                            }
                        }
                    ]
                }
            elif method == "tools/call":
                params = req.get("params", {})
                name = params.get("name")
                args = params.get("arguments", {})

                if name == "echo":
                    text = args.get("text", "")
                    resp["result"] = {
                        "content": [{"type": "text", "text": str(text)}]
                    }
                elif name == "add":
                    a = float(args.get("a", 0))
                    b = float(args.get("b", 0))
                    # Check if integer
                    if a.is_integer() and b.is_integer():
                         res = int(a + b)
                    else:
                         res = a + b
                    resp["result"] = {
                        "content": [{"type": "text", "text": str(res)}]
                    }
                else:
                    resp["error"] = {"code": -32601, "message": f"Tool not found: {name}"}
            else:
                 # Method not found
                 if req_id is not None:
                    resp["error"] = {"code": -32601, "message": f"Method not found: {method}"}
                 else:
                    continue

            resp_str = json.dumps(resp)
            logging.info(f"Sending: {resp_str}")
            sys.stdout.write(resp_str + "\n")
            sys.stdout.flush()

        except json.JSONDecodeError:
            logging.error("Invalid JSON received")
        except Exception as e:
            logging.error(f"Error processing request: {e}")
            # Try to send error if possible
            # ...

if __name__ == "__main__":
    main()
