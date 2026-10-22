// Ordering rules for the board (design section 15).

import type { BlockCard, ChatSummary, MoveTarget } from './types';

/** Chats waiting on you, oldest first. */
export function needsYouOrder(chats: ChatSummary[]): ChatSummary[] {
  return [...chats].sort((a, b) => (a.status_since ?? a.last_activity) - (b.status_since ?? b.last_activity));
}

/** Most recently active first. */
export function byLastActive(blocks: BlockCard[]): BlockCard[] {
  return [...blocks].sort((a, b) => b.last_active - a.last_active);
}

/** Most recently active first. */
export function chatsByActivity(chats: ChatSummary[]): ChatSummary[] {
  return [...chats].sort((a, b) => b.last_activity - a.last_activity);
}

/** The blocks a chat can be moved to, in board order. */
export function moveTargets(blocks: BlockCard[]): MoveTarget[] {
  return blocks.filter((b) => !b.archived).map((b) => ({ id: b.id, title: b.title }));
}
