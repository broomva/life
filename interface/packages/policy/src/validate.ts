import type { UINode, SolvedLayout, Violation } from '@life/ikr-ir';
import { defaultRules, type PolicyRule } from './rules.js';

export function validate(
  spec: UINode,
  solved: SolvedLayout,
  rules: PolicyRule[] = defaultRules,
): Violation[] {
  const violations: Violation[] = [];
  for (const rule of rules) {
    violations.push(...rule.evaluate(spec, solved));
  }
  // Sort: errors first, then warnings
  violations.sort((a, b) => {
    if (a.severity === 'error' && b.severity === 'warning') return -1;
    if (a.severity === 'warning' && b.severity === 'error') return 1;
    return 0;
  });
  return violations;
}
