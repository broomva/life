/**
 * IKR MCP Server — Streamable HTTP + stdio transport.
 */
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import {
	computeLayout,
	validateConstraints,
	repairLayoutTool,
	suggestVariants,
	measureText,
} from "./tools.js";

// Zod doesn't have a recursive schema helper that's easy to use here,
// so we accept the spec as a JSON object and cast it.
const jsonObject = z.record(z.unknown());

const constraintsSchema = z.object({
	width: z.number(),
	height: z.number().optional(),
	lineHeight: z.number().optional(),
	surface: jsonObject,
});

export function createIKRServer(): McpServer {
	const server = new McpServer({
		name: "ikr-layout",
		version: "0.0.1",
	});

	server.tool(
		"compute_layout",
		"Solve layout for a UI spec at given constraints. Returns positions, sizes, and line counts for all nodes.",
		{
			spec: jsonObject,
			constraints: constraintsSchema,
		},
		async ({ spec, constraints }) => {
			const result = computeLayout({
				spec: spec as any,
				constraints: constraints as any,
			});
			return {
				content: [
					{ type: "text", text: JSON.stringify(result, null, 2) },
				],
			};
		},
	);

	server.tool(
		"validate_constraints",
		"Solve layout and validate against constraint rules. Returns violations with repair suggestions.",
		{
			spec: jsonObject,
			constraints: constraintsSchema,
		},
		async ({ spec, constraints }) => {
			const result = validateConstraints({
				spec: spec as any,
				constraints: constraints as any,
			});
			return {
				content: [
					{ type: "text", text: JSON.stringify(result, null, 2) },
				],
			};
		},
	);

	server.tool(
		"repair_layout",
		"Apply two-tier repair (deterministic rules + optional LLM) to fix constraint violations. Returns the repaired spec and solved layout.",
		{
			spec: jsonObject,
			constraints: constraintsSchema,
			maxIterations: z.number().optional(),
		},
		async ({ spec, constraints, maxIterations }) => {
			const result = await repairLayoutTool({
				spec: spec as any,
				constraints: constraints as any,
				maxIterations: maxIterations ?? 3,
			});
			return {
				content: [
					{ type: "text", text: JSON.stringify(result, null, 2) },
				],
			};
		},
	);

	server.tool(
		"suggest_variants",
		"Generate layout variants at multiple widths to compare how a spec responds to different container sizes.",
		{
			spec: jsonObject,
			widths: z.array(z.number()),
			surface: z.enum(["terminal", "dom"]).optional(),
		},
		async ({ spec, widths, surface }) => {
			const result = suggestVariants({
				spec: spec as any,
				widths,
				surface: surface ?? "terminal",
			});
			return {
				content: [
					{ type: "text", text: JSON.stringify(result, null, 2) },
				],
			};
		},
	);

	server.tool(
		"measure_text",
		"Measure text dimensions in a monospace terminal. Returns line count, height, and width.",
		{
			text: z.string(),
			maxWidth: z.number(),
		},
		async ({ text, maxWidth }) => {
			const result = measureText({ text, maxWidth });
			return {
				content: [
					{ type: "text", text: JSON.stringify(result) },
				],
			};
		},
	);

	return server;
}
