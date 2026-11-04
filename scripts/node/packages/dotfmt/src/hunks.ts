import { diffLines } from "diff";

/** Replaces lines `start` up to `end` (exclusive) of the original text with `text`. */
export interface Hunk {
  start: number;
  end: number;
  text: string;
}

interface Block {
  start: number;
  removed: string[];
  added: string[];
}

export function hunks(before: string, after: string): Hunk[] {
  const result: Hunk[] = [];
  let line = 0;
  let block: Block | undefined;
  for (const change of diffLines(before, after)) {
    if (!change.added && !change.removed) {
      if (block) {
        result.push(...split(block));
        block = undefined;
      }
      line += change.count;
      continue;
    }
    block ??= { start: line, removed: [], added: [] };
    const lines = change.value.split(/(?<=\n)/).filter(Boolean);
    if (change.added) {
      block.added.push(...lines);
    } else {
      block.removed.push(...lines);
      line += change.count;
    }
  }
  if (block) {
    result.push(...split(block));
  }
  return result;
}

/** Line-for-line rewrites become one hunk per line, so a range only takes its own lines. */
function split({ start, removed, added }: Block): Hunk[] {
  if (removed.length === added.length) {
    return added.map((text, index) => ({
      start: start + index,
      end: start + index + 1,
      text,
    }));
  }
  return [{ start, end: start + removed.length, text: added.join("") }];
}
