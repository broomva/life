// @life/ikr — Umbrella package re-exporting the full Interface Kernel

// Semantic UI IR types
export type {
	UINode,
	TextBlockNode,
	InlineRowNode,
	CardNode,
	ColumnNode,
	ChipNode,
	IconNode,
	ButtonNode,
	SectionNode,
	TextRole,
	TextConstraints,
	BoxConstraints,
	LayoutConstraints,
	OverflowPolicy,
	Density,
	Surface,
	FontMetrics,
	SolvedLayout,
	SolvedNode,
	Violation,
	RepairStrategy,
	ViolationSeverity,
} from "@life/ikr-ir";

// Layout kernel
export { solveLayout, measureMonoText, monoStringWidth } from "@life/ikr-layout";
export type { TextMeasureFn } from "@life/ikr-layout";

// Constraint policy
export { validate, maxLinesRule, overflowRule, minTouchTargetRule, defaultRules } from "@life/ikr-policy";
export type { PolicyRule } from "@life/ikr-policy";

// AI repair loop
export { repairLayout, applyDeterministicRepairs, buildRepairPrompt, applyRepairPatches } from "@life/ikr-repair";
export type { RepairOptions, RepairResult, RepairPatch } from "@life/ikr-repair";

// Signal runtime
export {
	signal,
	computed,
	effect,
	batch,
	untracked,
	createRoot,
	onCleanup,
	getOwner,
	runWithOwner,
	scheduleLayout,
	flushLayout,
} from "@life/ikr-signals";
export type { Signal, ReadonlySignal, Owner } from "@life/ikr-signals";

// Renderers
export { renderToDOM, createReactiveRenderer, bindToSignal } from "@life/ikr-render-dom";
export {
	renderToTerminal,
	createReactiveTerminalRenderer,
	TerminalBuffer,
} from "@life/ikr-render-terminal";
