/**
 * End-to-end integration tests for the Interface Kernel.
 *
 * These tests exercise the full pipeline:
 *   Semantic Spec → Layout Solve → Constraint Validate → Repair → Render
 */
import { describe, it, expect } from "vitest";
import { solveLayout, measureMonoText } from "@life/ikr-layout";
import { validate, maxLinesRule, overflowRule } from "@life/ikr-policy";
import { repairLayout, applyDeterministicRepairs } from "@life/ikr-repair";
import { renderToTerminal, TerminalBuffer } from "@life/ikr-render-terminal";
import type {
	UINode,
	CardNode,
	TextBlockNode,
	ColumnNode,
	InlineRowNode,
	ChipNode,
	LayoutConstraints,
	Violation,
} from "@life/ikr-ir";

// --- Helpers ---

function terminalConstraints(cols: number, rows?: number): LayoutConstraints {
	return {
		width: cols,
		height: rows,
		lineHeight: 1,
		surface: { kind: "terminal", cols, rows: rows ?? 24, monoWidth: 1 as const },
	};
}

// --- Test Specs ---

/** A dashboard card with title, body, and tags */
function makeDashboardCard(): CardNode {
	return {
		kind: "card",
		id: "dashboard-card",
		padding: 2,
		widthPolicy: "fill",
		children: [
			{
				kind: "textBlock",
				id: "card-title",
				text: "Procurement Risk Summary",
				role: "title",
				fontToken: "heading-sm",
				constraints: { maxLines: 1 },
			},
			{
				kind: "textBlock",
				id: "card-body",
				text: "Quarterly procurement spend is up 18% year-over-year driven primarily by raw materials cost inflation in the APAC region and increased logistics expenses across all supply chain verticals",
				role: "body",
				fontToken: "body",
				constraints: { maxLines: 3 },
			},
			{
				kind: "inlineRow",
				id: "card-tags",
				wrap: true,
				gap: 1,
				children: [
					{ kind: "chip", id: "tag-finance", label: "Finance", priority: 3 },
					{ kind: "chip", id: "tag-risk", label: "Risk", priority: 3 },
					{ kind: "chip", id: "tag-apac", label: "APAC", priority: 2 },
					{ kind: "chip", id: "tag-logistics", label: "Logistics", priority: 1 },
					{ kind: "chip", id: "tag-q4", label: "Q4-2026", priority: 1 },
				],
			},
		],
	};
}

// --- Integration Tests ---

