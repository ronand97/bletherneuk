// Worktree names for "New chat → new worktree" must match the service's and bridge's rule
// (docs/protocol.md sections 1.2 and 2.5): ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$.
// The first character is a letter or digit, so a name can never be read as a command-line flag.

export const WORKTREE_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

/** Why a worktree name can't be used, or null when it can. */
export function worktreeNameProblem(name: string): string | null {
  if (name.length === 0) return 'Enter a name for the worktree.';
  if (name.length > 64) return 'Use at most 64 characters.';
  if (!/^[A-Za-z0-9]/.test(name)) return 'Start with a letter or digit.';
  if (!WORKTREE_NAME.test(name)) return 'Use only letters, digits, dots, hyphens and underscores.';
  return null;
}
