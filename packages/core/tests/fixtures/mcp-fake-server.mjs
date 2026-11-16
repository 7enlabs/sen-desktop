// Fake MCP server for tests (Sen P0-5). Speaks JSON-RPC 2.0 over
// stdio, newline-delimited, like a real MCP server. Tools:
// - echo: returns the arguments back as text.
// - add: returns a + b as text (numbers, defaults 0).
// - fail: returns a tool result with isError: true.
// - slow: never replies (drives the client timeout path).
// Unknown tools get a JSON-RPC error -32602. Unknown methods get
// -32601. This file is test-only: it is the faked server boundary,
// never the code under test.
import { createInterface } from "node:readline";

const rl = createInterface({ input: process.stdin, terminal: false });

function send(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

rl.on("line", (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  if (!msg || msg.jsonrpc !== "2.0") {
    return;
  }
  // Notification (no id): nothing to answer.
  if (typeof msg.method === "string" && msg.id === undefined) {
    return;
  }
  const id = msg.id;
  const reply = (result) => send({ jsonrpc: "2.0", id, result });
  const replyError = (code, message) =>
    send({ jsonrpc: "2.0", id, error: { code, message } });

  switch (msg.method) {
    case "initialize":
      reply({
        protocolVersion: "2024-11-05",
        capabilities: { tools: {} },
        serverInfo: { name: "sen-fake-mcp", version: "0.0.0" },
      });
      return;
    case "ping":
      reply({});
      return;
    case "tools/list":
      reply({
        tools: [
          {
            name: "echo",
            description: "Echo the arguments back as text",
            inputSchema: { type: "object", properties: {} },
          },
          {
            name: "add",
            description: "Add a and b",
            inputSchema: { type: "object", properties: {} },
          },
          {
            name: "fail",
            description: "Always fails as a tool error",
            inputSchema: { type: "object", properties: {} },
          },
          {
            name: "slow",
            description: "Never replies",
            inputSchema: { type: "object", properties: {} },
          },
          {
            name: "read_file",
            description: "Echo the arguments back (auto-tier stand-in)",
            inputSchema: { type: "object", properties: {} },
          },
        ],
      });
      return;
    case "tools/call": {
      const name = msg.params?.name;
      const args = msg.params?.arguments ?? {};
      if (name === "echo") {
        reply({
          content: [{ type: "text", text: JSON.stringify(args) }],
          isError: false,
        });
        return;
      }
      if (name === "add") {
        const sum = (Number(args.a) || 0) + (Number(args.b) || 0);
        reply({
          content: [{ type: "text", text: String(sum) }],
          isError: false,
        });
        return;
      }
      if (name === "fail") {
        reply({
          content: [{ type: "text", text: "boom" }],
          isError: true,
        });
        return;
      }
      if (name === "slow") {
        return; // never replies: the client must time out
      }
      if (name === "read_file") {
        reply({
          content: [{ type: "text", text: JSON.stringify(args) }],
          isError: false,
        });
        return;
      }
      replyError(-32602, `Unknown tool: ${String(name)}`);
      return;
    }
    default:
      replyError(-32601, `Method not found: ${String(msg.method)}`);
  }
});
