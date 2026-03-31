#!/usr/bin/env node
/**
 * IKR MCP Server CLI — run via stdio for Claude Desktop / Claude Code.
 *
 * Usage:
 *   npx @life/ikr-mcp-server        # stdio transport
 *   ikr-mcp                          # if installed globally
 */
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { createIKRServer } from "./server.js";

async function main() {
	const server = createIKRServer();
	const transport = new StdioServerTransport();
	await server.connect(transport);
}

main().catch((err) => {
	console.error("IKR MCP Server failed:", err);
	process.exit(1);
});
