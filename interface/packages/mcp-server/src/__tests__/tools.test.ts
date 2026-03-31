import { describe, it, expect } from "vitest";
import {
	computeLayout,
	validateConstraints,
	repairLayoutTool,
	suggestVariants,
	measureText,
} from "../tools.js";
import type { TextBlockNode, CardNode, LayoutConstraints } from "@life/ikr-ir";

function terminalConstraints(cols: number): LayoutConstraints {
	return {
		width: cols,
		lineHeight: 1,
		surface: { kind: "terminal", cols, rows: 24, monoWidth: 1 as const },
	};
}

describe("MCP Tools", () => {
	const textBlock: TextBlockNode = {
		kind: "textBlock",
		id: "t1",
		text: "Hello World from the Interface Kernel",
		role: "body",
		fontToken: "body",
		constraints: { maxLines: 1 },
	};

	it("compute_layout returns solved layout", () => {
		const result = computeLayout({
			spec: textBlock,
			constraints: terminalConstraints(80),
		});
		expect(result.nodes.length).toBe(1);
		expect(result.nodes[0].id).toBe("t1");
		expect(result.width).toBe(80);
	});

	it("validate_constraints detects violations", () => {
		const result = validateConstraints({
			spec: textBlock,
			constraints: terminalConstraints(10), // very narrow → wraps
		});
		expect(result.violations.length).toBeGreaterThan(0);
		expect(result.violations[0].rule).toBe("maxLines");
	});

	it("validate_constraints returns empty for valid layout", () => {
		const result = validateConstraints({
			spec: textBlock,
			constraints: terminalConstraints(80), // wide enough
		});
		expect(result.violations.length).toBe(0);
	});

	it("repair_layout fixes violations", async () => {
		const result = await repairLayoutTool({
			spec: textBlock,
			constraints: terminalConstraints(10),
		});
		expect(result.repairsApplied.length).toBeGreaterThan(0);
		expect(result.fullyResolved).toBe(true);
	});

	it("repair_layout returns immediately for valid layout", async () => {
		const result = await repairLayoutTool({
			spec: textBlock,
			constraints: terminalConstraints(80),
		});
		expect(result.repairsApplied.length).toBe(0);
		expect(result.iterations).toBe(0);
		expect(result.fullyResolved).toBe(true);
	});

	it("suggest_variants returns results for multiple widths", () => {
		const result = suggestVariants({
			spec: textBlock,
			widths: [20, 40, 60, 80],
			surface: "terminal",
		});
		expect(result.length).toBe(4);
		expect(result[0].width).toBe(20);
		expect(result[3].width).toBe(80);
		// Narrow width should have violations, wide should not
		expect(result[0].violations.length).toBeGreaterThan(0);
		expect(result[3].violations.length).toBe(0);
	});

	it("measure_text returns dimensions", () => {
		const result = measureText({ text: "Hello World", maxWidth: 80 });
		expect(result.lineCount).toBe(1);
		expect(result.width).toBe(11);
	});

	it("measure_text wraps long text", () => {
		const result = measureText({ text: "Hello World", maxWidth: 5 });
		expect(result.lineCount).toBeGreaterThan(1);
	});
});
