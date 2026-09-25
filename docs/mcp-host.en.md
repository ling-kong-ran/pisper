# Let other agents use Pisper

Pisper's **built-in MCP server** exposes the running Pisper Runtime to other agents on the same computer. This is separate from adding an external MCP server inside Pisper: the former lets another agent connect to Pisper; the latter lets Pisper connect to another service.

The server is off by default. Open **MCP → Built-in MCP server** in Pisper, enable it, then copy the connection settings into the other agent's MCP client. Keep the Pisper Runtime running. The on-device Android and iOS Runtimes do not currently provide this server.

## Connect

The client must support **MCP Streamable HTTP** and a custom `Authorization` request header. The server listens only on the local loopback interface. Its default address is `http://127.0.0.1:5175/mcp`; another computer or phone cannot connect directly. Use the address shown in Pisper if it differs.

This is a generic configuration example. Client schemas vary; enter the address and token shown by Pisper in that client's MCP server settings:

```json
{
  "mcpServers": {
    "pisper": {
      "url": "http://127.0.0.1:5175/mcp",
      "headers": {
        "Authorization": "Bearer <token copied from Pisper>"
      }
    }
  }
}
```

If the connection fails, check that Pisper is still running, the server status says it is listening, and the client uses Streamable HTTP with the complete `Bearer ` prefix. If another program occupies port 5175, Pisper reports that in the server status. Free the port and toggle the server off and on to retry.

## Available capabilities

The server currently exposes 21 tools. An external agent can discover their names and parameters before calling them:

| Area | Capabilities |
| --- | --- |
| Sessions and messages | List sessions, read messages or live state, create and rename sessions, send messages, poll a run, stop an active turn |
| Goals | Read or pause a session goal |
| Memory | Search relevant memory |
| Workflows and schedules | List and run workflows, inspect or stop a workflow run, list and run an existing scheduled task now |
| Status and assets | Read Runtime capabilities and today's usage, search assets, inspect tracked session file changes |

`pisper_send_message` returns a run ID immediately. The external agent should poll `pisper_get_run` for its result. If the run is waiting for approval, return to Pisper to decide. Tools executed inside a conversation still follow that session's permissions and approval settings. Workflows and scheduled tasks follow their existing execution rules. This server does not offer tools to read Provider secrets, approve permission requests, or call arbitrary Pisper internals.

## Token and shutdown

**A local program holding the connection token can call every exposed tool**, including reading sessions and memory, sending messages, and starting workflows. Give the token only to clients you trust. Do not commit token-bearing settings to a repository or expose them in screenshots, logs, or chat. Pisper stores the token and enable switch in its Agent data directory; `PISPER_AGENT_DIR` moves them with that directory.

Turning the server off on the MCP page stops the listener and aborts conversation turns still running through this server. You can enable it again later. If a token may have leaked, rotate it on the same page and update each client's configuration. The old token stops working immediately and existing connections are closed. The enable switch persists across restarts; if it was on, the Pisper Runtime starts listening again when it launches.

This is a local integration. Desktop remote access device tokens cannot replace the built-in MCP token, and the built-in server is not exposed through Desktop remote access.
