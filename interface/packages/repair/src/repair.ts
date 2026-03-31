import type {
  UINode,
  LayoutConstraints,
  SolvedLayout,
  Violation,
} from '@life/ikr-ir';
import { solveLayout } from '@life/ikr-layout';
import { validate } from '@life/ikr-policy';
import { applyDeterministicRepairs } from './deterministic.js';
import {
  buildRepairPrompt,
  applyRepairPatches,
  type RepairPatch,
} from './llm-repair.js';

export type RepairOptions = {
  constraints: LayoutConstraints;
  /** AI model for Tier 2 repair. If not provided, only deterministic repair is used. */
  model?: string;
  /** Maximum repair iterations. Default: 3 */
  maxIterations?: number;
  /** Custom LLM call function. Allows injecting AI SDK or mocking. */
  callLLM?: (prompt: string) => Promise<RepairPatch[]>;
};

export type RepairResult = {
  spec: UINode;
  solved: SolvedLayout;
  repairsApplied: string[];
  iterations: number;
  fullyResolved: boolean;
};

/**
 * Two-tier layout repair.
 *
 * Tier 1: Deterministic rules (collapse chips, switch density, hide nodes).
 * Tier 2: LLM-based semantic compression (summarize text, rewrite labels).
 *
 * Re-solves layout after each repair pass. Returns when valid or max iterations reached.
 */
export async function repairLayout(
  spec: UINode,
  violations: Violation[],
  options: RepairOptions,
): Promise<RepairResult> {
  const maxIter = options.maxIterations ?? 3;
  let current = spec;
  let currentViolations = violations;
  const allRepairs: string[] = [];
  let iteration = 0;

  while (currentViolations.length > 0 && iteration < maxIter) {
    iteration++;

    // Tier 1: deterministic
    const { repaired, applied, remaining } = applyDeterministicRepairs(
      current,
      currentViolations,
    );
    current = repaired;
    allRepairs.push(...applied);

    // Re-solve after deterministic repairs
    let solved = solveLayout(current, options.constraints);
    currentViolations = validate(current, solved);

    if (currentViolations.length === 0) {
      return {
        spec: current,
        solved,
        repairsApplied: allRepairs,
        iterations: iteration,
        fullyResolved: true,
      };
    }

    // Tier 2: LLM (only if callLLM is provided)
    if (options.callLLM && remaining.length > 0) {
      const prompt = buildRepairPrompt(current, currentViolations);
      const patches = await options.callLLM(prompt);
      current = applyRepairPatches(current, patches);
      allRepairs.push(
        ...patches.map((p) => `llm:${p.action}:${p.nodeId}`),
      );

      // Re-solve after LLM repairs
      solved = solveLayout(current, options.constraints);
      currentViolations = validate(current, solved);
    }

    if (currentViolations.length === 0) {
      const finalSolved = solveLayout(current, options.constraints);
      return {
        spec: current,
        solved: finalSolved,
        repairsApplied: allRepairs,
        iterations: iteration,
        fullyResolved: true,
      };
    }
  }

  const finalSolved = solveLayout(current, options.constraints);
  return {
    spec: current,
    solved: finalSolved,
    repairsApplied: allRepairs,
    iterations: iteration,
    fullyResolved: currentViolations.length === 0,
  };
}