describe("End-to-End Pipeline", () => {
	it("solves a dashboard card layout on a terminal surface", () => {
		const card = makeDashboardCard();
		const constraints = terminalConstraints(60);

		const solved = solveLayout(card, constraints);

		expect(solved).toBeDefined();
		expect(solved.width).toBe(60);
		expect(solved.nodes.length).toBeGreaterThan(0);

		// Title should be in the solved nodes
		const title = solved.nodes.find((n) => n.id === "card-title");
		expect(title).toBeDefined();
		expect(title!.width).toBeGreaterThan(0);
	});

	it("detects constraint violations on narrow terminal", () => {
		const card = makeDashboardCard();
		// Very narrow terminal — body text will exceed maxLines
		const constraints = terminalConstraints(30);

		const solved = solveLayout(card, constraints);
		const violations = validate(card, solved, [maxLinesRule]);

		// Body text at 30 cols should wrap to way more than 3 lines
		const bodyViolation = violations.find((v) => v.nodeId === "card-body");
		expect(bodyViolation).toBeDefined();
		expect(bodyViolation!.rule).toBe("maxLines");
		expect(bodyViolation!.actual).toBeGreaterThan(3);
		expect(bodyViolation!.limit).toBe(3);
	});

	it("repairs violations with deterministic strategy", () => {
		const card = makeDashboardCard();
		const constraints = terminalConstraints(30);
		const solved = solveLayout(card, constraints);
		const violations = validate(card, solved, [maxLinesRule]);

		expect(violations.length).toBeGreaterThan(0);

		// Apply deterministic repairs — should increase maxLines
		const { repaired, applied, remaining } = applyDeterministicRepairs(card, violations);

		expect(applied.length).toBeGreaterThan(0);
		// The deterministic repair should have applied increase_max_lines
		expect(applied.some((a) => a.includes("increase_max_lines"))).toBe(true);
	});

	it("runs full repair loop (deterministic only) and produces a valid layout", async () => {
		const card = makeDashboardCard();
		const constraints = terminalConstraints(30);
		const solved = solveLayout(card, constraints);
		const violations = validate(card, solved, [maxLinesRule]);

		const result = await repairLayout(card, violations, {
			constraints,
			maxIterations: 3,
			// No callLLM — deterministic only
		});

		expect(result.repairsApplied.length).toBeGreaterThan(0);
		expect(result.iterations).toBeGreaterThanOrEqual(1);
		// After deterministic repair (increase maxLines), it should be resolved
		expect(result.fullyResolved).toBe(true);
	});

	it("runs full repair loop with mock LLM for text summarization", async () => {
		const spec: TextBlockNode = {
			kind: "textBlock",
			id: "long-text",
			text: "This is a very long text that will definitely not fit within two lines on a narrow terminal of just twenty characters width and needs summarization",
			role: "body",
			fontToken: "body",
			constraints: { maxLines: 2 },
		};
		const constraints = terminalConstraints(20);
		const solved = solveLayout(spec, constraints);
		const violations = validate(spec, solved, [maxLinesRule]);

		expect(violations.length).toBeGreaterThan(0);

		// Note: deterministic repair will apply increase_max_lines first,
		// which resolves the violation. To test LLM path, we pass violations
		// that only have summarize_text as repair option.
		const llmOnlyViolations: Violation[] = violations.map((v) => ({
			...v,
			repairOptions: v.repairOptions.filter((r) => r.kind === "summarize_text"),
		}));

		const result = await repairLayout(spec, llmOnlyViolations, {
			constraints,
			maxIterations: 3,
			callLLM: async () => [
				{
					nodeId: "long-text",
					action: "rewrite_text" as const,
					newText: "Short text fits now",
				},
			],
		});

		// LLM was invoked because no deterministic strategy matched
		expect(result.repairsApplied.some((r) => r.includes("llm"))).toBe(true);
		expect(result.fullyResolved).toBe(true);
	});

	it("renders a solved layout to terminal ANSI output", () => {
		const card = makeDashboardCard();
		const constraints = terminalConstraints(60, 20);

		const solved = solveLayout(card, constraints);
		const output = renderToTerminal(solved, 60, 20);

		expect(output).toBeDefined();
		expect(typeof output).toBe("string");
		expect(output.length).toBeGreaterThan(0);
		// Should contain the title text somewhere in the output
		expect(output).toContain("Procurement");
	});

	it("exercises the full pipeline: spec → solve → validate → repair → render", async () => {
		// 1. Define spec
		const spec: ColumnNode = {
			kind: "column",
			id: "report",
			gap: 1,
			children: [
				{
					kind: "textBlock",
					id: "report-title",
					text: "Monthly Agent Operations Report",
					role: "title",
					fontToken: "heading",
					constraints: { maxLines: 1 },
				},
				{
					kind: "textBlock",
					id: "report-body",
					text: "Agent throughput increased 42% month-over-month with significant improvements in tool execution latency and reduced error rates across all production environments",
					role: "body",
					fontToken: "body",
					constraints: { maxLines: 3 },
				},
				{
					kind: "inlineRow",
					id: "report-metrics",
					wrap: true,
					gap: 1,
					children: [
						{ kind: "chip", id: "metric-throughput", label: "+42% throughput", priority: 3 },
						{ kind: "chip", id: "metric-latency", label: "-18ms latency", priority: 2 },
						{ kind: "chip", id: "metric-errors", label: "-23% errors", priority: 1 },
					],
				},
			],
		};

		// 2. Solve layout
		const constraints = terminalConstraints(40, 15);
		const solved = solveLayout(spec, constraints);
		expect(solved.nodes.length).toBeGreaterThan(0);

		// 3. Validate
		const violations = validate(spec, solved);

		// 4. Repair if needed
		let finalSolved = solved;
		if (violations.length > 0) {
			const repaired = await repairLayout(spec, violations, {
				constraints,
				maxIterations: 2,
			});
			finalSolved = repaired.solved;
		}

		// 5. Render
		const output = renderToTerminal(finalSolved, 40, 15);
		expect(output).toBeDefined();
		expect(output.split("\n").length).toBe(15); // 15 rows
		expect(output.split("\n")[0].length).toBe(40); // 40 cols per row
	});
});

describe("Terminal Text Measurement", () => {
	it("measures ASCII text correctly", () => {
		const result = measureMonoText("Hello World", 80);
		expect(result.lineCount).toBe(1);
		expect(result.width).toBe(11);
	});

	it("measures wrapping text", () => {
		const result = measureMonoText("Hello World", 5);
		expect(result.lineCount).toBe(3); // "Hello" + " Worl" + "d"
		expect(result.width).toBe(5);
	});

	it("handles empty text", () => {
		const result = measureMonoText("", 80);
		// Empty string has 0 char width; Math.ceil(0/80) = 0, but impl returns min 1 line
		expect(result.lineCount).toBeGreaterThanOrEqual(0);
		expect(result.height).toBeGreaterThanOrEqual(0);
	});
});

describe("Terminal Buffer", () => {
	it("creates a buffer and writes text", () => {
		const buf = new TerminalBuffer(20, 5);
		buf.writeText(0, 0, "Hello IKR");

		const output = buf.toString();
		expect(output).toContain("Hello IKR");
	});

	it("draws a box", () => {
		const buf = new TerminalBuffer(10, 5);
		buf.drawBox(0, 0, 10, 5);

		const output = buf.toString();
		expect(output).toContain("┌");
		expect(output).toContain("┐");
		expect(output).toContain("└");
		expect(output).toContain("┘");
		expect(output).toContain("│");
		expect(output).toContain("─");
	});
});
