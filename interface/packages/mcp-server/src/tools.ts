/**
 * IKR MCP Tools — expose layout kernel capabilities to any MCP client.
 *
 * Tools:
 *   compute_layout      — solve layout for a spec + constraints
 *   validate_constraints — solve + validate, return violations
 *   repair_layout        — two-tier repair, return fixed spec
 *   suggest_variants     — generate layout variants at multiple widths
 *   measure_text         — measure text dimensions for terminal surface
 */
import type {
	UINode,
	LayoutConstraints,
	SolvedLayout,
	Violation,
} from "@life/ikr-ir";
import { solveLayout, measureMonoText } from "@life/ikr-layout";
import { validate } from "@life/ikr-policy";
import { repairLayout } from "@life/ikr-repair";
import type { RepairResult } from "@life/ikr-repair";

export type ComputeLayoutInput = {
	spec: UINode;
	constraints: LayoutConstraints;
};

export type ValidateInput = {
	spec: UINode;
	constraints: LayoutConstraints;
};

export type RepairInput = {
	spec: UINode;
	constraints: LayoutConstraints;
	maxIterations?: number;
};

export type SuggestVariantsInput = {
	spec: UINode;
	widths: number[];
	surface?: "terminal" | "dom";
};

export type MeasureTextInput = {
	text: string;
	maxWidth: number;
};

export function computeLayout(input: ComputeLayoutInput): SolvedLayout {
	return solveLayout(input.spec, input.constraints);
}

export function validateConstraints(
	input: ValidateInput,
): { solved: SolvedLayout; violations: Violation[] } {
	const solved = solveLayout(input.spec, input.constraints);
	const violations = validate(input.spec, solved);
	return { solved, violations };
}

export async function repairLayoutTool(
	input: RepairInput,
): Promise<RepairResult> {
	const solved = solveLayout(input.spec, input.constraints);
	const violations = validate(input.spec, solved);

	if (violations.length === 0) {
		return {
			spec: input.spec,
			solved,
			repairsApplied: [],
			iterations: 0,
			fullyResolved: true,
		};
	}

	return repairLayout(input.spec, violations, {
		constraints: input.constraints,
		maxIterations: input.maxIterations ?? 3,
	});
}

export function suggestVariants(
	input: SuggestVariantsInput,
): Array<{ width: number; solved: SolvedLayout; violations: Violation[] }> {
	return input.widths.map((width) => {
		const constraints: LayoutConstraints = {
			width,
			lineHeight: input.surface === "terminal" ? 1 : 20,
			surface:
				input.surface === "terminal"
					? { kind: "terminal", cols: width, rows: 24, monoWidth: 1 as const }
					: { kind: "raw" },
		};
		const solved = solveLayout(input.spec, constraints);
		const violations = validate(input.spec, solved);
		return { width, solved, violations };
	});
}

export function measureText(
	input: MeasureTextInput,
): { lineCount: number; height: number; width: number } {
	return measureMonoText(input.text, input.maxWidth);
}
